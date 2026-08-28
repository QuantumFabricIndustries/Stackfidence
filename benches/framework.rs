//! Benchmarks for the AgentStack framework.
//!
//! Run with: `cargo bench`
//!
//! Measures the overhead of each layer in isolation and the full executor
//! pipeline. Mirrors VerifyStack's `_bench_v*.py` pattern: measure real
//! scenarios, not micro-synthetic loops.

use agent_stack::agent::{Agent, AgentContext, AgentOutput};
use agent_stack::blackboard::SlotKey;
use agent_stack::budget::{Budget, BudgetLimit, ResourceKind};
use agent_stack::causal::CausalTrace;
use agent_stack::executor::RunConfig;
use agent_stack::graph::{Edge, Graph, Node, NodeKind};
use agent_stack::id::AgentId;
use agent_stack::loop_spec::LoopSpec;
use agent_stack::runtime::Runtime;
use agent_stack::validate::InputValidator;
use agent_stack::value::Value;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;

/// A minimal agent that just returns its input.
struct Passthrough { id: AgentId }
impl Agent for Passthrough {
    fn id(&self) -> AgentId { self.id }
    fn name(&self) -> &str { "passthrough" }
    fn run(&self, _ctx: &AgentContext) -> agent_stack::error::AgentResult<AgentOutput> {
        Ok(AgentOutput::done(Value::str("ok")))
    }
}

/// An agent that reads + writes the blackboard (exercises layer 2).
struct BlackboardAgent { id: AgentId }
impl Agent for BlackboardAgent {
    fn id(&self) -> AgentId { self.id }
    fn name(&self) -> &str { "bb" }
    fn run(&self, ctx: &AgentContext) -> agent_stack::error::AgentResult<AgentOutput> {
        let v = ctx.blackboard.read(ctx.agent, &SlotKey::new("bench"))?;
        ctx.blackboard.write(ctx.agent, &SlotKey::new("bench"), v)?;
        Ok(AgentOutput::done(Value::null()))
    }
}

