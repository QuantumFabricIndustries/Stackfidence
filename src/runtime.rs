//! The `Runtime` — wires all 13 layers together and resolves agents.
//!
//! The runtime owns the shared services (blackboard, budget, causal trace,
//! goal, interrupt bus, run record, memory store, input validator) and a
//! registry of agents keyed by label. The `GraphExecutor` (executor.rs) drives
//! a graph against a runtime.
//!
//! Design borrowed from VerifyStack's engine orchestrator: one object owns the
//! shared context (there: `RepoGraph`; here: the layer services) and the
//! per-run checks query it.

use crate::agent::{Agent, DynAgent};
use crate::blackboard::{Blackboard, SlotKey};
use crate::budget::Budget;
use crate::causal::CausalTrace;
use crate::error::{AgentError, AgentResult, Interrupt, InterruptKind};
use crate::executor::{GraphExecutor, RunConfig, RunOutcome};
use crate::goal::Goal;
use crate::graph::Graph;
use crate::id::RunId;
use crate::interrupt::InterruptBus;
use crate::memory::MemoryStore;
use crate::record::RunRecord;
use crate::trust::TrustContext;
use crate::validate::InputValidator;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// A handler for human-in-the-loop nodes (layer 13). Returns the human's
/// decision as a `Value` (the value to write to the output slot) or an error
/// to raise as a HumanRejected interrupt. Stored as `Arc` so it can be cloned
/// into a worker thread for real timeout enforcement.
pub type HumanHandler = Arc<dyn Fn(&str, &crate::value::Value) -> AgentResult<crate::value::Value> + Send + Sync>;

/// A handler for negotiation nodes (layer 9).
pub type NegotiationHandler = Box<dyn Fn(&str, &crate::value::Value) -> AgentResult<crate::value::Value> + Send + Sync>;

/// The runtime. Clone is cheap (all fields are Arc/shared).
pub struct Runtime {
    pub blackboard: Arc<Blackboard>,
    pub budget: Arc<Budget>,
    pub trace: Arc<CausalTrace>,
    pub goal: Arc<Goal>,
    pub interrupts: Arc<InterruptBus>,
    pub record: Arc<Mutex<RunRecord>>,
    pub memory: Arc<Mutex<MemoryStore>>,
    pub validator: Arc<InputValidator>,
    pub trust_root: TrustContext,
    agents: Mutex<HashMap<String, DynAgent>>,
    /// Registry of subgraphs for hierarchical composition (layer 1 / layer 9).
    subgraphs: Mutex<HashMap<crate::id::GraphId, Arc<Graph>>>,
    human: Mutex<Option<HumanHandler>>,
    /// Default timeout for human-in-the-loop nodes (layer 13). If a handler
    /// doesn't return within this duration, the node escalates (HumanRejected).
    human_timeout: Mutex<std::time::Duration>,
    negotiation: Mutex<Option<NegotiationHandler>>,
}

impl Runtime {
    /// Build a runtime with the given services and a permissive trust root.
    pub fn new(
        blackboard: Arc<Blackboard>,
        budget: Arc<Budget>,
        trace: Arc<CausalTrace>,
        goal: Arc<Goal>,
        validator: Arc<InputValidator>,
    ) -> Self {
        Self {
            blackboard,
            budget,
            trace,
            goal,
            interrupts: InterruptBus::shared(),
            record: RunRecord::shared(),
            memory: Arc::new(Mutex::new(MemoryStore::new())),
            validator,
            trust_root: TrustContext::permissive_root(),
            agents: Mutex::new(HashMap::new()),
            subgraphs: Mutex::new(HashMap::new()),
            human: Mutex::new(None),
            human_timeout: Mutex::new(std::time::Duration::from_secs(30)),
            negotiation: Mutex::new(None),
        }
    }

    /// A fully permissive runtime for tests / quick starts.
    pub fn permissive() -> Self {
        Self::new(
            Blackboard::permissive(),
            Arc::new(Budget::unlimited()),
            Arc::new(CausalTrace::new()),
            Goal::trivial(),
            Arc::new(InputValidator::new()),
        )
    }

