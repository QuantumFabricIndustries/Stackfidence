//! Resource budget (layer 7).
//!
//! Time, tokens, money, API calls — the graph has no native model for any of
//! it in most frameworks. Here the budget is a first-class, threadsafe tracker
//! that every agent spends from via `AgentContext::spend`. When a resource runs
//! out, the runtime can degrade gracefully (skip low-priority nodes, compress
//! context) instead of crashing at the limit.

use crate::error::{AgentError, AgentResult};
use crate::id::SpanId;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The kinds of resources a budget can track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Tokens,
    MoneyMicros, // micro-dollars, to keep integers
    ApiCalls,
    /// Wall-clock budget, in milliseconds.
    WallMs,
}

/// A limit on one resource.
#[derive(Clone, Copy, Debug)]
pub struct BudgetLimit {
    pub kind: ResourceKind,
    pub cap: u64,
}

/// A snapshot of spend so far.
#[derive(Clone, Debug, Default)]
pub struct BudgetReport {
    pub spent: HashMap<ResourceKind, u64>,
}

impl BudgetReport {
    pub fn spent(&self, kind: ResourceKind) -> u64 {
        self.spent.get(&kind).copied().unwrap_or(0)
    }
}

/// The budget tracker. Shared across a run via `Arc<Budget>`.
pub struct Budget {
    limits: Mutex<HashMap<ResourceKind, u64>>,
    spent: Mutex<HashMap<ResourceKind, u64>>,
    /// Per-span spend, for causal attribution.
    per_span: Mutex<HashMap<SpanId, HashMap<ResourceKind, u64>>>,
    start: Instant,
}

impl Budget {
    pub fn new(limits: Vec<BudgetLimit>) -> Self {
        let mut map = HashMap::new();
        for l in limits {
            map.insert(l.kind, l.cap);
        }
        Self {
            limits: Mutex::new(map),
            spent: Mutex::new(HashMap::new()),
            per_span: Mutex::new(HashMap::new()),
            start: Instant::now(),
        }
    }

    /// Unlimited budget (dev/test convenience).
    pub fn unlimited() -> Self {
        Self::new(vec![])
    }

    /// Spend `amount` of `kind`, attributed to `span`. Returns an error if the
    /// cap is exceeded.
    ///
    /// Wall-clock (`WallMs`) is special: it is not "spent" by agents; elapsed
    /// time is read from the internal clock instead. The `amount` argument is
    /// ignored for `WallMs` — the check always runs so that passing `0` cannot
    /// silently bypass the limit.
    pub fn spend(&self, kind: ResourceKind, amount: u64, span: SpanId) -> AgentResult<()> {
        // Wall-clock check MUST come before the amount==0 early-return, because
        // the elapsed time comes from the clock, not from `amount`. A caller that
        // passes 0 (meaning "trigger the check but spend nothing") must still get
        // the limit enforced — the previous ordering silently bypassed it.
        if kind == ResourceKind::WallMs {
            return self.check_wall(kind);
        }
        if amount == 0 {
            return Ok(());
        }
        let mut spent = self.spent.lock();
        let total = spent.entry(kind).or_insert(0);
        *total += amount;
        let total_now = *total;
        drop(spent);

        // attribute to span
        self.per_span
            .lock()
            .entry(span)
            .or_default()
            .entry(kind)
            .and_modify(|v| *v += amount)
            .or_insert(amount);

        let limits = self.limits.lock();
        if let Some(cap) = limits.get(&kind) {
            if total_now > *cap {
                return Err(AgentError::Budget(format!(
                    "{:?} exhausted: spent {} > cap {}",
                    kind, total_now, cap
                )));
            }
        }
        Ok(())
    }

    fn check_wall(&self, kind: ResourceKind) -> AgentResult<()> {
        let elapsed = self.start.elapsed().as_millis() as u64;
        let limits = self.limits.lock();
        if let Some(cap) = limits.get(&kind) {
            // Use >= so a cap of 0 means "already expired" (the run must complete
            // within `cap` milliseconds; 0 means it was already over budget at
            // the instant the Budget was created).
            if elapsed >= *cap {
                return Err(AgentError::Budget(format!(
                    "wall-clock exhausted: {}ms >= cap {}ms",
                    elapsed, cap
                )));
            }
        }
        Ok(())
    }

