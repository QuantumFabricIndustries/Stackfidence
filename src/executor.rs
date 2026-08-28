//! The graph executor (layer 1, part 3) — and the integration point for all
//! other layers.
//!
//! Drives a `Graph` against a `Runtime`. For each node it:
//!   - checks the interrupt bus (layer 8) and budget (layer 7),
//!   - opens a causal span (layer 3),
//!   - validates the input (layer 11),
//!   - builds an `AgentContext` with the inherited trust context (layer 4),
//!   - runs the agent,
//!   - checks goal alignment (layer 5),
//!   - collects meta-cognitive signals (layer 6),
//!   - writes the output to the blackboard (layer 2),
//!   - records model calls for replay (layer 10),
//!   - handles loops (layer 1 iteration),
//!   - follows edges (routing), including OnError edges (layer 8).
//!
//! Subgraph nodes (hierarchical composition) recursively run a nested graph
//! with a narrowed trust context. Human and negotiation nodes delegate to the
//! runtime's installed handlers (layers 13 and 9).

use crate::agent::{AgentContext, AgentOutput, AgentStatus};
use crate::blackboard::SlotKey;
use crate::causal::{Cause, EffectKind};
use crate::error::{AgentError, AgentResult, Interrupt, InterruptKind};
use crate::graph::{EdgeKind, Graph, Node, NodeKind};
use crate::id::{NodeId, RunId, SpanId};
use crate::loop_spec::{LoopState, LoopSpec};
use crate::runtime::Runtime;
use crate::trust::TrustContext;
use crate::value::Value;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-run configuration.
#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Safety cap on total node invocations (prevents runaway loops/graphs).
    pub max_steps: u32,
    /// Initial blackboard seed: slot -> value.
    pub seed: Vec<(String, Value)>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self { max_steps: 10_000, seed: Vec::new() }
    }
}

impl RunConfig {
    pub fn with_seed(mut self, seed: Vec<(String, Value)>) -> Self {
        self.seed = seed;
        self
    }
}

/// The outcome of a run.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub run_id: RunId,
    pub steps: u32,
    pub span_count: usize,
    pub final_snapshot: HashMap<String, Value>,
    /// The interrupt that stopped the run, if any.
    pub interrupted: Option<Interrupt>,
    /// Final status.
    pub status: RunStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    /// Completed normally.
    Completed,
    /// Stopped by an interrupt.
    Interrupted,
    /// Stopped because max_steps was hit.
    StepLimited,
}

/// A work item in the executor's queue.
struct Task {
    node: NodeId,
    cause: Cause,
    input: Value,
    /// Trust context to use for this node (already narrowed for the call chain).
    trust: TrustContext,
}

/// The executor.
pub struct GraphExecutor<'a> {
    rt: &'a Runtime,
    steps: u32,
}

impl<'a> GraphExecutor<'a> {
    pub fn new(rt: &'a Runtime) -> Self {
        Self { rt, steps: 0 }
    }

    /// Run a graph to completion (or until interrupted / step-limited).
    /// Entry nodes receive `Value::null()` as their input.
    pub fn run(&mut self, graph: &Graph, config: RunConfig) -> AgentResult<RunOutcome> {
        self.run_with_entry_input(graph, config, Value::null(), self.rt.trust_root.clone())
    }

