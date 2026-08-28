//! The blackboard — the coordination substrate (layer 2).
//!
//! A typed, shared working memory that agents read from, write to, and contend
//! over, with rules about who can do what (enforced via `AccessPolicy`).
//!
//! This is *not* a key-value store bolted on. It is the place agents genuinely
//! coordinate through: every read is recorded for causal memory, every write is
//! arbitrated by policy, and slots can be locked for atomic read-modify-write.
//!
//! Design borrowed from VerifyStack's per-file cache pattern: a small, typed,
//! serializable store with explicit keys rather than a sprawling dict.

use crate::error::{AgentError, AgentResult};
use crate::id::AgentId;
use crate::policy::{AccessPolicy, Permission};
use crate::value::Value;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;

/// A slot key is a dotted string path, e.g. `results.summarize` or `state.count`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlotKey(pub String);

impl SlotKey {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SlotKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SlotKey {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SlotKey {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// One slot's history entry (for causal memory to consume).
#[derive(Clone, Debug)]
pub struct WriteRecord {
    pub slot: String,
    pub writer: AgentId,
    pub value: Value,
    pub seq: u64,
}

/// The blackboard.
pub struct Blackboard {
    slots: RwLock<HashMap<String, Value>>,
    policy: AccessPolicy,
    /// Monotonic write sequence, for ordering in causal traces.
    seq: Mutex<u64>,
    /// Append-only write log (consumed by the causal tracer).
    log: Mutex<Vec<WriteRecord>>,
}

impl Blackboard {
    pub fn new(policy: AccessPolicy) -> Self {
        Self {
            slots: RwLock::new(HashMap::new()),
            policy,
            seq: Mutex::new(0),
            log: Mutex::new(Vec::new()),
        }
    }

    /// Build a blackboard with a permissive policy (dev/test convenience).
    pub fn permissive() -> Arc<Self> {
        Arc::new(Self::new(AccessPolicy::new().with_default_allow(true)))
    }

    /// Read a slot. Records nothing (reads are tracked by the causal tracer via
    /// `AgentContext`). Returns `Value::Null` if absent.
    pub fn read(&self, agent: AgentId, key: &SlotKey) -> AgentResult<Value> {
        if !self.policy.check(agent, key.as_str(), Permission::Read) {
            return Err(AgentError::Blackboard(format!(
                "agent {:?} denied read on {}",
                agent, key
            )));
        }
        Ok(self.slots.read().get(key.as_str()).cloned().unwrap_or(Value::Null))
    }

    /// Write a slot. Enforces policy and appends to the write log.
    pub fn write(&self, agent: AgentId, key: &SlotKey, value: Value) -> AgentResult<()> {
        if !self.policy.check(agent, key.as_str(), Permission::Write) {
            return Err(AgentError::Blackboard(format!(
                "agent {:?} denied write on {}",
                agent, key
            )));
        }
        let mut seq = self.seq.lock();
        *seq += 1;
        let s = *seq;
        drop(seq);
        self.slots.write().insert(key.as_str().to_string(), value.clone());
        self.log.lock().push(WriteRecord {
            slot: key.as_str().to_string(),
            writer: agent,
            value,
            seq: s,
        });
        Ok(())
    }

    /// Atomic read-modify-write. Requires Lock permission. Holds the slots
    /// write lock for the whole RMW so concurrent updaters on the same slot
    /// serialize. (The executor is sequential today; this keeps the contract
    /// correct if concurrency is added later.)
    pub fn update<F>(&self, agent: AgentId, key: &SlotKey, f: F) -> AgentResult<Value>
    where
        F: FnOnce(&Value) -> Value,
    {
        if !self.policy.check(agent, key.as_str(), Permission::Lock) {
            return Err(AgentError::Blackboard(format!(
                "agent {:?} denied lock on {}",
                agent, key
            )));
        }
        let mut slots = self.slots.write();
        let current = slots.get(key.as_str()).cloned().unwrap_or(Value::Null);
        let new = f(&current);
        let mut seq = self.seq.lock();
        *seq += 1;
        let s = *seq;
        drop(seq);
        slots.insert(key.as_str().to_string(), new.clone());
        self.log.lock().push(WriteRecord {
            slot: key.as_str().to_string(),
            writer: agent,
            value: new.clone(),
            seq: s,
        });
        Ok(new)
    }

    /// Drain the write log (called by the causal tracer at end of a span).
    pub fn drain_log(&self) -> Vec<WriteRecord> {
        std::mem::take(&mut *self.log.lock())
    }

    /// Snapshot all slots (for persistence / replay).
    pub fn snapshot(&self) -> HashMap<String, Value> {
        self.slots.read().clone()
    }

    /// Restore a snapshot (for replay).
    pub fn restore(&self, snap: HashMap<String, Value>) {
        *self.slots.write() = snap;
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.slots.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read() {
        let bb = Blackboard::permissive();
        let a = AgentId::new();
        bb.write(a, &SlotKey::new("x"), Value::int(42)).unwrap();
        let v = bb.read(a, &SlotKey::new("x")).unwrap();
        assert_eq!(v, Value::int(42));
    }

    #[test]
    fn missing_slot_is_null() {
        let bb = Blackboard::permissive();
        let a = AgentId::new();
        assert!(bb.read(a, &SlotKey::new("nope")).unwrap().is_null());
    }

    #[test]
    fn update_is_atomic() {
        let bb = Blackboard::permissive();
        let a = AgentId::new();
        bb.write(a, &SlotKey::new("count"), Value::int(1)).unwrap();
        let new = bb.update(a, &SlotKey::new("count"), |v| {
            if let Value::Int(i) = v {
                Value::Int(i + 1)
            } else {
                Value::Int(1)
            }
        }).unwrap();
        assert_eq!(new, Value::int(2));
        assert_eq!(bb.read(a, &SlotKey::new("count")).unwrap(), Value::int(2));
    }

    #[test]
    fn policy_denies() {
        let policy = AccessPolicy::new(); // fail-closed
        let bb = Blackboard::new(policy);
        let a = AgentId::new();
        let err = bb.write(a, &SlotKey::new("x"), Value::int(1)).unwrap_err();
        assert!(matches!(err, AgentError::Blackboard(_)));
    }

    #[test]
    fn write_log_records_writes() {
        let bb = Blackboard::permissive();
        let a = AgentId::new();
        bb.write(a, &SlotKey::new("x"), Value::int(1)).unwrap();
        bb.write(a, &SlotKey::new("y"), Value::bool(true)).unwrap();
        let log = bb.drain_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].slot, "x");
        assert_eq!(log[1].seq, 2);
    }
}
