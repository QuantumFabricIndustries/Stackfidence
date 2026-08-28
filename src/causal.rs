//! Causal memory (layer 3).
//!
//! A structured record of *why* things happened: what triggered a node, what
//! state it read, what decision it made, and what it changed. Not just a log —
//! a dependency graph of effects. Without this you can't do meaningful
//! reflection, rollback, or trust verification; you just have a black box.
//!
//! The trace is a tree of `Span`s. Each span records its cause (parent span or
//! external trigger), the reads/writes it performed, and the effects it
//! produced. Effects can depend on other effects (cross-span causality).

use crate::blackboard::WriteRecord;
use crate::id::{AgentId, NodeId, SpanId, TraceId};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Why a span exists.
#[derive(Clone, Debug)]
pub enum Cause {
    /// External trigger (user request, scheduler).
    External(String),
    /// Spawned by a parent span.
    Parent(SpanId),
    /// Caused by a specific prior effect.
    Effect(EffectId),
    /// A loop iteration; caused by the previous iteration's span.
    LoopIteration(SpanId, u32),
}

/// An effect a span produced (a decision, an output, a side effect).
#[derive(Clone, Debug)]
pub struct Effect {
    pub id: EffectId,
    pub span: SpanId,
    pub kind: EffectKind,
    pub description: String,
    /// Effects this one depends on.
    pub depends_on: Vec<EffectId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub u64);

#[derive(Clone, Debug)]
pub enum EffectKind {
    Decision,
    Output,
    WriteSlot(String),
    Escalation,
    Interrupt,
}

/// One span in the causal trace.
#[derive(Clone, Debug)]
pub struct Span {
    pub id: SpanId,
    pub trace: TraceId,
    pub node: NodeId,
    pub agent: AgentId,
    pub cause: Cause,
    pub reads: Vec<String>,
    pub writes: Vec<WriteRecord>,
    pub effects: Vec<Effect>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Span {
    pub fn new(id: SpanId, trace: TraceId, node: NodeId, agent: AgentId, cause: Cause) -> Self {
        Self {
            id,
            trace,
            node,
            agent,
            cause,
            reads: Vec::new(),
            writes: Vec::new(),
            effects: Vec::new(),
            started_at: chrono::Utc::now(),
            ended_at: None,
        }
    }

    pub fn end(&mut self) {
        self.ended_at = Some(chrono::Utc::now());
    }

    pub fn record_read(&mut self, slot: String) {
        self.reads.push(slot);
    }

    pub fn record_writes(&mut self, recs: Vec<WriteRecord>) {
        self.writes.extend(recs);
    }

    pub fn add_effect(&mut self, kind: EffectKind, description: impl Into<String>, depends_on: Vec<EffectId>) -> EffectId {
        let id = EffectId(self.effects.len() as u64);
        self.effects.push(Effect {
            id,
            span: self.id,
            kind,
            description: description.into(),
            depends_on,
        });
        id
    }
}

/// The causal trace: a collection of spans with lookup by id.
pub struct CausalTrace {
    pub id: TraceId,
    spans: Mutex<HashMap<SpanId, Span>>,
    /// Children of each span (for tree traversal).
    children: Mutex<HashMap<SpanId, Vec<SpanId>>>,
    /// Monotonic span order (insertion order).
    order: Mutex<Vec<SpanId>>,
    next_effect: Mutex<u64>,
}

impl CausalTrace {
    pub fn new() -> Self {
        Self {
            id: TraceId::new(),
            spans: Mutex::new(HashMap::new()),
            children: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
            next_effect: Mutex::new(0),
        }
    }

    /// Open a span. Returns its id.
    pub fn open(&self, node: NodeId, agent: AgentId, cause: Cause) -> SpanId {
        let id = SpanId::new();
        let parent = cause.parent_span();
        let span = Span::new(id, self.id, node, agent, cause);
        self.spans.lock().insert(id, span);
        self.order.lock().push(id);
        if let Some(p) = parent {
            self.children.lock().entry(p).or_default().push(id);
        }
        id
    }