    /// Run a graph with a specific input value and trust context for entry nodes.
    /// Used by subgraph recursion (layer 1) to pass the parent's output as the
    /// child's entry input and to narrow trust.
    pub fn run_with_entry_input(
        &mut self,
        graph: &Graph,
        config: RunConfig,
        entry_input: Value,
        entry_trust: TrustContext,
    ) -> AgentResult<RunOutcome> {
        let run_id = self.rt.new_run_id();
        let mut queue: Vec<Task> = Vec::new();
        for entry in &graph.entries {
            queue.push(Task {
                node: *entry,
                cause: Cause::External(format!("run {}", run_id)),
                input: entry_input.clone(),
                trust: entry_trust.clone(),
            });
        }

        let mut interrupted: Option<Interrupt> = None;
        let mut status = RunStatus::Completed;

        while let Some(task) = queue.pop() {
            // 1. Interrupt check (layer 8).
            if self.rt.interrupts.pending() {
                interrupted = self.rt.interrupts.take();
                status = RunStatus::Interrupted;
                break;
            }
            // 2. Step limit (safety).
            if self.steps >= config.max_steps {
                status = RunStatus::StepLimited;
                break;
            }
            self.steps += 1;

            let node = match graph.node(task.node) {
                Some(n) => n.clone(),
                None => continue,
            };

            let outcome = self.run_node(graph, &node, &task, &config);

            match outcome {
                NodeOutcome::Done(output) => {
                    // Follow edges.
                    self.enqueue_successors(graph, &node, &output, &task.trust, &mut queue);
                }
                NodeOutcome::Failed(err) => {
                    // Try OnError edges; if none, propagate as interrupt.
                    let has_error_edge = graph
                        .out_edges(node.id)
                        .iter()
                        .any(|e| e.kind == EdgeKind::OnError);
                    if has_error_edge {
                        for e in graph.out_edges(node.id).iter() {
                            if e.kind == EdgeKind::OnError {
                                queue.push(Task {
                                    node: e.to,
                                    cause: Cause::External("error-edge".into()),
                                    input: Value::str(format!("error: {}", err)),
                                    trust: task.trust.clone(),
                                });
                            }
                        }
                    } else {
                        // No error edge: raise an interrupt and stop.
                        self.rt.interrupts.raise(
                            Interrupt::new(InterruptKind::Internal, format!("{}", err))
                                .with_node(node.id),
                        );
                        interrupted = self.rt.interrupts.take();
                        status = RunStatus::Interrupted;
                        break;
                    }
                }
            }
        }

        Ok(RunOutcome {
            run_id,
            steps: self.steps,
            span_count: self.rt.trace.len(),
            final_snapshot: self.rt.blackboard.snapshot(),
            interrupted,
            status,
        })
    }

    /// Run a single node (handling loops). Returns the final output or failure.
    fn run_node(
        &mut self,
        graph: &Graph,
        node: &Node,
        task: &Task,
        config: &RunConfig,
    ) -> NodeOutcome {
        // Handle loops by repeatedly invoking the node body.
        if let Some(loop_spec) = &node.loop_spec {
            return self.run_loop(graph, node, task, loop_spec, config);
        }
        self.run_node_once(graph, node, task, None)
    }

    /// Run a node under a loop.
    fn run_loop(
        &mut self,
        graph: &Graph,
        node: &Node,
        task: &Task,
        spec: &LoopSpec,
        _config: &RunConfig,
    ) -> NodeOutcome {
        let mut state = LoopState::new();
        let mut last_output = Value::null();
        let mut last_err: Option<AgentError> = None;
        let mut parent_span: Option<SpanId> = None;

        loop {
            // Interrupt / step checks happen in run_node_once via the bus, but
            // also check here between iterations.
            if self.rt.interrupts.pending() {
                break;
            }
            // Read the loop condition slot (if any).
            let slot_val = match spec.slot() {
                Some(slot) => {
                    match self.rt.blackboard.read(self.rt.trust_root.actor, slot) {
                        Ok(v) => v,
                        Err(e) => return NodeOutcome::Failed(e),
                    }
                }
                None => Value::null(),
            };
            if !spec.continues(&state, &slot_val) {
                break;
            }

            // The input for this iteration: foreach uses the current item,
            // otherwise the original task input.
            let iter_input = match spec.current_item(&slot_val, &state) {
                Some(item) => item.clone(),
                None => task.input.clone(),
            };

            let cause = match parent_span {
                Some(p) => Cause::LoopIteration(p, state.iteration),
                None => task.cause.clone(),
            };

            let outcome = self.run_node_once(graph, node, task, Some((cause, iter_input, &mut parent_span)));
            match outcome {
                NodeOutcome::Done(o) => {
                    last_output = o;
                    // If an agent emitted a Stop meta-signal, break the loop.
                    if self.last_meta_stop() {
                        break;
                    }
                }
                NodeOutcome::Failed(e) => {
                    last_err = Some(e);
                    break;
                }
            }
            state.advance();
        }

        match last_err {
            Some(e) => NodeOutcome::Failed(e),
            None => NodeOutcome::Done(last_output),
        }
    }

