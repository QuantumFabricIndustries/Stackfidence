//! Human-in-the-loop as a first-class node type (layer 13).
//!
//! Most frameworks treat human review as an exception or a bolt-on. Here it's a
//! node type with the same contract as any other: it produces a `Value`, can
//! time out, can escalate, and hands back into the graph. The runtime installs a
//! `HumanHandler` (see runtime.rs); this module provides helpers for building
//! handlers and the structured handoff record.
//!
//! A `HumanNode` in a graph is just `NodeKind::Human(label)`. The handler
//! receives the label + the input value and returns the reviewed value, or an
//! error (which the executor raises as a `HumanRejected` interrupt).

use crate::error::{AgentError, AgentResult};
use crate::value::Value;
use std::sync::Arc;
use std::time::Duration;

/// A structured handoff describing what the human is being asked.
#[derive(Clone, Debug)]
pub struct HumanHandoff {
    /// What kind of review is requested.
    pub kind: ReviewKind,
    /// The input the human should look at.
    pub input: Value,
    /// A human-readable prompt.
    pub prompt: String,
    /// How long to wait before auto-escalating.
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewKind {
    /// Approve / reject a proposed action.
    Approve,
    /// Choose between options.
    Choose,
    /// Free-form input.
    Freeform,
    /// Senior review required.
    Senior,
}

/// The human's decision.
#[derive(Clone, Debug)]
pub enum HumanDecision {
    Approved(Value),
    Rejected(String),
    /// Timed out waiting for a human; escalate.
    TimedOut,
}

impl HumanDecision {
    pub fn into_result(self) -> AgentResult<Value> {
        match self {
            HumanDecision::Approved(v) => Ok(v),
            HumanDecision::Rejected(reason) => Err(AgentError::HumanRejected(reason)),
            HumanDecision::TimedOut => Err(AgentError::HumanRejected("human review timed out".into())),
        }
    }
}

/// A handler that auto-approves everything (dev/test default).
pub fn auto_approve_handler() -> Arc<dyn Fn(&str, &Value) -> AgentResult<Value> + Send + Sync> {
    Arc::new(|_label, input: &Value| Ok(input.clone()))
}

/// A handler that always rejects with a given reason.
pub fn always_reject_handler(
    reason: impl Into<String> + Send + Sync + 'static,
) -> Arc<dyn Fn(&str, &Value) -> AgentResult<Value> + Send + Sync> {
    let reason = reason.into();
    Arc::new(move |_label, _input| Err(AgentError::HumanRejected(reason.clone())))
}

/// A handler that maps a choice index in the input to one of the provided
/// options. Expects input to be `Object{choice:Int,options:List}`.
pub fn choice_handler(
    options: Vec<Value>,
) -> Arc<dyn Fn(&str, &Value) -> AgentResult<Value> + Send + Sync> {
    Arc::new(move |_label, input| {
        if let Value::Object(fields) = input {
            let choice = fields.iter().find(|(k, _)| k == "choice").and_then(|(_, v)| {
                if let Value::Int(i) = v { Some(*i) } else { None }
            });
            if let Some(idx) = choice {
                if idx >= 0 && (idx as usize) < options.len() {
                    return Ok(options[idx as usize].clone());
                }
            }
        }
        Err(AgentError::HumanRejected("invalid choice".into()))
    })
}

/// A handler that sleeps for a given duration before approving. Used to test
/// timeout behavior.
pub fn slow_handler(
    delay: Duration,
) -> Arc<dyn Fn(&str, &Value) -> AgentResult<Value> + Send + Sync> {
    Arc::new(move |_label, input| {
        std::thread::sleep(delay);
        Ok(input.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_passes_input() {
        let h = auto_approve_handler();
        let v = h("review", &Value::int(7)).unwrap();
        assert_eq!(v, Value::int(7));
    }

    #[test]
    fn always_reject_handler_rejects() {
        let h = always_reject_handler("nope");
        let err = h("review", &Value::null()).unwrap_err();
        assert!(matches!(err, AgentError::HumanRejected(_)));
    }

    #[test]
    fn choice_handler_picks_option() {
        let h = choice_handler(vec![Value::str("a"), Value::str("b")]);
        let input = Value::obj(vec![("choice", Value::int(1))]);
        let v = h("choose", &input).unwrap();
        assert_eq!(v, Value::str("b"));
    }

    #[test]
    fn timed_out_decision_errors() {
        let r = HumanDecision::TimedOut.into_result();
        assert!(r.is_err());
    }

    #[test]
    fn slow_handler_delays_then_approves() {
        let h = slow_handler(Duration::from_millis(10));
        let v = h("review", &Value::int(1)).unwrap();
        assert_eq!(v, Value::int(1));
    }
}
