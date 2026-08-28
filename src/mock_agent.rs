//! A deterministic mock agent — the framework's built-in agent.
//!
//! No LLM is required to run the framework: register `MockAgent`s (or your own
//! `Agent` impls) and the runtime executes graphs deterministically. This is
//! what makes the framework testable and what makes replay (layer 10) exact.
//!
//! A `MockAgent` is configured with a pure function from input to output, so
//! given the same input it always produces the same output.

use crate::agent::{AgentContext, AgentOutput};
use crate::error::AgentResult;
use crate::id::AgentId;
use crate::meta::Confidence;
use crate::value::Value;
use std::sync::Arc;

/// A pure-function mock agent.
pub struct MockAgent {
    id: AgentId,
    name: String,
    func: Arc<dyn Fn(&Value) -> Value + Send + Sync>,
    confidence: Confidence,
}

impl MockAgent {
    pub fn new(name: impl Into<String>, func: impl Fn(&Value) -> Value + Send + Sync + 'static) -> Self {
        Self {
            id: AgentId::new(),
            name: name.into(),
            func: Arc::new(func),
            confidence: Confidence::high(),
        }
    }

    pub fn with_confidence(mut self, c: Confidence) -> Self {
        self.confidence = c;
        self
    }

    /// A mock that always returns a constant value.
    pub fn constant(name: impl Into<String>, value: Value) -> Self {
        let v = value.clone();
        Self::new(name, move |_| v.clone())
    }

    /// A mock that returns its input unchanged.
    pub fn identity(name: impl Into<String>) -> Self {
        Self::new(name, |v| v.clone())
    }

    /// A mock that reads a blackboard slot and returns it.
    pub fn read_slot(name: impl Into<String>, slot: impl Into<String>) -> Self {
        let slot = slot.into();
        // Note: this can't read the blackboard from a pure function; use
        // `BlackboardReader` instead for that.
        let _ = slot;
        Self::identity(name)
    }
}

impl crate::agent::Agent for MockAgent {
    fn id(&self) -> AgentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
        let v = (self.func)(&ctx.input);
        Ok(AgentOutput::done_with(v, self.confidence))
    }
}

/// A mock agent that reads a blackboard slot and returns it (needs the context).
pub struct BlackboardReader {
    id: AgentId,
    name: String,
    slot: String,
}

impl BlackboardReader {
    pub fn new(name: impl Into<String>, slot: impl Into<String>) -> Self {
        Self { id: AgentId::new(), name: name.into(), slot: slot.into() }
    }
}

impl crate::agent::Agent for BlackboardReader {
    fn id(&self) -> AgentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
        let v = ctx.blackboard.read(ctx.agent, &crate::blackboard::SlotKey::new(&self.slot))?;
        Ok(AgentOutput::done(v))
    }
}

/// A mock agent that writes a value to a blackboard slot and returns it.
pub struct BlackboardWriter {
    id: AgentId,
    name: String,
    slot: String,
    value: Value,
}

impl BlackboardWriter {
    pub fn new(name: impl Into<String>, slot: impl Into<String>, value: Value) -> Self {
        Self { id: AgentId::new(), name: name.into(), slot: slot.into(), value }
    }
}

impl crate::agent::Agent for BlackboardWriter {
    fn id(&self) -> AgentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
        ctx.blackboard
            .write(ctx.agent, &crate::blackboard::SlotKey::new(&self.slot), self.value.clone())?;
        Ok(AgentOutput::done(self.value.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Runtime;

    #[test]
    fn mock_constant_returns_value() {
        let rt = Runtime::permissive();
        rt.register("c", MockAgent::constant("c", Value::int(99)));
        let mut g = crate::graph::Graph::new("t");
        let n = g.add_node(crate::graph::Node::agent("c", "c").with_output_slot("out.c"));
        let _ = n;
        let out = rt.run(&g, crate::executor::RunConfig::default()).unwrap();
        assert_eq!(out.final_snapshot.get("out.c").cloned().unwrap(), Value::int(99));
    }

    #[test]
    fn blackboard_writer_then_reader() {
        let rt = Runtime::permissive();
        rt.register(
            "w",
            BlackboardWriter::new("w", "shared", Value::str("hello")),
        );
        rt.register("r", BlackboardReader::new("r", "shared"));
        let mut g = crate::graph::Graph::new("t");
        let w = g.add_node(crate::graph::Node::agent("w", "w").with_output_slot("out.w"));
        let r = g.add_node(crate::graph::Node::agent("r", "r").with_output_slot("out.r"));
        g.add_edge(crate::graph::Edge::normal(w, r)).unwrap();
        let out = rt.run(&g, crate::executor::RunConfig::default()).unwrap();
        assert_eq!(
            out.final_snapshot.get("out.r").cloned().unwrap(),
            Value::str("hello")
        );
    }
}