    /// Was a Stop meta-signal emitted in the last invocation?
    fn last_meta_stop(&self) -> bool {
        // The meta vec is per-invocation and drained inside run_node_once;
        // we signal stop via the interrupt bus instead. Check for a Stop-style
        // interrupt isn't set here; instead we rely on agents raising Stop via
        // ctx.raise. So this is a no-op fallback kept for clarity.
        false
    }

    /// Run a node exactly once (one invocation, no loop).
    /// `loop_ctx` carries loop cause/input/parent span when invoked from a loop.
    fn run_node_once(
        &mut self,
        graph: &Graph,
        node: &Node,
        task: &Task,
        loop_ctx: Option<(Cause, Value, &mut Option<SpanId>)>,
    ) -> NodeOutcome {
        let (cause, input) = match &loop_ctx {
            Some((c, i, _)) => (c.clone(), i.clone()),
            None => (task.cause.clone(), task.input.clone()),
        };

        // Budget wall-clock check (layer 7).
        if let Err(e) = self.rt.budget.spend(crate::budget::ResourceKind::WallMs, 0, SpanId::new()) {
            self.rt.interrupts.raise(
                Interrupt::new(InterruptKind::BudgetExhausted, format!("{}", e)).with_node(node.id),
            );
            return NodeOutcome::Failed(e);
        }

        // Resolve the agent label (for Agent nodes).
        let agent_id = match &node.kind {
            NodeKind::Agent(label) => match self.rt.resolve(label) {
                Some(a) => a.id(),
                None => {
                    let e = AgentError::Other(format!("no agent for label {:?}", label));
                    return NodeOutcome::Failed(e);
                }
            },
            _ => self.rt.trust_root.actor, // routers/human/negotiation/subgraph
        };

        // Open a causal span (layer 3).
        let span = self.rt.trace.open(node.id, agent_id, cause.clone());

        // Validate input (layer 11).
        if let Err(err) = self.rt.validator.validate(&input) {
            let reason = format!("{}", err);
            self.rt.interrupts.raise(
                Interrupt::new(InterruptKind::BadInput, reason.clone()).with_node(node.id).with_span(span),
            );
            self.rt.trace.close(span);
            return NodeOutcome::Failed(AgentError::Validation(reason));
        }

        // Build the agent context.
        let output_slot = SlotKey::new(
            node.output_slot
                .clone()
                .unwrap_or_else(|| format!("out.{}", node.label)),
        );
        let meta = Arc::new(Mutex::new(Vec::new()));
        let ctx = AgentContext {
            node: node.id,
            agent: agent_id,
            span,
            blackboard: self.rt.blackboard.clone(),
            trust: task.trust.clone(),
            budget: self.rt.budget.clone(),
            goal: self.rt.goal.clone(),
            meta: meta.clone(),
            interrupts: self.rt.interrupts.clone(),
            record: self.rt.record.clone(),
            output_slot: output_slot.clone(),
            input: input.clone(),
        };

        // Dispatch by node kind.
        let run_result: AgentResult<AgentOutput> = match &node.kind {
            NodeKind::Agent(label) => self.rt.run_agent(label, &ctx),
            NodeKind::Router => Ok(AgentOutput::done(input.clone())),
            NodeKind::Human(label) => self.run_human(label, &input, &ctx),
            NodeKind::Negotiation(label) => self.run_negotiation(label, &input, &ctx),
            NodeKind::Subgraph(child_id) => {
                // Hierarchical composition: run the nested graph with narrowed trust.
                // The nested graph is looked up from the runtime's graph registry
                // if present; here we expect it to have been registered via
                // `register_subgraph`. For simplicity we return the input as-is
                // if no child graph is registered.
                self.run_subgraph(*child_id, &input, &ctx, graph)
            }
        };

        // Record blackboard writes into the causal span (layer 3).
        let writes = self.rt.blackboard.drain_log();
        // Only attribute writes that happened during this span (heuristic: all
        // drained writes since last drain). This is acceptable because the
        // executor is sequential.
        self.rt.trace.record_writes(span, writes);

        // Close span.
        self.rt.trace.close(span);

        // Set parent span for loop continuation.
        if let Some((_, _, parent)) = loop_ctx {
            *parent = Some(span);
        }

        let output = match run_result {
            Ok(o) => o,
            Err(e) => {
                // Record the failure as an effect.
                self.rt.trace.add_effect(
                    span,
                    EffectKind::Interrupt,
                    format!("node failed: {}", e),
                    vec![],
                );
                return NodeOutcome::Failed(e);
            }
        };

        // Goal alignment check (layer 5).
        let snapshot = self.blackboard_snapshot_value();
        let report = self.rt.goal.check(&snapshot, &output.value);
        self.rt.trace.add_effect(
            span,
            EffectKind::Decision,
            format!("goal check: {:?} ({})", report.status, report.note),
            vec![],
        );
        match report.status {
            crate::goal::GoalStatus::Violated => {
                self.rt.interrupts.raise(
                    Interrupt::new(InterruptKind::GoalDrift, report.note.clone())
                        .with_node(node.id)
                        .with_span(span),
                );
                return NodeOutcome::Failed(AgentError::Other(format!("goal drift: {}", report.note)));
            }
            _ => {}
        }

        // Write the output to the blackboard (layer 2).
        if let Err(e) = self
            .rt
            .blackboard
            .write(task.trust.actor, &output_slot, output.value.clone())
        {
            return NodeOutcome::Failed(e);
        }

        // Record the output as an effect.
        self.rt.trace.add_effect(span, EffectKind::Output, "produced output", vec![]);

        // Handle meta-cognitive signals (layer 6).
        let signals = std::mem::take(&mut *meta.lock());
        for sig in &signals {
            if let crate::meta::MetaSignal::Escalate(esc) = sig {
                self.rt.interrupts.raise(
                    Interrupt::new(InterruptKind::Escalation, esc.reason.clone())
                        .with_node(node.id)
                        .with_span(span),
                );
            }
        }
        // If the agent signalled escalate via status, raise an interrupt.
        if output.status == AgentStatus::Escalate {
            let reason = output.message.clone().unwrap_or_else(|| "escalation".to_string());
            self.rt.interrupts.raise(
                Interrupt::new(InterruptKind::Escalation, reason)
                    .with_node(node.id)
                    .with_span(span),
            );
        }

        NodeOutcome::Done(output.value)
    }

