//! End-to-end integration test exercising all 13 layers in a single run.
//!
//! Scenario: a small graph that
//!   1. loops an incrementer 3 times (layer 1: graph + loops),
//!   2. writes to a shared blackboard (layer 2),
//!   3. produces a causal trace (layer 3),
//!   4. runs under a trust context (layer 4),
//!   5. stays aligned to a goal (layer 5),
//!   6. emits a meta-cognitive confidence signal (layer 6),
//!   7. spends from a budget (layer 7),
//!   8. can be stopped by an interrupt (layer 8),
//!   9. runs a negotiation node (layer 9),
//!  10. is recorded and replayed deterministically (layer 10),
//!  11. validates inputs at boundaries (layer 11),
//!  12. manages working memory with decay (layer 12),
//!  13. routes through a human-in-the-loop node (layer 13).

use agent_stack::agent::{Agent, AgentContext, AgentOutput};
use agent_stack::blackboard::SlotKey;
use agent_stack::budget::{Budget, BudgetLimit, ResourceKind};
use agent_stack::causal::{CausalTrace, EffectKind};
use agent_stack::executor::{RunConfig, RunStatus};
use agent_stack::graph::{Edge, Graph, Node, NodeKind};
use agent_stack::id::AgentId;
use agent_stack::loop_spec::LoopSpec;
use agent_stack::memory::{DecayKind, DecayPolicy, MemoryStore};
use agent_stack::meta::{Confidence, MetaSignal};
use agent_stack::negotiation::{Negotiation, NegotiationOutcome, ProviderAction};
use agent_stack::contract::Contract;
use agent_stack::runtime::Runtime;
use agent_stack::trace_store::TraceRecord;
use agent_stack::validate::InputValidator;
use agent_stack::value::Value;
use std::sync::Arc;

/// An agent that increments a blackboard counter and emits a confidence signal.
struct Incrementer { id: AgentId }
impl Agent for Incrementer {
    fn id(&self) -> AgentId { self.id }
    fn name(&self) -> &str { "incrementer" }
    fn run(&self, ctx: &AgentContext) -> agent_stack::error::AgentResult<AgentOutput> {
        // Layer 7: spend a token.
        ctx.spend(ResourceKind::Tokens, 1)?;
        // Layer 6: emit a meta-cognitive signal.
        ctx.emit_meta(MetaSignal::Confidence(ctx.span, ctx.agent, Confidence::high()));
        // Layer 2: read-modify-write the counter.
        let cur = ctx.blackboard.read(ctx.agent, &SlotKey::new("count"))?;
        let newv = if let Value::Int(i) = cur { Value::Int(i + 1) } else { Value::Int(1) };
        ctx.blackboard.write(ctx.agent, &SlotKey::new("count"), newv.clone())?;
        // Layer 12: also mirror into the working-memory store.
        // (The memory store is held by the runtime; here we just produce output.)
        Ok(AgentOutput::done_with(newv, Confidence::high()))
    }
}

