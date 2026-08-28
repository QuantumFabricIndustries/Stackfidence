//! Run record — for determinism / replayability (layer 10).
//!
//! Given the same inputs and the same model-call responses, a run should be
//! reproducible. Most agent systems are non-deterministic *by accident*. Here
//! every model call an agent makes is recorded (inputs + outputs) into a
//! `RunRecord`. The `Replayer` (replay.rs) feeds those recorded responses back
//! to agents instead of calling a real model, making the run deterministic.
//!
//! Pairs with causal memory: causal memory records *what* happened; the run
//! record guarantees you could make it happen again.

use crate::id::SpanId;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// A single recorded model call: the request that went out and the response
/// that came back. The `key` is a deterministic hash of the request so replays
/// can match calls.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelCall {
    pub span: SpanId,
    /// A label identifying which agent / step made the call.
    pub caller: String,
    /// Deterministic key derived from the request (e.g. sha256 of inputs).
    pub key: String,
    /// The request payload (JSON).
    pub request: serde_json::Value,
    /// The response payload (JSON) that was returned.
    pub response: serde_json::Value,
    /// Tokens spent (if known).
    pub tokens: u64,
}

/// The run record: append-only log of model calls + the initial blackboard
/// seed. Serializable so a run can be persisted and replayed later.
#[derive(Default, Serialize, Deserialize)]
pub struct RunRecord {
    pub calls: Vec<ModelCall>,
    /// Initial blackboard seed at run start (slot -> value JSON).
    pub seed: Vec<(String, serde_json::Value)>,
}

impl RunRecord {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_seed(seed: Vec<(String, serde_json::Value)>) -> Self {
        Self { calls: Vec::new(), seed }
    }

    pub fn record_call(&mut self, call: ModelCall) {
        self.calls.push(call);
    }

    /// Look up the recorded response for a given key (used by the replayer).
    pub fn response_for(&self, key: &str) -> Option<&serde_json::Value> {
        self.calls.iter().find(|c| c.key == key).map(|c| &c.response)
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Deserialize from JSON.
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(v.clone()).ok()
    }
}

/// A thread-safe wrapper used by `AgentContext`.
pub struct SyncRunRecord(pub Mutex<RunRecord>);

impl SyncRunRecord {
    pub fn new() -> Self {
        Self(Mutex::new(RunRecord::new()))
    }
}

// The agent context uses `Arc<Mutex<RunRecord>>` directly; provide a helper.
impl RunRecord {
    pub fn shared() -> std::sync::Arc<Mutex<Self>> {
        std::sync::Arc::new(Mutex::new(Self::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_lookup() {
        let mut r = RunRecord::new();
        r.record_call(ModelCall {
            span: SpanId::new(),
            caller: "echo".into(),
            key: "k1".into(),
            request: serde_json::json!({"q": "hi"}),
            response: serde_json::json!({"a": "hello"}),
            tokens: 5,
        });
        assert_eq!(r.response_for("k1").unwrap()["a"], "hello");
        assert!(r.response_for("missing").is_none());
    }

    #[test]
    fn json_roundtrip() {
        let mut r = RunRecord::with_seed(vec![("x".into(), serde_json::json!(1))]);
        r.record_call(ModelCall {
            span: SpanId::new(),
            caller: "a".into(),
            key: "k".into(),
            request: serde_json::json!({}),
            response: serde_json::json!({"ok": true}),
            tokens: 0,
        });
        let j = r.to_json();
        let back = RunRecord::from_json(&j).unwrap();
        assert_eq!(back.calls.len(), 1);
        assert_eq!(back.seed.len(), 1);
    }
}