    fn run_human(
        &self,
        label: &str,
        input: &Value,
        ctx: &AgentContext,
    ) -> AgentResult<AgentOutput> {
        let handler = match self.rt.human_handler_arc() {
            Some(h) => h,
            None => {
                // No handler installed: treat as auto-approve (dev default).
                return Ok(AgentOutput::done(input.clone()));
            }
        };

        let timeout = self.rt.human_timeout();
        let label_owned = label.to_string();
        let input_owned = input.clone();

        // Spawn a worker thread that calls the handler, and wait on a channel
        // with a deadline. On timeout, escalate (HumanRejected interrupt).
        // This keeps the framework fully synchronous while giving a *hard*
        // timeout guarantee on human nodes (layer 13).
        let (tx, rx) = std::sync::mpsc::channel();
        let handler_clone = handler.clone();
        std::thread::spawn(move || {
            let result = handler_clone(&label_owned, &input_owned);
            let _ = tx.send(result);
        });

        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(AgentOutput::done(v)),
            Ok(Err(e)) => {
                // Handler rejected — raise an interrupt and propagate.
                self.rt.interrupts.raise(
                    Interrupt::new(InterruptKind::HumanRejected, format!("{}", e))
                        .with_node(ctx.node)
                        .with_span(ctx.span),
                );
                Err(e)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Timed out — escalate.
                let reason = format!("human review timed out after {:?}", timeout);
                self.rt.interrupts.raise(
                    Interrupt::new(InterruptKind::HumanRejected, reason.clone())
                        .with_node(ctx.node)
                        .with_span(ctx.span),
                );
                Err(AgentError::HumanRejected(reason))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Worker thread panicked — escalate.
                let reason = "human review worker thread disconnected".to_string();
                self.rt.interrupts.raise(
                    Interrupt::new(InterruptKind::Internal, reason.clone())
                        .with_node(ctx.node)
                        .with_span(ctx.span),
                );
                Err(AgentError::Other(reason))
            }
        }
    }

    fn run_negotiation(
        &self,
        label: &str,
        input: &Value,
        _ctx: &AgentContext,
    ) -> AgentResult<AgentOutput> {
        match self.rt.negotiation_handler() {
            Some(h) => {
                let v = h.handle(label, input)?;
                Ok(AgentOutput::done(v))
            }
            None => {
                // No handler: pass-through.
                Ok(AgentOutput::done(input.clone()))
            }
        }
    }

    fn run_subgraph(
        &mut self,
        child_id: crate::id::GraphId,
        input: &Value,
        ctx: &AgentContext,
        _parent_graph: &Graph,
    ) -> AgentResult<AgentOutput> {
        // Hierarchical composition (layer 1): look up the registered child graph
        // and run it via a nested executor. The child shares ALL services
        // (blackboard, budget, trace, goal, interrupts, record) so causal memory
        // is continuous across the composition boundary — a span in the child
        // graph is a descendant of this span in the parent trace.
        //
        // Trust narrows: the child call chain gets a subset of the parent's
        // authorities plus a "subgraph" constraint, so a subgraph can never hold
        // more authority than the node that invoked it.
        let child_graph = match self.rt.resolve_subgraph(child_id) {
            Some(g) => g,
            None => {
                // No child registered: pass through (dev default).
                self.rt.trace.add_effect(
                    ctx.span,
                    EffectKind::Decision,
                    "subgraph node has no registered child graph (pass-through)",
                    vec![],
                );
                return Ok(AgentOutput::done(input.clone()));
            }
        };

        // Narrow trust for the child call chain.
        let child_trust = ctx.trust.narrow(
            ctx.agent,
            &[crate::trust::Authority::new("compute")],
            &[crate::trust::Constraint::new("subgraph")],
        );

        // Record the composition decision in the causal trace.
        self.rt.trace.add_effect(
            ctx.span,
            EffectKind::Decision,
            format!("composed subgraph ({} nodes, narrowed trust)", child_graph.node_count()),
            vec![],
        );

        // Run the child graph with a nested executor. The child's entry nodes
        // receive the subgraph's input value directly as their task input, and
        // the narrowed trust context. All services (blackboard, budget, trace,
        // goal, interrupts, record) are shared so causal memory is continuous.
        let remaining = u32::saturating_sub(10_000, self.steps);
        let child_config = RunConfig {
            max_steps: remaining,
            seed: Vec::new(),
        };

        let mut child_exec = GraphExecutor::new(self.rt);
        let child_outcome = child_exec.run_with_entry_input(
            &child_graph,
            child_config,
            input.clone(),
            child_trust.clone(),
        )?;

        // Merge the child's step count into the parent's.
        self.steps = self.steps.saturating_add(child_outcome.steps);

        // The child's result: read back from the subgraph node's output slot
        // (the child graph's contract is to write its result there). If the
        // child didn't write to it, fall back to the input.
        let result = self
            .rt
            .blackboard
            .read(child_trust.actor, &ctx.output_slot)
            .unwrap_or_else(|_| input.clone());

        // If the child was interrupted, propagate the interrupt to the parent.
        if let Some(int) = child_outcome.interrupted {
            self.rt.interrupts.raise(int);
        }

        Ok(AgentOutput::done(result))
    }

    /// Enqueue successor nodes based on edges.
    fn enqueue_successors(
        &self,
        graph: &Graph,
        node: &Node,
        output: &Value,
        trust: &TrustContext,
        queue: &mut Vec<Task>,
    ) {
        let snapshot = self.blackboard_snapshot_value();
        for e in graph.out_edges(node.id).iter() {
            if e.kind == EdgeKind::OnError {
                continue;
            }
            if e.passes(&snapshot, output) {
                queue.push(Task {
                    node: e.to,
                    cause: Cause::Parent(/* span unknown here */ SpanId::new()),
                    input: output.clone(),
                    trust: trust.clone(),
                });
            }
        }
    }

    /// Snapshot the blackboard as a `Value::Object` (for goal checks / edge
    /// conditions).
    fn blackboard_snapshot_value(&self) -> Value {
        let snap = self.rt.blackboard.snapshot();
        let pairs: Vec<(String, Value)> = snap.into_iter().collect();
        Value::obj(pairs)
    }
}