    /// How much of `kind` has been spent.
    pub fn spent(&self, kind: ResourceKind) -> u64 {
        self.spent.lock().get(&kind).copied().unwrap_or(0)
    }

    /// Remaining budget for `kind`, or `u64::MAX` if unlimited.
    pub fn remaining(&self, kind: ResourceKind) -> u64 {
        let limits = self.limits.lock();
        match limits.get(&kind) {
            Some(cap) => cap.saturating_sub(self.spent(kind)),
            None => u64::MAX,
        }
    }

    /// Is `kind` over budget right now?
    pub fn is_exhausted(&self, kind: ResourceKind) -> bool {
        self.remaining(kind) == 0
    }

    /// Full report.
    pub fn report(&self) -> BudgetReport {
        BudgetReport {
            spent: self.spent.lock().clone(),
        }
    }

    /// Spend attributed to a span.
    pub fn span_spend(&self, span: SpanId) -> HashMap<ResourceKind, u64> {
        self.per_span.lock().get(&span).cloned().unwrap_or_default()
    }

    /// Elapsed wall time since the budget started.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spend_within_limit() {
        let b = Budget::new(vec![BudgetLimit { kind: ResourceKind::Tokens, cap: 1000 }]);
        let s = SpanId::new();
        b.spend(ResourceKind::Tokens, 400, s).unwrap();
        assert_eq!(b.spent(ResourceKind::Tokens), 400);
        assert_eq!(b.remaining(ResourceKind::Tokens), 600);
    }

    #[test]
    fn spend_over_limit_errors() {
        let b = Budget::new(vec![BudgetLimit { kind: ResourceKind::Tokens, cap: 100 }]);
        let s = SpanId::new();
        b.spend(ResourceKind::Tokens, 100, s).unwrap();
        let err = b.spend(ResourceKind::Tokens, 1, s).unwrap_err();
        assert!(matches!(err, AgentError::Budget(_)));
    }

    #[test]
    fn unlimited_never_exhausted() {
        let b = Budget::unlimited();
        let s = SpanId::new();
        b.spend(ResourceKind::ApiCalls, 1_000_000, s).unwrap();
        assert!(!b.is_exhausted(ResourceKind::ApiCalls));
    }

    #[test]
    fn wall_clock_limit_enforced_even_when_amount_is_zero() {
        // Bug regression: passing amount=0 used to short-circuit before checking
        // the wall-clock limit. This test verifies the limit IS checked regardless.
        let b = Budget::new(vec![BudgetLimit { kind: ResourceKind::WallMs, cap: 0 }]);
        let s = SpanId::new();
        // cap=0 means the limit is already exceeded from the first tick.
        // Passing amount=0 must still check the wall-clock and return an error.
        let err = b.spend(ResourceKind::WallMs, 0, s).unwrap_err();
        assert!(
            matches!(err, AgentError::Budget(_)),
            "expected Budget error when wall-clock cap is 0, got {:?}",
            err
        );
    }

    #[test]
    fn wall_clock_limit_passes_within_deadline() {
        // A generous cap (10 minutes) should not fire immediately.
        let b = Budget::new(vec![BudgetLimit {
            kind: ResourceKind::WallMs,
            cap: 600_000, // 10 minutes
        }]);
        let s = SpanId::new();
        b.spend(ResourceKind::WallMs, 1, s).unwrap();
        b.spend(ResourceKind::WallMs, 0, s).unwrap(); // also works with amount=0
    }

    #[test]
    fn span_attribution() {
        let b = Budget::new(vec![BudgetLimit { kind: ResourceKind::Tokens, cap: 1000 }]);
        let s = SpanId::new();
        b.spend(ResourceKind::Tokens, 250, s).unwrap();
        let attr = b.span_spend(s);
        assert_eq!(attr.get(&ResourceKind::Tokens), Some(&250));
    }
}
