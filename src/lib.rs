//! `agent_stack` — a complete agent orchestration framework.
//!
//! Most users want `agent_stack::prelude::*`.
//!
//! The framework is organised into 13 layers, each in its own module. Layers
//! 1–2 give you dynamic behaviour (topology + iteration + shared state). Layers
//! 3–13 give you controllable, explainable, safe behaviour at scale.

pub mod error;
pub mod id;
pub mod value;

// Layer 1: graph + loops
pub mod graph;
pub mod loop_spec;
pub mod executor;

// Layer 2: coordination substrate
pub mod blackboard;
pub mod policy;

// Layer 3: causal memory
pub mod causal;
pub mod trace_store;

// Layer 4: trust propagation
pub mod trust;

// Layer 5: goal/intent model
pub mod goal;

// Layer 6: meta-cognition
pub mod meta;

// Layer 7: resource budget
pub mod budget;

// Layer 8: interrupt / exception propagation
pub mod interrupt;

// Layer 9: negotiation / contracting
pub mod contract;
pub mod negotiation;

// Layer 10: determinism / replayability
pub mod record;
pub mod replay;

// Layer 11: adversarial input validation
pub mod validate;

// Layer 12: decay / forgetting
pub mod memory;

// Layer 13: human-in-the-loop
pub mod human;

// Agent trait + runtime
pub mod agent;
pub mod mock_agent;
pub mod runtime;

#[cfg(feature = "llm")]
pub mod llm_agent;

pub mod prelude;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