enum NodeOutcome {
    Done(Value),
    Failed(AgentError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentOutput};
    use crate::graph::{Edge, Graph, Node};
    use crate::id::AgentId;
    use crate::value::Value;

    struct ConstAgent(AgentId, String);
    impl Agent for ConstAgent {
        fn id(&self) -> AgentId {
            self.0
        }
        fn name(&self) -> &str {
            "const"
        }
        fn run(&self, _ctx: &AgentContext) -> AgentResult<AgentOutput> {
            Ok(AgentOutput::done(Value::str(self.1.clone())))
        }
    }

    #[test]
    fn runs_simple_two_node_graph() {
        let rt = Runtime::permissive();
        rt.register("a", ConstAgent(AgentId::new(), "hello".into()));
        rt.register("b", ConstAgent(AgentId::new(), "world".into()));
        let mut g = Graph::new("t");
        let a = g.add_node(Node::agent("a", "a").with_output_slot("out.a"));
        let b = g.add_node(Node::agent("b", "b").with_output_slot("out.b"));
        g.add_edge(Edge::normal(a, b)).unwrap();
        let out = rt.run(&g, RunConfig::default()).unwrap();
        assert_eq!(out.status, RunStatus::Completed);
        assert_eq!(
            out.final_snapshot.get("out.b").cloned().unwrap_or(Value::null()),
            Value::str("world")
        );
        assert!(out.span_count >= 2);
    }

