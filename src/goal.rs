//! Goal / intent model (layer 5).
//!
//! The original goal — what the user actually wanted — has no formal
//! representation in most agent graphs. By node 4 it's diluted through
//! paraphrasing and instruction drift. Here a `Goal` travels with the
//! execution as an invariant: every node's output is checked against a
//! user-supplied satisfaction predicate. Drift is detected and can raise an
//! interrupt (Goodhart's Law applied at the agent level).

use crate::error::InterruptKind;
use crate::value::Value;
use parking_lot::Mutex;
use std::sync::Arc;

/// Status of alignment to the goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalStatus {
    /// Output still aligned with the goal.
    Aligned,
    /// Output drifted but is recoverable.
    Drifting,
    /// Output no longer serves the goal.
    Violated,
}

/// A report from checking an output against the goal.
#[derive(Clone, Debug)]
pub struct GoalReport {
    pub status: GoalStatus,
    pub note: String,
}

/// The goal invariant.
///
/// `satisfied` is a user-supplied predicate: given the current blackboard
/// snapshot (as a `Value` object) and the latest output, is the goal met?
/// `aligned` checks whether a single output keeps the run on-track.
pub struct Goal {
    /// Human-readable statement of intent. Travels with the run.
    pub statement: String,
    satisfied: Mutex<Box<dyn Fn(&Value, &Value) -> bool + Send + Sync>>,
    aligned: Mutex<Box<dyn Fn(&Value, &Value) -> GoalReport + Send + Sync>>,
    /// Number of drift detections so far.
    drift_count: Mutex<u32>,
}

impl std::fmt::Debug for Goal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Goal")
            .field("statement", &self.statement)
            .field("drift_count", &*self.drift_count.lock())
            .finish()
    }
}

impl Goal {
    pub fn new(
        statement: impl Into<String>,
        satisfied: impl Fn(&Value, &Value) -> bool + Send + Sync + 'static,
        aligned: impl Fn(&Value, &Value) -> GoalReport + Send + Sync + 'static,
    ) -> Self {
        Self {
            statement: statement.into(),
            satisfied: Mutex::new(Box::new(satisfied)),
            aligned: Mutex::new(Box::new(aligned)),
            drift_count: Mutex::new(0),
        }
    }

    /// A trivial goal that's always satisfied and aligned. Useful for tests /
    /// when a run has no explicit goal (but you should supply one).
    pub fn trivial() -> Arc<Self> {
        Arc::new(Self::new(
            "trivial",
            |_, _| true,
            |_, _| GoalReport { status: GoalStatus::Aligned, note: "no constraint".into() },
        ))
    }

    /// Is the goal fully satisfied given the blackboard snapshot and last output?
    pub fn is_satisfied(&self, snapshot: &Value, output: &Value) -> bool {
        (self.satisfied.lock())(snapshot, output)
    }

    /// Check alignment of a single output. Increments drift_count if drifting.
    pub fn check(&self, snapshot: &Value, output: &Value) -> GoalReport {
        let r = (self.aligned.lock())(snapshot, output);
        if r.status != GoalStatus::Aligned {
            *self.drift_count.lock() += 1;
        }
        r
    }

    pub fn drift_count(&self) -> u32 {
        *self.drift_count.lock()
    }

    /// Convenience: map a violated goal to an interrupt kind.
    pub fn violated_interrupt_kind() -> InterruptKind {
        InterruptKind::GoalDrift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_goal_always_aligned() {
        let g = Goal::trivial();
        let r = g.check(&Value::null(), &Value::str("anything"));
        assert_eq!(r.status, GoalStatus::Aligned);
        assert!(g.is_satisfied(&Value::null(), &Value::null()));
    }

    #[test]
    fn drift_detected() {
        let g = Goal::new(
            "sum must be positive",
            |snap, _| snap.to_json().get("sum").and_then(|v| v.as_i64()).map(|i| i > 0).unwrap_or(false),
            |_snap, output| {
            if matches!(output, Value::Int(i) if *i < 0) {
                GoalReport { status: GoalStatus::Violated, note: "negative".into() }
            } else {
                GoalReport { status: GoalStatus::Aligned, note: "ok".into() }
            }
            },
        );
        let snap = Value::obj(vec![("sum", Value::int(5))]);
        assert_eq!(g.check(&snap, &Value::int(-1)).status, GoalStatus::Violated);
        assert_eq!(g.drift_count(), 1);
        assert!(g.is_satisfied(&snap, &Value::null()));
    }
}
