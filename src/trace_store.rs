//! Trace store — disk persistence for causal memory (layer 3, part 2).
//!
//! Persists a `CausalTrace` to disk as JSON so a run can be inspected,
//! diffed, and explained after the fact. Mirrors VerifyStack's
//! `.verifystack/graph_cache.json` pattern: a small, typed, JSON file under a
//! per-run cache directory that is safe to delete and rebuild.

use crate::causal::{Cause, EffectKind};
use crate::error::AgentResult;
use crate::id::SpanId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// A serializable span.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpanRecord {
    pub id: SpanId,
    pub node: crate::id::NodeId,
    pub agent: crate::id::AgentId,
    pub cause: CauseRecord,
    pub reads: Vec<String>,
    pub writes: Vec<WriteRecordSer>,
    pub effects: Vec<EffectRecord>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CauseRecord {
    External(String),
    Parent(SpanId),
    Effect(u64),
    LoopIteration(SpanId, u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteRecordSer {
    pub slot: String,
    pub writer: crate::id::AgentId,
    pub value: serde_json::Value,
    pub seq: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectRecord {
    pub id: u64,
    pub span: SpanId,
    pub kind: String,
    pub description: String,
    pub depends_on: Vec<u64>,
}

/// A serializable trace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceRecord {
    pub id: crate::id::TraceId,
    pub spans: Vec<SpanRecord>,
    pub children: HashMap<String, Vec<SpanId>>,
}

impl TraceRecord {
    /// Build from a live `CausalTrace`.
    pub fn from_trace(trace: &crate::causal::CausalTrace) -> Self {
        let spans: Vec<SpanRecord> = trace
            .all_spans()
            .into_iter()
            .map(|s| SpanRecord {
                id: s.id,
                node: s.node,
                agent: s.agent,
                cause: cause_to_record(&s.cause),
                reads: s.reads.clone(),
                writes: s
                    .writes
                    .iter()
                    .map(|w| WriteRecordSer {
                        slot: w.slot.clone(),
                        writer: w.writer,
                        value: w.value.to_json(),
                        seq: w.seq,
                    })
                    .collect(),
                effects: s
                    .effects
                    .iter()
                    .map(|e| EffectRecord {
                        id: e.id.0,
                        span: e.span,
                        kind: effect_kind_str(&e.kind),
                        description: e.description.clone(),
                        depends_on: e.depends_on.iter().map(|d| d.0).collect(),
                    })
                    .collect(),
                started_at: s.started_at,
                ended_at: s.ended_at,
            })
            .collect();

        // children map
        let mut children: HashMap<String, Vec<SpanId>> = HashMap::new();
        for s in trace.all_spans() {
            if let Some(parent) = s.cause.parent_span() {
                children
                    .entry(parent.to_string())
                    .or_default()
                    .push(s.id);
            }
        }

        Self { id: trace.id, spans, children }
    }

    /// Save to a JSON file.
    pub fn save(&self, path: &Path) -> AgentResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load from a JSON file.
    pub fn load(path: &Path) -> AgentResult<Self> {
        let json = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }
}

fn cause_to_record(c: &Cause) -> CauseRecord {
    match c {
        Cause::External(s) => CauseRecord::External(s.clone()),
        Cause::Parent(p) => CauseRecord::Parent(*p),
        Cause::Effect(e) => CauseRecord::Effect(e.0),
        Cause::LoopIteration(p, i) => CauseRecord::LoopIteration(*p, *i),
    }
}

fn effect_kind_str(k: &EffectKind) -> String {
    match k {
        EffectKind::Decision => "decision".into(),
        EffectKind::Output => "output".into(),
        EffectKind::WriteSlot(s) => format!("write:{}", s),
        EffectKind::Escalation => "escalation".into(),
        EffectKind::Interrupt => "interrupt".into(),
    }
}

// Re-exports for tests / external use.
pub use crate::causal::Span as _SpanReexport;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::causal::{CausalTrace, EffectKind};
    use crate::id::{AgentId, NodeId};
    use tempfile::tempdir;

    #[test]
    fn trace_roundtrips_through_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trace.json");
        let t = CausalTrace::new();
        let n = NodeId::new();
        let a = AgentId::new();
        let s = t.open(n, a, Cause::External("user".into()));
        t.add_effect(s, EffectKind::Decision, "decided", vec![]).unwrap();
        t.close(s);

        let rec = TraceRecord::from_trace(&t);
        rec.save(&path).unwrap();
        let back = TraceRecord::load(&path).unwrap();
        assert_eq!(back.spans.len(), 1);
        assert_eq!(back.spans[0].effects.len(), 1);
    }
}
