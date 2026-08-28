//! Meta-cognition (layer 6).
//!
//! Agents don't natively know how confident they are, whether they're the right
//! agent for the task, or when to stop and escalate. Here agents emit
//! `MetaSignal`s (confidence scores, uncertainty, escalation requests) that the
//! graph can route on. This changes the whole reliability profile: the graph
//! can branch to a stronger agent, request human review, or abort — instead of
//! running confidently into garbage.

use crate::id::{AgentId, SpanId};

/// A confidence score in [0.0, 1.0].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Confidence(pub f64);

impl Confidence {
    pub fn new(v: f64) -> Self {
        Self(v.clamp(0.0, 1.0))
    }
    pub fn high() -> Self {
        Self(0.9)
    }
    pub fn medium() -> Self {
        Self(0.5)
    }
    pub fn low() -> Self {
        Self(0.1)
    }
    pub fn value(&self) -> f64 {
        self.0
    }
    /// Below this an agent is considered uncertain.
    pub const UNCERTAIN_THRESHOLD: f64 = 0.4;
    pub fn is_uncertain(&self) -> bool {
        self.0 < Self::UNCERTAIN_THRESHOLD
    }
}

/// A meta-cognitive signal emitted by an agent.
#[derive(Clone, Debug)]
pub enum MetaSignal {
    /// Confidence in the last output.
    Confidence(SpanId, AgentId, Confidence),
    /// Agent believes it's the wrong agent for this input.
    WrongAgent(SpanId, AgentId, String),
    /// Agent wants to escalate to a stronger agent or human.
    Escalate(EscalationSignal),
    /// Agent wants to stop the loop / run.
    Stop(SpanId, AgentId, String),
}

/// An escalation request.
#[derive(Clone, Debug)]
pub struct EscalationSignal {
    pub span: SpanId,
    pub agent: AgentId,
    pub reason: String,
    /// Suggested next step: a node id label or "human".
    pub suggest: String,
}

impl MetaSignal {
    /// Does this signal indicate the agent is uncertain / failing?
    pub fn is_negative(&self) -> bool {
        match self {
            MetaSignal::Confidence(_, _, c) => c.is_uncertain(),
            MetaSignal::WrongAgent(_, _, _) => true,
            MetaSignal::Escalate(_) => true,
            MetaSignal::Stop(_, _, _) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_clamps() {
        assert_eq!(Confidence::new(5.0).value(), 1.0);
        assert_eq!(Confidence::new(-1.0).value(), 0.0);
    }

    #[test]
    fn uncertain_threshold() {
        assert!(Confidence::low().is_uncertain());
        assert!(!Confidence::high().is_uncertain());
    }

    #[test]
    fn negative_signals() {
        let s = SpanId::new();
        let a = AgentId::new();
        assert!(MetaSignal::Stop(s, a, "x".into()).is_negative());
        assert!(!MetaSignal::Confidence(s, a, Confidence::high()).is_negative());
    }
}
