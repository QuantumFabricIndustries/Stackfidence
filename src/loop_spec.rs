//! Loop specification (layer 1, part 2).
//!
//! Loops give graphs iteration: repeat a node a fixed number of times, while a
//! blackboard condition holds, until it holds, or once per item in a list slot.
//! `LoopState` tracks iterations so the executor and causal trace can record
//! *why* a given iteration happened (Cause::LoopIteration).

use crate::blackboard::SlotKey;
use crate::value::Value;

/// Kind of loop.
#[derive(Clone, Debug)]
pub enum LoopKind {
    /// Repeat exactly `count` times.
    Repeat { count: u32 },
    /// Loop while `slot` equals `target`.
    While { slot: SlotKey, target: Value },
    /// Loop until `slot` equals `target`.
    Until { slot: SlotKey, target: Value },
    /// Loop once per item in the list stored at `slot`.
    Foreach { slot: SlotKey },
}

/// A loop spec attached to a node.
#[derive(Clone, Debug)]
pub struct LoopSpec {
    pub kind: LoopKind,
    /// Hard cap on iterations regardless of condition (safety against runaway).
    pub max_iters: u32,
}

impl LoopSpec {
    pub fn repeat(count: u32) -> Self {
        Self { kind: LoopKind::Repeat { count }, max_iters: count.max(1) }
    }
    pub fn while_eq(slot: impl Into<SlotKey>, target: Value, max: u32) -> Self {
        Self { kind: LoopKind::While { slot: slot.into(), target }, max_iters: max.max(1) }
    }
    pub fn until_eq(slot: impl Into<SlotKey>, target: Value, max: u32) -> Self {
        Self { kind: LoopKind::Until { slot: slot.into(), target }, max_iters: max.max(1) }
    }
    pub fn foreach(slot: impl Into<SlotKey>, max: u32) -> Self {
        Self { kind: LoopKind::Foreach { slot: slot.into() }, max_iters: max.max(1) }
    }

    /// Decide whether to continue given the current iteration count and a
    /// blackboard read of the relevant slot.
    pub fn continues(&self, state: &LoopState, slot_value: &Value) -> bool {
        if state.iteration >= self.max_iters {
            return false;
        }
        match &self.kind {
            LoopKind::Repeat { count } => state.iteration < *count,
            LoopKind::While { target, .. } => slot_value == target,
            LoopKind::Until { target, .. } => slot_value != target,
            LoopKind::Foreach { .. } => {
                // Continue while there are still items at or beyond the index.
                if let Value::List(items) = slot_value {
                    (state.iteration as usize) < items.len()
                } else {
                    false
                }
            }
        }
    }

    /// For foreach loops, the item for the current iteration.
    pub fn current_item<'a>(&self, slot_value: &'a Value, state: &LoopState) -> Option<&'a Value> {
        if let LoopKind::Foreach { .. } = &self.kind {
            if let Value::List(items) = slot_value {
                return items.get(state.iteration as usize);
            }
        }
        None
    }

    /// The slot this loop reads its condition from (if any).
    pub fn slot(&self) -> Option<&SlotKey> {
        match &self.kind {
            LoopKind::While { slot, .. } | LoopKind::Until { slot, .. } | LoopKind::Foreach { slot } => Some(slot),
            LoopKind::Repeat { .. } => None,
        }
    }
}

/// Mutable loop state tracked by the executor.
#[derive(Clone, Debug, Default)]
pub struct LoopState {
    pub iteration: u32,
    pub done: bool,
}

impl LoopState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn advance(&mut self) {
        self.iteration += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_loop() {
        let spec = LoopSpec::repeat(3);
        let mut s = LoopState::new();
        assert!(spec.continues(&s, &Value::null()));
        s.advance(); // 1
        assert!(spec.continues(&s, &Value::null()));
        s.advance(); // 2
        assert!(spec.continues(&s, &Value::null()));
        s.advance(); // 3
        assert!(!spec.continues(&s, &Value::null()));
    }

    #[test]
    fn while_eq_loop() {
        let spec = LoopSpec::while_eq("flag", Value::bool(true), 10);
        let mut s = LoopState::new();
        assert!(spec.continues(&s, &Value::bool(true)));
        assert!(!spec.continues(&s, &Value::bool(false)));
        s.advance();
        assert!(spec.continues(&s, &Value::bool(true)));
    }

    #[test]
    fn until_eq_loop() {
        let spec = LoopSpec::until_eq("done", Value::bool(true), 10);
        let s = LoopState::new();
        assert!(spec.continues(&s, &Value::bool(false)));
        assert!(!spec.continues(&s, &Value::bool(true)));
    }

    #[test]
    fn foreach_loop() {
        let spec = LoopSpec::foreach("items", 10);
        let items = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let mut s = LoopState::new();
        assert!(spec.continues(&s, &items));
        assert_eq!(spec.current_item(&items, &s), Some(&Value::int(1)));
        s.advance();
        assert_eq!(spec.current_item(&items, &s), Some(&Value::int(2)));
        s.advance();
        s.advance();
        assert!(!spec.continues(&s, &items));
    }

    #[test]
    fn max_iters_caps() {
        let spec = LoopSpec::while_eq("flag", Value::bool(true), 2);
        let mut s = LoopState::new();
        s.advance();
        s.advance();
        assert!(!spec.continues(&s, &Value::bool(true))); // capped at 2
    }
}
