//! Decay / forgetting model for working memory (layer 12).
//!
//! Memory that never forgets becomes noise. The `MemoryStore` is the working-
//! set manager: entries have a decay policy that decides what to keep verbatim,
//! what to compress (summarize into a shorter value), and what to drop — and on
//! what schedule. This is distinct from causal memory (the trace): the trace
//! records *what* happened; this manages *what stays in working context* so
//! agents don't reason against ghosts of old runs.

use crate::value::Value;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::collections::HashMap;

/// When to act on an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecayKind {
    /// Keep verbatim forever.
    Keep,
    /// Compress (replace with a summary) after `ttl_seconds`.
    Compress { ttl_seconds: i64 },
    /// Drop after `ttl_seconds`.
    Drop { ttl_seconds: i64 },
}

impl Default for DecayKind {
    fn default() -> Self {
        DecayKind::Keep
    }
}

/// A decay policy maps slot names to a decay kind.
#[derive(Default)]
pub struct DecayPolicy {
    rules: HashMap<String, DecayKind>,
    /// Default kind for slots without an explicit rule.
    default: DecayKind,
}

impl DecayPolicy {
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            default: DecayKind::Keep,
        }
    }

    pub fn set_default(mut self, k: DecayKind) -> Self {
        self.default = k;
        self
    }

    pub fn set(&mut self, slot: impl Into<String>, kind: DecayKind) {
        self.rules.insert(slot.into(), kind);
    }

    pub fn kind_for(&self, slot: &str) -> DecayKind {
        self.rules.get(slot).copied().unwrap_or(self.default)
    }
}

/// One working-memory entry.
#[derive(Clone, Debug)]
pub struct MemoryEntry {
    pub slot: String,
    pub value: Value,
    pub created_at: DateTime<Utc>,
    pub last_touched: DateTime<Utc>,
    pub compressed: bool,
}

/// The working-memory store with decay.
pub struct MemoryStore {
    entries: HashMap<String, MemoryEntry>,
    policy: DecayPolicy,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self { entries: HashMap::new(), policy: DecayPolicy::new() }
    }

    pub fn with_policy(policy: DecayPolicy) -> Self {
        Self { entries: HashMap::new(), policy }
    }

    /// Write / touch an entry.
    pub fn put(&mut self, slot: impl Into<String>, value: Value) {
        let now = Utc::now();
        let slot = slot.into();
        self.entries.insert(
            slot.clone(),
            MemoryEntry {
                slot,
                value,
                created_at: now,
                last_touched: now,
                compressed: false,
            },
        );
    }

    pub fn get(&self, slot: &str) -> Option<&MemoryEntry> {
        self.entries.get(slot)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Run decay: compress or drop entries whose TTL has elapsed. Returns the
    /// number of entries dropped.
    pub fn decay(&mut self) -> usize {
        let now = Utc::now();
        let mut dropped = 0;
        let slots: Vec<String> = self.entries.keys().cloned().collect();
        for slot in slots {
            let kind = self.policy.kind_for(&slot);
            let entry = match self.entries.get_mut(&slot) {
                Some(e) => e,
                None => continue,
            };
            let age = (now - entry.last_touched).num_seconds();
            match kind {
                DecayKind::Keep => {}
                DecayKind::Compress { ttl_seconds } => {
                    if age >= ttl_seconds && !entry.compressed {
                        entry.value = compress(&entry.value);
                        entry.compressed = true;
                        entry.last_touched = now;
                    }
                }
                DecayKind::Drop { ttl_seconds } => {
                    if age >= ttl_seconds {
                        self.entries.remove(&slot);
                        dropped += 1;
                    }
                }
            }
        }
        dropped
    }

    /// Snapshot all entries (for persistence).
    pub fn snapshot(&self) -> Vec<MemoryEntry> {
        self.entries.values().cloned().collect()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Compress a value into a shorter summary. The real system would call an LLM;
/// here we do a deterministic structural compression: long strings are
/// truncated, long lists are capped, objects keep only field names.
fn compress(v: &Value) -> Value {
    match v {
        Value::Str(s) => {
            if s.len() > 64 {
                Value::str(format!("{}…({}b)", &s[..64], s.len()))
            } else {
                v.clone()
            }
        }
        Value::List(items) => {
            if items.len() > 8 {
                let mut head: Vec<Value> = items.iter().take(8).cloned().collect();
                head.push(Value::str(format!("…(+{} items)", items.len() - 8)));
                Value::List(head)
            } else {
                v.clone()
            }
        }
        Value::Object(fields) => {
            let summary: Vec<(String, Value)> = fields
                .iter()
                .map(|(k, child)| (k.clone(), Value::str(child.shape(1))))
                .collect();
            Value::Object(summary)
        }
        _ => v.clone(),
    }
}

// Thread-safe wrapper used by the runtime.
pub type SyncMemoryStore = Mutex<MemoryStore>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn drop_after_ttl() {
        let mut p = DecayPolicy::new();
        p.set("temp", DecayKind::Drop { ttl_seconds: 0 });
        let mut m = MemoryStore::with_policy(p);
        m.put("temp", Value::int(1));
        assert_eq!(m.len(), 1);
        sleep(Duration::from_millis(10));
        let dropped = m.decay();
        assert_eq!(dropped, 1);
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn keep_forever() {
        let mut m = MemoryStore::new();
        m.put("perm", Value::int(42));
        sleep(Duration::from_millis(5));
        m.decay();
        assert_eq!(m.get("perm").unwrap().value, Value::int(42));
    }

    #[test]
    fn compresses_long_string() {
        let long = Value::str("x".repeat(200));
        let c = compress(&long);
        match c {
            Value::Str(s) => assert!(s.contains("…")),
            _ => panic!("expected compressed string"),
        }
    }

    #[test]
    fn compresses_object_to_shapes() {
        let obj = Value::obj(vec![("a", Value::int(1)), ("b", Value::str("hi"))]);
        let c = compress(&obj);
        match c {
            Value::Object(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "a");
            }
            _ => panic!("expected object"),
        }
    }
}
