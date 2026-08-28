//! The `Agent` trait — the unit of work in a graph node.
//!
//! An agent is anything that, given a context, produces an output. The
//! framework supplies deterministic mock agents and (behind the `llm` feature)
//! a real HTTP LLM agent. Users implement this trait for their own agents.
//!
//! `AgentContext` is the per-invocation view the runtime hands to an agent. It
//! exposes the layers the agent is allowed to touch: the blackboard (read +
//! scoped write), the trust context it inherited, the budget it may spend, the
//! goal it must stay aligned to, a channel for meta-cognitive signals, the
//! causal span it runs under, and the interrupt bus it can raise on.

use crate::blackboard::Blackboard;
use crate::budget::Budget;
use crate::error::AgentResult;
use crate::goal::Goal;
use crate::id::{AgentId, NodeId, SpanId};
use crate::interrupt::InterruptBus;
use crate::meta::MetaSignal;
use crate::record::RunRecord;
use crate::trust::TrustContext;
use crate::value::Value;
use parking_lot::Mutex;
use std::sync::Arc;

/// What an agent produced, plus signals the runtime routes on.
#[derive(Clone, Debug)]
pub struct AgentOutput {
    /// The primary result value (written to the blackboard by the runtime).
    pub value: Value,
    /// How confident the agent is in this output (layer 6).
    pub confidence: crate::meta::Confidence,
    /// Lifecycle status.
    pub status: AgentStatus,
    /// Optional message for the trace / logs.
    pub message: Option<String>,
}

impl AgentOutput {
    pub fn done(value: Value) -> Self {
        Self {
            value,
            confidence: crate::meta::Confidence::high(),
            status: AgentStatus::Done,
            message: None,
        }
    }

    pub fn done_with(value: Value, conf: crate::meta::Confidence) -> Self {
        Self {
            value,
            confidence: conf,
            status: AgentStatus::Done,
            message: None,
        }
    }

    pub fn escalate(reason: impl Into<String>) -> Self {
        Self {
            value: Value::null(),
            confidence: crate::meta::Confidence::low(),
            status: AgentStatus::Escalate,
            message: Some(reason.into()),
        }
    }
}

/// Lifecycle status of a single agent invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    /// Completed successfully.
    Done,
    /// Agent believes it is past its competence; wants escalation.
    Escalate,
    /// Agent wants to renegotiate its contract (layer 9).
    Renegotiate,
    /// Agent produced a partial result under budget pressure.
    Degraded,
}

/// Per-invocation context handed to an agent.
///
/// All shared services are behind `Arc`/`Mutex` so the context is cheap to
/// clone and threadsafe. An agent should treat the blackboard as its primary
/// read interface and its `output_slot` as its primary write target.
pub struct AgentContext {
    /// The node this invocation corresponds to.
    pub node: NodeId,
    /// The agent's own id.
    pub agent: AgentId,
    /// The causal span this invocation runs under (layer 3).
    pub span: SpanId,
    /// Shared blackboard (layer 2).
    pub blackboard: Arc<Blackboard>,
    /// The trust context inherited for this invocation (layer 4).
    pub trust: TrustContext,
    /// The budget tracker (layer 7).
    pub budget: Arc<Budget>,
    /// The goal the run is aligned to (layer 5).
    pub goal: Arc<Goal>,
    /// Channel for meta-cognitive signals (layer 6).
    pub meta: Arc<Mutex<Vec<MetaSignal>>>,
    /// Interrupt bus (layer 8).
    pub interrupts: Arc<InterruptBus>,
    /// Run record for deterministic replay (layer 10).
    pub record: Arc<Mutex<RunRecord>>,
    /// The blackboard slot the runtime will write this agent's output to.
    pub output_slot: crate::blackboard::SlotKey,
    /// Inputs gathered from incoming edges, merged into one value.
    pub input: Value,
}

impl AgentContext {
    /// Emit a meta-cognitive signal (layer 6).
    pub fn emit_meta(&self, sig: MetaSignal) {
        self.meta.lock().push(sig);
    }

    /// Record a model call so the run is replayable (layer 10).
    pub fn record_model_call(&self, call: crate::record::ModelCall) {
        self.record.lock().record_call(call);
    }

    /// Spend from the budget; returns an error if exhausted (layer 7).
    pub fn spend(&self, kind: crate::budget::ResourceKind, amount: u64) -> AgentResult<()> {
        self.budget.spend(kind, amount, self.span)
    }

    /// Raise an interrupt (layer 8).
    pub fn raise(&self, interrupt: crate::error::Interrupt) {
        self.interrupts.raise(interrupt.with_span(self.span).with_node(self.node));
    }
}

/// The trait every agent implements.
pub trait Agent: Send + Sync {
    /// Stable id.
    fn id(&self) -> AgentId;
    /// Human-readable name.
    fn name(&self) -> &str;
    /// Run the agent against the given context.
    fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput>;
}

/// A reference-counted, dynamic agent. `Arc` so the same agent can be reused
/// across loop iterations and multiple nodes without remove-and-reinsert.
pub type DynAgent = Arc<dyn Agent>;

/// Turn any `Agent` into a reference-counted one.
pub fn boxed<A: Agent + 'static>(a: A) -> DynAgent {
    Arc::new(a)
}

// Re-export Span so doc links resolve.
#[allow(unused_imports)]
use crate::causal::Span as _Span;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::AgentId;

    struct Echo;
    impl Agent for Echo {
        fn id(&self) -> AgentId {
            AgentId::new()
        }
        fn name(&self) -> &str {
            "echo"
        }
        fn run(&self, _ctx: &AgentContext) -> AgentResult<AgentOutput> {
            Ok(AgentOutput::done(Value::str("ok")))
        }
    }

    #[test]
    fn boxed_agent_runs() {
        let a = boxed(Echo);
        assert_eq!(a.name(), "echo");
    }
}
