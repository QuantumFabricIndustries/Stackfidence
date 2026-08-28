//! `agentstack` CLI binary.
//!
//! Runs a graph from a JSON spec file against a runtime of mock agents, or
//! dumps a recorded trace. The spec format is minimal and documented in the
//! README. Real LLM agents require the `llm` feature and an API key.

use agent_stack::executor::RunConfig;
use agent_stack::graph::{Edge, EdgeKind, Graph, Node};
use agent_stack::runtime::Runtime;
use agent_stack::value::Value;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agentstack", version, about = "Run an AgentStack graph")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a graph from a JSON spec file.
    Run {
        /// Path to the graph spec JSON.
        spec: PathBuf,
        /// Optional seed file (JSON object of slot -> value).
        #[arg(long)]
        seed: Option<PathBuf>,
        /// Save the causal trace to this path as JSON.
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// Print the framework's 13 layers and what each module implements.
    Layers,
}

#[derive(Serialize, Deserialize)]
struct SpecNode {
    label: String,
    agent: String,
    #[serde(default)]
    output_slot: Option<String>,
    #[serde(default)]
    loop_kind: Option<String>,
    #[serde(default)]
    loop_count: Option<u32>,
    #[serde(default)]
    loop_slot: Option<String>,
    #[serde(default)]
    loop_target: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
struct SpecEdge {
    from: String,
    to: String,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Spec {
    name: String,
    nodes: Vec<SpecNode>,
    edges: Vec<SpecEdge>,
    #[serde(default)]
    entry: Option<String>,
}

fn build_graph(spec: &Spec) -> Result<Graph, String> {
    let mut g = Graph::new(&spec.name);
    let mut labels: std::collections::HashMap<String, _> = std::collections::HashMap::new();

    for n in &spec.nodes {
        let mut node = Node::agent(&n.label, &n.agent);
        if let Some(slot) = &n.output_slot {
            node = node.with_output_slot(slot);
        }
        if let Some(kind) = &n.loop_kind {
            let ls = match kind.as_str() {
                "repeat" => agent_stack::loop_spec::LoopSpec::repeat(n.loop_count.unwrap_or(1)),
                "while" => agent_stack::loop_spec::LoopSpec::while_eq(
                    n.loop_slot.clone().unwrap_or_default(),
                    Value::from_json(&n.loop_target.clone().unwrap_or(serde_json::Value::Null)),
                    n.loop_count.unwrap_or(100),
                ),
                "until" => agent_stack::loop_spec::LoopSpec::until_eq(
                    n.loop_slot.clone().unwrap_or_default(),
                    Value::from_json(&n.loop_target.clone().unwrap_or(serde_json::Value::Null)),
                    n.loop_count.unwrap_or(100),
                ),
                "foreach" => agent_stack::loop_spec::LoopSpec::foreach(
                    n.loop_slot.clone().unwrap_or_default(),
                    n.loop_count.unwrap_or(100),
                ),
                other => return Err(format!("unknown loop_kind: {}", other)),
            };
            node = node.with_loop(ls);
        }
        let id = g.add_node(node);
        labels.insert(n.label.clone(), id);
    }
    for e in &spec.edges {
        let from = *labels.get(&e.from).ok_or_else(|| format!("unknown node: {}", e.from))?;
        let to = *labels.get(&e.to).ok_or_else(|| format!("unknown node: {}", e.to))?;
        let edge = match e.kind.as_deref().unwrap_or("normal") {
            "normal" => Edge::normal(from, to),
            "error" => {
                Edge { kind: EdgeKind::OnError, ..Edge::normal(from, to) }
            }
            other => return Err(format!("unsupported edge kind in CLI: {}", other)),
        };
        g.add_edge(edge).map_err(|e| format!("{}", e))?;
    }
    if let Some(entry) = &spec.entry {
        if let Some(id) = labels.get(entry) {
            g.set_entry(*id);
        }
    }
    Ok(g)
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { spec, seed, trace } => {
            let spec_text = std::fs::read_to_string(&spec).unwrap_or_else(|e| {
                eprintln!("failed to read spec {}: {}", spec.display(), e);
                std::process::exit(1);
            });
            let spec_json: Spec = serde_json::from_str(&spec_text).unwrap_or_else(|e| {
                eprintln!("failed to parse spec: {}", e);
                std::process::exit(1);
            });
            let graph = build_graph(&spec_json).unwrap_or_else(|e| {
                eprintln!("{}", e);
                std::process::exit(1);
            });

            let rt = Runtime::permissive();
            // Register identity mock agents for every agent label in the spec
            // so the graph runs out of the box.
            for n in &spec_json.nodes {
                rt.register(&n.agent, agent_stack::mock_agent::MockAgent::identity(&n.agent));
            }

            let mut config = RunConfig::default();
            if let Some(seed_path) = seed {
                let seed_text = std::fs::read_to_string(&seed_path).unwrap_or_else(|e| {
                    eprintln!("failed to read seed {}: {}", seed_path.display(), e);
                    std::process::exit(1);
                });
                let seed_obj: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&seed_text).unwrap_or_else(|e| {
                        eprintln!("failed to parse seed: {}", e);
                        std::process::exit(1);
                    });
                let seed_vals: Vec<(String, Value)> = seed_obj
                    .into_iter()
                    .map(|(k, v)| (k, Value::from_json(&v)))
                    .collect();
                config = config.with_seed(seed_vals);
            }

            match rt.run(&graph, config) {
                Ok(out) => {
                    println!(
                        "run {}: status={:?} steps={} spans={}",
                        out.run_id, out.status, out.steps, out.span_count
                    );
                    if let Some(int) = &out.interrupted {
                        println!("interrupted: {}", int);
                    }
                    println!("blackboard:");
                    let mut slots: Vec<_> = out.final_snapshot.iter().collect();
                    slots.sort_by(|a, b| a.0.cmp(b.0));
                    for (k, v) in slots {
                        println!("  {} = {}", k, v);
                    }
                    if let Some(trace_path) = trace {
                        let rec = agent_stack::trace_store::TraceRecord::from_trace(&rt.trace);
                        if let Err(e) = rec.save(&trace_path) {
                            eprintln!("failed to save trace: {}", e);
                            std::process::exit(1);
                        }
                        println!("trace saved to {}", trace_path.display());
                    }
                }
                Err(e) => {
                    eprintln!("run failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Cmd::Layers => {
            println!("AgentStack — 13 layers:");
            println!("  1.  graph + loops          — graph.rs, loop_spec.rs, executor.rs");
            println!("  2.  coordination substrate — blackboard.rs, policy.rs");
            println!("  3.  causal memory          — causal.rs, trace_store.rs");
            println!("  4.  trust propagation      — trust.rs");
            println!("  5.  goal/intent model      — goal.rs");
            println!("  6.  meta-cognition         — meta.rs");
            println!("  7.  resource budget        — budget.rs");
            println!("  8.  interrupt propagation  — interrupt.rs, error.rs");
            println!("  9.  negotiation/contracting— contract.rs, negotiation.rs");
            println!(" 10.  determinism/replay     — record.rs, replay.rs");
            println!(" 11.  input validation       — validate.rs");
            println!(" 12.  memory decay           — memory.rs");
            println!(" 13.  human-in-the-loop      — human.rs");
        }
    }
}
