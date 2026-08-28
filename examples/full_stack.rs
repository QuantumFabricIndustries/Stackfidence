//! Full-stack example: a run that touches every layer, with a printed report.
//!
//! Run: `cargo run --example full_stack`

use agent_stack::agent::{Agent, AgentContext, AgentOutput};
use agent_stack::budget::{Budget, BudgetLimit, ResourceKind};
use agent_stack::causal::CausalTrace;
use agent_stack::executor::{RunConfig, RunStatus};
use agent_stack::graph::{Edge, Graph, Node, NodeKind};
use agent_stack::id::AgentId;
use agent_stack::loop_spec::LoopSpec;
use agent_stack::meta::{Confidence, MetaSignal};
use agent_stack::runtime::Runtime;
use agent_stack::trace_store::TraceRecord;
use agent_stack::validate::InputValidator;
use agent_stack::value::Value;
use std::sync::Arc;

struct Counter { id: AgentId }
impl Agent for Counter {
    fn id(&self) -> AgentId { self.id }
    fn name(&self) -> &str { "counter" }
    fn run(&self, ctx: &AgentContext) -> agent_stack::error::AgentResult<AgentOutput> {
        ctx.spend(ResourceKind::Tokens, 1)?;
        ctx.emit_meta(MetaSignal::Confidence(ctx.span, ctx.agent, Confidence::high()));
        let cur = ctx.blackboard.read(ctx.agent, &"count".into())?;
        let newv = if let Value::Int(i) = cur { Value::Int(i + 1) } else { Value::Int(1) };
        ctx.blackboard.write(ctx.agent, &"count".into(), newv.clone())?;
        Ok(AgentOutput::done_with(newv, Confidence::high()))
    }
}

fn main() {
    let rt = Runtime::new(
        agent_stack::blackboard::Blackboard::permissive(),
        Arc::new(Budget::new(vec![BudgetLimit { kind: ResourceKind::Tokens, cap: 1000 }])),
        Arc::new(CausalTrace::new()),
        agent_stack::goal::Goal::trivial(),
        Arc::new(InputValidator::new()),
    );
    rt.set_human_handler(agent_stack::human::auto_approve_handler());
    rt.set_negotiation_handler(Box::new(|_l, i| Ok(i.clone())));
    rt.register("counter", Counter { id: AgentId::new() });

    let mut g = Graph::new("full");
    let a = g.add_node(
        Node::agent("counter", "counter")
            .with_output_slot("out.counter")
            .with_loop(LoopSpec::repeat(5)),
    );
    let h = g.add_node(Node { label: "review".into(), kind: NodeKind::Human("review".into()), ..Node::router("review") }.with_output_slot("out.human"));
    g.add_edge(Edge::normal(a, h)).unwrap();

    let out = rt.run(&g, RunConfig::default().with_seed(vec![("count".into(), Value::int(0))])).unwrap();

    println!("=== AgentStack full-stack run ===");
    println!("status:       {:?}", out.status);
    println!("steps:        {}", out.steps);
    println!("spans:        {}", out.span_count);
    println!("tokens spent: {}", rt.budget.spent(ResourceKind::Tokens));
    println!("final count:  {}", out.final_snapshot.get("count").cloned().unwrap_or(Value::null()));
    println!("interrupted:  {:?}", out.interrupted);

    // Persist the causal trace and reload it to show explainability (layer 3).
    let rec = TraceRecord::from_trace(&rt.trace);
    let tmp = std::env::temp_dir().join("agentstack_trace.json");
    rec.save(&tmp).unwrap();
    let back = TraceRecord::load(&tmp).unwrap();
    println!("trace saved+reloaded: {} spans", back.spans.len());
    println!("  first span cause:   {:?}", back.spans.first().map(|s| &s.cause));
    println!("  first span effects: {}", back.spans.first().map(|s| s.effects.len()).unwrap_or(0));

    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(out.final_snapshot.get("count").cloned().unwrap_or(Value::null()), Value::int(5));
    println!("\nAll 13 layers exercised. OK.");
}
