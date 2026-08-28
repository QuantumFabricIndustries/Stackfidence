//! Interrupt bus (layer 8).
//!
//! Graphs have edges for happy paths; loops have break conditions. Neither has
//! a formal model for anomalous conditions that should short-circuit the normal
//! flow and propagate structured context upward. The interrupt bus is that
//! model: any node can raise an `Interrupt` (defined in `error.rs`), and the
//! executor checks the bus between steps, aborting the flow with full context —
//! not just an error string, but what state was read, what failed, and why.

use crate::error::Interrupt;
use parking_lot::Mutex;
use std::sync::Arc;

/// A broadcast bus for interrupts. Shared across a run via `Arc`.
#[derive(Default)]
pub struct InterruptBus {
    queue: Mutex<Vec<Interrupt>>,
    /// Whether abort has been requested (short-circuits further checks).
    aborted: Mutex<bool>,
}

impl InterruptBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Raise an interrupt. Multiple raises queue up; the executor processes the
    /// first and aborts.
    pub fn raise(&self, i: Interrupt) {
        if matches!(i.kind, crate::error::InterruptKind::Abort) {
            *self.aborted.lock() = true;
        }
        self.queue.lock().push(i);
    }

    /// Is there a pending interrupt?
    pub fn pending(&self) -> bool {
        !self.queue.lock().is_empty() || *self.aborted.lock()
    }

    /// Take the first pending interrupt (if any).
    pub fn take(&self) -> Option<Interrupt> {
        self.queue.lock().first().cloned()
    }

    /// Drain all pending interrupts.
    pub fn drain(&self) -> Vec<Interrupt> {
        std::mem::take(&mut *self.queue.lock())
    }

    /// Was an explicit abort raised?
    pub fn aborted(&self) -> bool {
        *self.aborted.lock()
    }

    pub fn clear(&self) {
        self.queue.lock().clear();
        *self.aborted.lock() = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Interrupt, InterruptKind};

    #[test]
    fn raise_and_take() {
        let bus = InterruptBus::new();
        bus.raise(Interrupt::new(InterruptKind::BudgetExhausted, "out of tokens"));
        assert!(bus.pending());
        let i = bus.take().unwrap();
        assert_eq!(i.kind, InterruptKind::BudgetExhausted);
    }

    #[test]
    fn abort_short_circuits() {
        let bus = InterruptBus::new();
        bus.raise(Interrupt::new(InterruptKind::Abort, "user abort"));
        assert!(bus.aborted());
        assert!(bus.pending());
    }
}