    /// Register an agent under a label so `NodeKind::Agent(label)` can resolve it.
    pub fn register<A: Agent + 'static>(&self, label: impl Into<String>, agent: A) {
        self.agents.lock().insert(label.into(), crate::agent::boxed(agent));
    }

    /// Register a boxed agent.
    pub fn register_boxed(&self, label: impl Into<String>, agent: DynAgent) {
        self.agents.lock().insert(label.into(), agent);
    }

    /// Register a subgraph for hierarchical composition (layer 1).
    /// The graph's `id` is the key; `NodeKind::Subgraph(id)` nodes look it up.
    pub fn register_subgraph(&self, graph: Graph) -> crate::id::GraphId {
        let id = graph.id;
        self.subgraphs.lock().insert(id, Arc::new(graph));
        id
    }

    /// Resolve a registered subgraph by id.
    pub fn resolve_subgraph(&self, id: crate::id::GraphId) -> Option<Arc<Graph>> {
        self.subgraphs.lock().get(&id).cloned()
    }

    /// Resolve an agent by label (returns a clone of the Arc).
    pub fn resolve(&self, label: &str) -> Option<DynAgent> {
        self.agents.lock().get(label).cloned()
    }

    /// Look up an agent by label and run it (used by the executor).
    pub fn run_agent(&self, label: &str, ctx: &crate::agent::AgentContext) -> AgentResult<crate::agent::AgentOutput> {
        let agent = self.resolve(label).ok_or_else(|| {
            AgentError::Other(format!("no agent registered for label {:?}", label))
        })?;
        agent.run(ctx)
    }

    /// Install a human-in-the-loop handler.
    pub fn set_human_handler(&self, h: HumanHandler) {
        *self.human.lock() = Some(h);
    }

    /// Set the default timeout for human-in-the-loop nodes (layer 13). If a
    /// handler doesn't return within this duration, the node escalates.
    pub fn set_human_timeout(&self, timeout: std::time::Duration) {
        *self.human_timeout.lock() = timeout;
    }

    /// The current human-node timeout.
    pub fn human_timeout(&self) -> std::time::Duration {
        *self.human_timeout.lock()
    }

    /// Clone the human handler Arc (for use in a worker thread). Returns None
    /// if no handler is installed.
    pub fn human_handler_arc(&self) -> Option<HumanHandler> {
        self.human.lock().clone()
    }

    pub fn human_handler(&self) -> Option<HumanRef<'_>> {
        self.human.lock().is_some().then(|| HumanRef { runtime: self })
    }

    /// Install a negotiation handler.
    pub fn set_negotiation_handler(&self, h: NegotiationHandler) {
        *self.negotiation.lock() = Some(h);
    }

    pub fn negotiation_handler(&self) -> Option<NegRef<'_>> {
        self.negotiation.lock().is_some().then(|| NegRef { runtime: self })
    }

    /// Run a graph against this runtime.
    pub fn run(&self, graph: &Graph, config: RunConfig) -> AgentResult<RunOutcome> {
        // Seed the blackboard.
        for (k, v) in &config.seed {
            self.blackboard
                .write(self.trust_root.actor, &SlotKey::new(k), v.clone())?;
        }
        // Seed the run record too (for replay).
        {
            let mut rec = self.record.lock();
            rec.seed = config.seed.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
        }
        let mut exec = GraphExecutor::new(self);
        exec.run(graph, config)
    }

    /// The run id (a fresh one per run call would be ideal; here we expose a
    /// helper to mint one).
    pub fn new_run_id(&self) -> RunId {
        RunId::new()
    }
}

// --- handler access shims ---

/// Human handler handle.
pub struct HumanRef<'a> {
    pub runtime: &'a Runtime,
}
impl<'a> HumanRef<'a> {
    pub fn handle(&self, label: &str, input: &crate::value::Value) -> AgentResult<crate::value::Value> {
        let h = self.runtime.human.lock();
        let h = h.as_ref().ok_or_else(|| AgentError::Other("no human handler".into()))?;
        h(label, input)
    }
}

/// Negotiation handler handle.
pub struct NegRef<'a> {
    pub runtime: &'a Runtime,
}
impl<'a> NegRef<'a> {
    pub fn handle(&self, label: &str, input: &crate::value::Value) -> AgentResult<crate::value::Value> {
        let h = self.runtime.negotiation.lock();
        let h = h.as_ref().ok_or_else(|| AgentError::Other("no negotiation handler".into()))?;
        h(label, input)
    }
}

/// Helper to build an interrupt for a missing agent.
pub fn missing_agent_interrupt(label: &str) -> Interrupt {
    Interrupt::new(InterruptKind::Internal, format!("no agent for label {:?}", label))
}

// Re-export RunOutcome for convenience.
pub use crate::executor::RunOutcome as _RunOutcomeReexport;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissive_runtime_builds() {
        let rt = Runtime::permissive();
        assert!(rt.blackboard.is_empty());
    }
}