    /// Record that a span read a slot.
    pub fn record_read(&self, span: SpanId, slot: String) {
        if let Some(s) = self.spans.lock().get_mut(&span) {
            s.record_read(slot);
        }
    }

    /// Record writes drained from the blackboard into a span.
    pub fn record_writes(&self, span: SpanId, recs: Vec<WriteRecord>) {
        if let Some(s) = self.spans.lock().get_mut(&span) {
            s.record_writes(recs);
        }
    }

    /// Add an effect to a span.
    pub fn add_effect(
        &self,
        span: SpanId,
        kind: EffectKind,
        description: impl Into<String>,
        depends_on: Vec<EffectId>,
    ) -> Option<EffectId> {
        let mut spans = self.spans.lock();
        let s = spans.get_mut(&span)?;
        // Use the trace-global counter so effect ids are unique across spans.
        let mut ne = self.next_effect.lock();
        *ne += 1;
        let global = *ne;
        drop(ne);
        let id = EffectId(global);
        s.effects.push(Effect {
            id,
            span,
            kind,
            description: description.into(),
            depends_on,
        });
        Some(id)
    }

    /// Close a span.
    pub fn close(&self, span: SpanId) {
        if let Some(s) = self.spans.lock().get_mut(&span) {
            s.end();
        }
    }

    pub fn get(&self, span: SpanId) -> Option<Span> {
        self.spans.lock().get(&span).cloned()
    }

    pub fn children_of(&self, span: SpanId) -> Vec<SpanId> {
        self.children.lock().get(&span).cloned().unwrap_or_default()
    }

    /// All spans in insertion order.
    pub fn all_spans(&self) -> Vec<Span> {
        let order = self.order.lock();
        let spans = self.spans.lock();
        order.iter().filter_map(|id| spans.get(id).cloned()).collect()
    }

    /// Number of spans.
    pub fn len(&self) -> usize {
        self.spans.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.lock().is_empty()
    }

    /// Find all effects that depended on a given effect (reverse causality).
    pub fn dependents_of(&self, effect: EffectId) -> Vec<EffectId> {
        let spans = self.spans.lock();
        let mut out = Vec::new();
        for s in spans.values() {
            for e in &s.effects {
                if e.depends_on.contains(&effect) {
                    out.push(e.id);
                }
            }
        }
        out
    }
}

impl Cause {
    pub fn parent_span(&self) -> Option<SpanId> {
        match self {
            Cause::Parent(p) => Some(*p),
            Cause::LoopIteration(p, _) => Some(*p),
            _ => None,
        }
    }
}

pub fn shared_trace() -> Arc<CausalTrace> {
    Arc::new(CausalTrace::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_records_spans_and_effects() {
        let t = CausalTrace::new();
        let n = NodeId::new();
        let a = AgentId::new();
        let root = t.open(n, a, Cause::External("user".into()));
        let child = t.open(n, a, Cause::Parent(root));
        let eff = t
            .add_effect(child, EffectKind::Decision, "chose path A", vec![])
            .unwrap();
        t.record_read(child, "state.x".into());
        t.close(child);
        t.close(root);

        assert_eq!(t.len(), 2);
        assert_eq!(t.children_of(root), vec![child]);
        let s = t.get(child).unwrap();
        assert_eq!(s.reads, vec!["state.x".to_string()]);
        assert_eq!(s.effects.len(), 1);
        assert_eq!(t.dependents_of(eff), vec![]);
    }

    #[test]
    fn effect_dependency_chain() {
        let t = CausalTrace::new();
        let n = NodeId::new();
        let a = AgentId::new();
        let s1 = t.open(n, a, Cause::External("start".into()));
        let e1 = t.add_effect(s1, EffectKind::Output, "produced x", vec![]).unwrap();
        let s2 = t.open(n, a, Cause::Effect(e1));
        let e2 = t.add_effect(s2, EffectKind::Decision, "used x", vec![e1]).unwrap();
        assert_eq!(t.dependents_of(e1), vec![e2]);
    }
}
