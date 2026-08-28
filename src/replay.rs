//! Replayer (layer 10, part 2).
//!
//! Given a `RunRecord` (record.rs), re-run a graph deterministically by feeding
//! the recorded model-call responses back to agents instead of calling a real
//! model. The replayer wraps the runtime so that any agent's `record_model_call`
//! becomes a lookup against the recorded responses.
//!
//! Determinism guarantee: if agents are pure functions of (input, model-call
//! responses) and the run record captures every model call's request+response,
//! then replay produces an identical causal trace.

use crate::error::{AgentError, AgentResult};
use crate::record::RunRecord;
use crate::runtime::Runtime;
use crate::value::Value;
use parking_lot::Mutex;
use std::sync::Arc;

/// A replayer wraps a runtime and a recorded run, serving recorded responses.
pub struct Replayer {
    pub record: Arc<Mutex<RunRecord>>,
}

impl Replayer {
    pub fn new(record: Arc<Mutex<RunRecord>>) -> Self {
        Self { record }
    }

    /// Build a replayer from a serialized run record (e.g. loaded from disk).
    pub fn from_json(json: &serde_json::Value) -> AgentResult<Self> {
        let rec = RunRecord::from_json(json)
            .ok_or_else(|| AgentError::Other("invalid run record".into()))?;
        Ok(Self::new(Arc::new(Mutex::new(rec))))
    }

    /// Look up the recorded response for a request key. Agents that want to be
    /// replayable should call this (via their context) instead of a real model.
    pub fn response_for(&self, key: &str) -> Option<serde_json::Value> {
        self.record.lock().response_for(key).cloned()
    }

    /// Replay a run against a runtime: seed the blackboard from the record and
    /// run the graph. Agents must consult the replayer for model responses.
    pub fn replay(
        &self,
        rt: &Runtime,
        graph: &crate::graph::Graph,
        config: crate::executor::RunConfig,
    ) -> AgentResult<crate::executor::RunOutcome> {
        // If the caller didn't supply a seed, use the recorded one.
        let mut cfg = config;
        if cfg.seed.is_empty() {
            let rec = self.record.lock();
            cfg.seed = rec
                .seed
                .iter()
                .map(|(k, v)| (k.clone(), Value::from_json(v)))
                .collect();
        }
        rt.run(graph, cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{RunConfig, RunStatus};
    use crate::graph::{Graph, Node};
    use crate::id::AgentId;
    use crate::value::Value;
    use crate::agent::{Agent, AgentContext, AgentOutput};

    struct RecordedAgent {
        id: AgentId,
        key: &'static str,
        response: Value,
    }
    impl Agent for RecordedAgent {
        fn id(&self) -> AgentId { self.id }
        fn name(&self) -> &str { "recorded" }
        fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
            // Record a model call with a fixed key so replay can match it.
            ctx.record_model_call(crate::record::ModelCall {
                span: ctx.span,
                caller: "recorded".into(),
                key: self.key.into(),
                request: serde_json::json!({"input": ctx.input.to_json()}),
                response: self.response.to_json(),
                tokens: 1,
            });
            Ok(AgentOutput::done(self.response.clone()))
        }
    }

    #[test]
    fn replay_serves_recorded_response() {
        let rt = Runtime::permissive();
        rt.register("a", RecordedAgent { id: AgentId::new(), key: "k1", response: Value::int(42) });
        let mut g = Graph::new("r");
        let a = g.add_node(Node::agent("a", "a").with_output_slot("out.a"));
        let _ = a;

        // First run records the call.
        let out1 = rt.run(&g, RunConfig::default()).unwrap();
        assert_eq!(out1.status, RunStatus::Completed);

        // Build a replayer from the recorded run.
        let rec_json = rt.record.lock().to_json();
        let rp = Replayer::from_json(&rec_json).unwrap();
        assert_eq!(rp.response_for("k1").unwrap(), Value::int(42).to_json());
    }
}