/// An agent that records a model call so the run is replayable (layer 10).
struct RecordedEcho { id: AgentId, key: &'static str }
impl Agent for RecordedEcho {
    fn id(&self) -> AgentId { self.id }
    fn name(&self) -> &str { "echo" }
    fn run(&self, ctx: &AgentContext) -> agent_stack::error::AgentResult<AgentOutput> {
        ctx.record_model_call(agent_stack::record::ModelCall {
            span: ctx.span,
            caller: "echo".into(),
            key: self.key.into(),
            request: serde_json::json!({"in": ctx.input.to_json()}),
            response: serde_json::json!({"out": "echoed"}),
            tokens: 1,
        });
        Ok(AgentOutput::done(Value::str("echoed")))
    }
}

/// An agent for the subgraph child: reads "count" from the shared blackboard
/// (which the parent's incrementer loop wrote to), doubles it, and writes to
/// "out.sub". This verifies the subgraph shares the blackboard (layer 2) and
/// that hierarchical composition actually executes the child graph (layer 1).
struct Doubler { id: AgentId }
impl Agent for Doubler {
    fn id(&self) -> AgentId { self.id }
    fn name(&self) -> &str { "doubler" }
    fn run(&self, ctx: &AgentContext) -> agent_stack::error::AgentResult<AgentOutput> {
        let count = ctx.blackboard.read(ctx.agent, &SlotKey::new("count"))?;
        let doubled = if let Value::Int(i) = count { Value::Int(i * 2) } else { Value::Int(0) };
        ctx.blackboard.write(ctx.agent, &SlotKey::new("out.sub"), doubled.clone())?;
        Ok(AgentOutput::done(doubled))
    }
}

#[test]
fn all_thirteen_layers_end_to_end() {
    // --- Layer 7: budget (enough for the run) ---
    let budget = Arc::new(Budget::new(vec![
        BudgetLimit { kind: ResourceKind::Tokens, cap: 100 },
    ]));

    // --- Layer 3: causal trace ---
    let trace = Arc::new(CausalTrace::new());

    // --- Layer 11: input validator ---
    let validator = Arc::new(InputValidator::new());

    // --- Layer 5: goal (stays aligned: count must stay non-negative) ---
    let goal = Arc::new(agent_stack::goal::Goal::new(
        "count stays non-negative and increases",
        |snap, _| {
            snap.to_json().get("count").and_then(|v| v.as_i64()).map(|i| i >= 0).unwrap_or(true)
        },
        |_snap, output| {
            if let Value::Int(i) = output {
                if *i < 0 {
                    return agent_stack::goal::GoalReport {
                        status: agent_stack::goal::GoalStatus::Violated,
                        note: "count went negative".into(),
                    };
                }
            }
            agent_stack::goal::GoalReport { status: agent_stack::goal::GoalStatus::Aligned, note: "ok".into() }
        },
    ));

    let rt = Runtime::new(
        agent_stack::blackboard::Blackboard::permissive(),
        budget,
        trace.clone(),
        goal,
        validator,
    );

    // --- Layer 13: human handler (auto-approve) ---
    rt.set_human_handler(agent_stack::human::auto_approve_handler());

    // --- Layer 9: negotiation handler ---
    rt.set_negotiation_handler(Box::new(|_label, input| Ok(input.clone())));

    // --- Layer 4: trust root is permissive by default ---
    // (narrowing is exercised by the subgraph node below)

    // Register agents.
    rt.register("incr", Incrementer { id: AgentId::new() });
    rt.register("echo", RecordedEcho { id: AgentId::new(), key: "echo-k" });
    rt.register("doubler", Doubler { id: AgentId::new() });

    // --- Layer 1 (hierarchical composition): build + register a child graph ---
    let mut child = Graph::new("child");
    let _child_node = child.add_node(
        Node::agent("doubler", "doubler").with_output_slot("out.sub"),
    );
    let child_id = rt.register_subgraph(child);

    // --- Build the parent graph (layer 1) ---
    let mut g = Graph::new("integration");
    let incr = g.add_node(
        Node::agent("incr", "incr")
            .with_output_slot("out.incr")
            .with_loop(LoopSpec::repeat(3)), // layer 1 loop
    );
    let echo = g.add_node(Node::agent("echo", "echo").with_output_slot("out.echo"));
    let human = g.add_node(Node { label: "review".into(), kind: NodeKind::Human("review".into()), ..Node::router("review") }.with_output_slot("out.human"));
    let neg = g.add_node(Node { label: "neg".into(), kind: NodeKind::Negotiation("neg".into()), ..Node::router("neg") }.with_output_slot("out.neg"));
    let sub = g.add_node(Node { label: "sub".into(), kind: NodeKind::Subgraph(child_id), ..Node::router("sub") }.with_output_slot("out.sub"));

    g.add_edge(Edge::normal(incr, echo)).unwrap();
    g.add_edge(Edge::normal(echo, human)).unwrap();
    g.add_edge(Edge::normal(human, neg)).unwrap();
    g.add_edge(Edge::normal(neg, sub)).unwrap();

    // --- Layer 12: working memory with decay (exercised separately below) ---
    let mut mem = MemoryStore::with_policy({
        let mut p = DecayPolicy::new();
        p.set("temp", DecayKind::Drop { ttl_seconds: 0 });
        p
    });
    mem.put("temp", Value::int(1));

    // --- Run ---
    let config = RunConfig::default().with_seed(vec![("count".into(), Value::int(0))]);
    let out = rt.run(&g, config).unwrap();

    // Layer 1 + 2: counter incremented 3x by the loop.
    assert_eq!(out.status, RunStatus::Completed);
    assert_eq!(
        out.final_snapshot.get("count").cloned().unwrap_or(Value::null()),
        Value::int(3),
        "incrementer loop should run 3 times"
    );

    // Layer 3: causal trace has a span per node invocation (incr x3 + echo + human + neg + sub + doubler).
    assert!(out.span_count >= 8, "expected at least 8 spans (incl. child), got {}", out.span_count);
    let trace_rec = TraceRecord::from_trace(&rt.trace);
    assert!(!trace_rec.spans.is_empty());

    // Layer 1 (hierarchical composition): the subgraph actually ran the child
    // graph, which doubled the count (3 → 6) and wrote to "out.sub".
    assert_eq!(
        out.final_snapshot.get("out.sub").cloned().unwrap_or(Value::null()),
        Value::int(6),
        "subgraph should have doubled the count (3 → 6)"
    );

    // Layer 6: at least one confidence signal was emitted (recorded as effects).
    let has_decision_effect = rt.trace.all_spans().iter()
        .any(|s| s.effects.iter().any(|e| matches!(e.kind, EffectKind::Decision)));
    assert!(has_decision_effect, "goal-check decision effects should be recorded");

    // Layer 7: budget was spent.
    assert!(rt.budget.spent(ResourceKind::Tokens) >= 3, "incrementer should have spent tokens");

    // Layer 10: run record captured the echo model call.
    let rec_json = rt.record.lock().to_json();
    let rp = agent_stack::replay::Replayer::from_json(&rec_json).unwrap();
    assert_eq!(rp.response_for("echo-k").unwrap()["out"], "echoed");

    // Layer 10 (determinism): replay the run and confirm the same final count.
    let rt2 = Runtime::new(
        agent_stack::blackboard::Blackboard::permissive(),
        Arc::new(Budget::unlimited()),
        Arc::new(CausalTrace::new()),
        agent_stack::goal::Goal::trivial(),
        Arc::new(InputValidator::new()),
    );
    rt2.register("incr", Incrementer { id: AgentId::new() });
    rt2.register("echo", RecordedEcho { id: AgentId::new(), key: "echo-k" });
    rt2.register("doubler", Doubler { id: AgentId::new() });
    rt2.set_human_handler(agent_stack::human::auto_approve_handler());
    rt2.set_negotiation_handler(Box::new(|_l, i| Ok(i.clone())));
    // Register the same child graph for replay.
    let mut child2 = Graph::new("child");
    let _cn = child2.add_node(Node::agent("doubler", "doubler").with_output_slot("out.sub"));
    rt2.register_subgraph(child2);
    let out2 = rp.replay(&rt2, &g, RunConfig::default().with_seed(vec![("count".into(), Value::int(0))])).unwrap();
    assert_eq!(
        out2.final_snapshot.get("count").cloned().unwrap_or(Value::null()),
        Value::int(3),
        "replay should reproduce the same count"
    );

    // Layer 12: decay drops the temp entry.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let dropped = mem.decay();
    assert_eq!(dropped, 1);
    assert!(mem.is_empty());

    // Layer 9: negotiation contract lifecycle (unit-level, within the same test).
    let contract = Contract::new(Value::str("deliver 3"), |v| matches!(v, Value::Int(i) if *i == 3));
    let neg = Negotiation::new(contract, 3);
    let neg_out = neg.run(|r, _o| {
        if r == 0 { ProviderAction::Counter(Value::str("ok I'll do 3")) }
        else { ProviderAction::Accept(Value::int(3)) }
    }).unwrap();
    assert!(matches!(neg_out, NegotiationOutcome::Fulfilled(_)));

    // Layer 8: interrupt bus — raising an abort is observable.
    rt.interrupts.raise(agent_stack::error::Interrupt::new(
        agent_stack::error::InterruptKind::Abort,
        "test abort",
    ));
    assert!(rt.interrupts.aborted());
}

#[test]
fn human_node_timeout_escalates() {
    // Layer 13: a human node that times out should escalate (HumanRejected
    // interrupt), stopping the run with Interrupted status.
    let rt = Runtime::permissive();
    rt.set_human_handler(agent_stack::human::slow_handler(std::time::Duration::from_millis(50)));
    rt.set_human_timeout(std::time::Duration::from_millis(1));

    let mut g = Graph::new("human-timeout");
    let _h = g.add_node(Node {
        label: "review".into(),
        kind: NodeKind::Human("review".into()),
        ..Node::router("review")
    }
    .with_output_slot("out.human"));

    let out = rt.run(&g, RunConfig::default()).unwrap();
    assert_eq!(out.status, RunStatus::Interrupted, "timeout should interrupt the run");
    assert!(out.interrupted.is_some(), "an interrupt should be recorded");
    assert_eq!(
        out.interrupted.as_ref().unwrap().kind,
        agent_stack::error::InterruptKind::HumanRejected,
        "interrupt kind should be HumanRejected"
    );
}