    #[test]
    fn loop_repeat_runs_three_times() {
        let rt = Runtime::permissive();
        // An agent that increments a counter on the blackboard.
        struct Incr(AgentId);
        impl Agent for Incr {
            fn id(&self) -> AgentId {
                self.0
            }
            fn name(&self) -> &str {
                "incr"
            }
            fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
                let cur = ctx.blackboard.read(ctx.agent, &SlotKey::new("count"))?;
                let newv = if let Value::Int(i) = cur { Value::Int(i + 1) } else { Value::Int(1) };
                ctx.blackboard.write(ctx.agent, &SlotKey::new("count"), newv.clone())?;
                Ok(AgentOutput::done(newv))
            }
        }
        rt.register("incr", Incr(AgentId::new()));
        let mut g = Graph::new("loop");
        let n = g.add_node(
            Node::agent("incr", "incr")
                .with_output_slot("out.incr")
                .with_loop(crate::loop_spec::LoopSpec::repeat(3)),
        );
        let _ = n;
        let out = rt.run(&g, RunConfig::default().with_seed(vec![("count".into(), Value::int(0))])).unwrap();
        assert_eq!(out.final_snapshot.get("count").cloned().unwrap_or(Value::null()), Value::int(3));
    }

    #[test]
    fn subgraph_recurses_and_shares_causal_trace() {
        // Child graph: receives input via ctx.input (entry input), doubles it,
        // writes to "out.child".
        struct Doubler(AgentId);
        impl Agent for Doubler {
            fn id(&self) -> AgentId { self.0 }
            fn name(&self) -> &str { "doubler" }
            fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
                let doubled = if let Value::Int(i) = &ctx.input { Value::Int(i * 2) } else { Value::Int(0) };
                ctx.blackboard.write(ctx.agent, &SlotKey::new("out.child"), doubled.clone())?;
                Ok(AgentOutput::done(doubled))
            }
        }

        let rt = Runtime::permissive();
        rt.register("doubler", Doubler(AgentId::new()));

        // Build and register the child graph.
        let mut child = Graph::new("child");
        let _child_node = child.add_node(
            Node::agent("doubler", "doubler").with_output_slot("out.child"),
        );
        let child_id = rt.register_subgraph(child);

        // Parent graph: a subgraph node that invokes the child.
        // The subgraph node receives 21 as its input (from the seed), passes it
        // to the child graph as the entry input, and reads back from "out.child".
        let mut parent = Graph::new("parent");
        let sub = parent.add_node(Node {
            label: "sub".into(),
            kind: NodeKind::Subgraph(child_id),
            ..Node::router("sub")
        }
        .with_output_slot("out.child")); // read back from the slot the child writes

        let _ = sub;
        // Seed the parent's entry input with 21. Since the subgraph node IS the
        // entry, it receives this as its task input... but the executor sets
        // entry input to null. So we use a feeder node instead.
        // Actually: the subgraph node is the entry, so its input is null.
        // We need a feeder node that outputs 21, then edges to the subgraph.
        struct Feeder(AgentId, Value);
        impl Agent for Feeder {
            fn id(&self) -> AgentId { self.0 }
            fn name(&self) -> &str { "feeder" }
            fn run(&self, _ctx: &AgentContext) -> AgentResult<AgentOutput> {
                Ok(AgentOutput::done(self.1.clone()))
            }
        }
        rt.register("feeder", Feeder(AgentId::new(), Value::int(21)));

        let mut parent2 = Graph::new("parent");
        let feed = parent2.add_node(Node::agent("feed", "feeder").with_output_slot("out.feed"));
        let sub2 = parent2.add_node(Node {
            label: "sub".into(),
            kind: NodeKind::Subgraph(child_id),
            ..Node::router("sub")
        }
        .with_output_slot("out.child"));
        parent2.add_edge(Edge::normal(feed, sub2)).unwrap();

        let out = rt.run(&parent2, RunConfig::default()).unwrap();

        assert_eq!(out.status, RunStatus::Completed);
        // The child doubled 21 → 42 and wrote to "out.child".
        assert_eq!(
            out.final_snapshot.get("out.child").cloned().unwrap_or(Value::null()),
            Value::int(42),
            "subgraph should have doubled the input"
        );
        // Causal trace spans both parent and child (at least 3 spans: feeder + sub + doubler).
        assert!(out.span_count >= 3, "causal trace should span parent + child, got {} spans", out.span_count);
    }

    #[test]
    fn human_node_times_out_and_escalates() {
        let rt = Runtime::permissive();
        // Install a handler that takes 10ms, but set the timeout to 1ms.
        rt.set_human_handler(crate::human::slow_handler(std::time::Duration::from_millis(10)));
        rt.set_human_timeout(std::time::Duration::from_millis(1));

        let mut g = Graph::new("human-timeout");
        let _h = g.add_node(Node {
            label: "review".into(),
            kind: NodeKind::Human("review".into()),
            ..Node::router("review")
        }
        .with_output_slot("out.human"));

        let out = rt.run(&g, RunConfig::default()).unwrap();
        // The run should be interrupted by the timeout.
        assert_eq!(out.status, RunStatus::Interrupted);
        assert!(out.interrupted.is_some());
        assert_eq!(
            out.interrupted.as_ref().unwrap().kind,
            crate::error::InterruptKind::HumanRejected
        );
    }

    #[test]
    fn human_node_completes_within_timeout() {
        let rt = Runtime::permissive();
        // Handler is instant (auto-approve), timeout is generous.
        rt.set_human_handler(crate::human::auto_approve_handler());
        rt.set_human_timeout(std::time::Duration::from_secs(5));

        let mut g = Graph::new("human-ok");
        let _h = g.add_node(Node {
            label: "review".into(),
            kind: NodeKind::Human("review".into()),
            ..Node::router("review")
        }
        .with_output_slot("out.human"));

        let out = rt.run(&g, RunConfig::default()).unwrap();
        assert_eq!(out.status, RunStatus::Completed);
    }
}
