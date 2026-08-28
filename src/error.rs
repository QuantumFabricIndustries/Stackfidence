//! Errors and the `Interrupt` family (layer 8).
//!
//! `AgentError` is the framework's general error type. `Interrupt` is the
//! structured, first-class failure that short-circuits the normal graph flow
//! and carries context upward — what state was read, what failed, and why.
//! Interrupts are distinct from plain errors: an error is "this node failed";
//! an interrupt is "the whole flow must stop *now*, and here is the structured
//! reason plus the causal span that triggered it".

use crate::id::{NodeId, SpanId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// General framework error.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("graph error: {0}")]
    Graph(String),
    #[error("blackboard error: {0}")]
    Blackboard(String),
    #[error("budget exhausted: {0}")]
    Budget(String),
    #[error("trust violation: {0}")]
    Trust(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("interrupt: {kind:?} ({reason})")]
    Interrupt {
        kind: InterruptKind,
        reason: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("negotiation failed: {0}")]
    Negotiation(String),
    #[error("human review rejected: {0}")]
    HumanRejected(String),
    #[error("other: {0}")]
    Other(String),
}

impl From<Interrupt> for AgentError {
    fn from(i: Interrupt) -> Self {
        AgentError::Interrupt {
            kind: i.kind,
            reason: i.reason,
        }
    }
}

/// Kinds of structured interrupts. These map to the categories of "things that
/// should short-circuit the normal flow" rather than just retrying locally.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterruptKind {
    /// A node exceeded its budget and cannot continue.
    BudgetExhausted,
    /// A trust/policy violation occurred mid-flow.
    TrustViolation,
    /// A human reviewer rejected the work.
    HumanRejected,
    /// An agent signalled it is operating past its competence and escalated.
    Escalation,
    /// Input validation at a boundary failed in a way that can't be retried.
    BadInput,
    /// A goal-invariant check failed — the run is no longer aligned.
    GoalDrift,
    /// An explicit abort was requested.
    Abort,
    /// A panic-like internal failure.
    Internal,
}

/// A structured interrupt carrying the context needed to explain *why* the flow
/// stopped and *where* it stopped.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interrupt {
    pub kind: InterruptKind,
    pub reason: String,
    /// The node that raised the interrupt, if known.
    pub node: Option<NodeId>,
    /// The causal span that was active when the interrupt fired.
    pub span: Option<SpanId>,
    /// Arbitrary structured context (state read, decision made, etc.).
    pub context: serde_json::Value,
}

impl Interrupt {
    pub fn new(kind: InterruptKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
            node: None,
            span: None,
            context: serde_json::Value::Null,
        }
    }

    pub fn with_node(mut self, n: NodeId) -> Self {
        self.node = Some(n);
        self
    }

    pub fn with_span(mut self, s: SpanId) -> Self {
        self.span = Some(s);
        self
    }

    pub fn with_context(mut self, ctx: serde_json::Value) -> Self {
        self.context = ctx;
        self
    }
}

impl std::fmt::Display for Interrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.reason)
    }
}

/// Convenience result alias used throughout the framework.
pub type AgentResult<T> = Result<T, AgentError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_carries_context() {
        let i = Interrupt::new(InterruptKind::BudgetExhausted, "tokens used up")
            .with_context(serde_json::json!({"used": 1000, "limit": 1000}));
        assert_eq!(i.kind, InterruptKind::BudgetExhausted);
        assert_eq!(i.context["used"], 1000);
    }

    #[test]
    fn interrupt_to_error() {
        let i = Interrupt::new(InterruptKind::GoalDrift, "output diverged");
        let e: AgentError = i.into();
        assert!(matches!(e, AgentError::Interrupt { .. }));
    }
}