fn bench_linear_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("linear_graph");
    for n in [2, 10, 50, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(n), n, |b, &n| {
            b.iter(|| {
                let rt = Runtime::permissive();
                rt.register("p", Passthrough { id: AgentId::new() });
                let mut g = Graph::new("linear");
                let mut prev = None;
                for i in 0..n {
                    let node = Node::agent(format!("n{}", i), "p").with_output_slot(format!("out.{}", i));
                    let id = g.add_node(node);
                    if let Some(p) = prev {
                        g.add_edge(Edge::normal(p, id)).unwrap();
                    }
                    prev = Some(id);
                }
                rt.run(&g, RunConfig::default()).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_loop_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("loop_graph");
    for iters in [1, 10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(iters), iters, |b, &iters| {
            b.iter(|| {
                let rt = Runtime::permissive();
                rt.register("p", Passthrough { id: AgentId::new() });
                let mut g = Graph::new("looped");
                let _n = g.add_node(
                    Node::agent("loop", "p")
                        .with_output_slot("out.loop")
                        .with_loop(LoopSpec::repeat(iters as u32)),
                );
                rt.run(&g, RunConfig::default()).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_blackboard_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("blackboard");
    group.bench_function("read_write", |b| {
        let rt = Runtime::permissive();
        let agent = AgentId::new();
        b.iter(|| {
            let v = rt.blackboard.read(agent, &SlotKey::new("bench")).unwrap();
            rt.blackboard.write(agent, &SlotKey::new("bench"), v).unwrap();
        });
    });
    group.bench_function("update_rmw", |b| {
        let rt = Runtime::permissive();
        let agent = AgentId::new();
        rt.blackboard.write(agent, &SlotKey::new("count"), Value::int(0)).unwrap();
        b.iter(|| {
            rt.blackboard.update(agent, &SlotKey::new("count"), |v| {
                if let Value::Int(i) = v { Value::Int(i + 1) } else { Value::Int(1) }
            }).unwrap();
        });
    });
    group.finish();
}

fn bench_blackboard_graph(c: &mut Criterion) {
    // A graph where every node reads + writes the blackboard (exercises layers
    // 1 + 2 + 3 together, more realistic than the passthrough linear graph).
    let mut group = c.benchmark_group("blackboard_graph");
    for n in [2, 10, 50].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(n), n, |b, &n| {
            b.iter(|| {
                let rt = Runtime::permissive();
                rt.register("bb", BlackboardAgent { id: AgentId::new() });
                let mut g = Graph::new("bb-chain");
                let mut prev = None;
                for i in 0..n {
                    let node = Node::agent(format!("n{}", i), "bb").with_output_slot(format!("out.{}", i));
                    let id = g.add_node(node);
                    if let Some(p) = prev {
                        g.add_edge(Edge::normal(p, id)).unwrap();
                    }
                    prev = Some(id);
                }
                rt.run(&g, RunConfig::default().with_seed(vec![("bench".into(), Value::int(0))])).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_causal_trace(c: &mut Criterion) {
    let mut group = c.benchmark_group("causal_trace");
    for n in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(n), n, |b, &n| {
            b.iter(|| {
                let t = CausalTrace::new();
                let node = agent_stack::id::NodeId::new();
                let agent = AgentId::new();
                let mut parent = None;
                for i in 0..n {
                    let cause = match parent {
                        Some(p) => agent_stack::causal::Cause::Parent(p),
                        None => agent_stack::causal::Cause::External("bench".into()),
                    };
                    let span = t.open(node, agent, cause);
                    t.add_effect(span, agent_stack::causal::EffectKind::Decision, format!("d{}", i), vec![]).unwrap();
                    t.close(span);
                    parent = Some(span);
                }
                black_box(t.all_spans());
            });
        });
    }
    group.finish();
}

fn bench_budget(c: &mut Criterion) {
    let mut group = c.benchmark_group("budget");
    group.bench_function("spend", |b| {
        let budget = Arc::new(Budget::new(vec![BudgetLimit { kind: ResourceKind::Tokens, cap: 1_000_000 }]));
        let span = agent_stack::id::SpanId::new();
        b.iter(|| {
            budget.spend(ResourceKind::Tokens, 1, span).unwrap();
        });
    });
    group.finish();
}

fn bench_input_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("input_validation");
    let validator = Arc::new(InputValidator::new());
    let small = Value::obj(vec![("a", Value::int(1)), ("b", Value::str("hello"))]);
    let large = Value::obj((0..20).map(|i| (format!("f{}", i), Value::int(i))).collect());
    group.bench_function("small_object", |b| {
        b.iter(|| validator.validate(&small).unwrap());
    });
    group.bench_function("large_object", |b| {
        b.iter(|| validator.validate(&large).unwrap());
    });
    group.finish();
}

fn bench_subgraph(c: &mut Criterion) {
    let mut group = c.benchmark_group("subgraph");
    for depth in [1, 3, 5].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(depth), depth, |b, &depth| {
            b.iter(|| {
                let rt = Runtime::permissive();
                rt.register("p", Passthrough { id: AgentId::new() });

                // Build a chain of nested subgraphs of the given depth.
                let mut current_id = None;
                for _ in 0..depth {
                    let mut child = Graph::new("child");
                    let inner = match current_id {
                        Some(id) => {
                            child.add_node(Node {
                                label: "sub".into(),
                                kind: NodeKind::Subgraph(id),
                                ..Node::router("sub")
                            }.with_output_slot("out.sub"))
                        }
                        None => {
                            child.add_node(Node::agent("leaf", "p").with_output_slot("out.leaf"))
                        }
                    };
                    let _ = inner;
                    current_id = Some(rt.register_subgraph(child));
                }

                let mut parent = Graph::new("parent");
                let _top = parent.add_node(Node {
                    label: "top".into(),
                    kind: NodeKind::Subgraph(current_id.unwrap()),
                    ..Node::router("top")
                }.with_output_slot("out.sub"));

                rt.run(&parent, RunConfig::default()).unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_linear_graph,
    bench_loop_graph,
    bench_blackboard_contention,
    bench_blackboard_graph,
    bench_causal_trace,
    bench_budget,
    bench_input_validation,
    bench_subgraph,
);
criterion_main!(benches);
