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
    pub fn spend(&self, kind: ResourceKind, amount: u64, span: SpanId) -> AgentResult<()> {
        if amount == 0 {
            return Ok(());
        }
        // Special-case wall clock: it's not "spent" by agents, it elapses.
        if kind == ResourceKind::WallMs {
            return self.check_wall(kind);
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
            if elapsed > *cap {
                return Err(AgentError::Budget(format!(
                    "wall-clock exhausted: {:?}ms > cap {:?}ms",
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
    fn span_attribution() {
        let b = Budget::new(vec![BudgetLimit { kind: ResourceKind::Tokens, cap: 1000 }]);
        let s = SpanId::new();
        b.spend(ResourceKind::Tokens, 250, s).unwrap();
        let attr = b.span_spend(s);
        assert_eq!(attr.get(&ResourceKind::Tokens), Some(&250));
    }
}
