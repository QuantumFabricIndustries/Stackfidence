//! Stable, serializable identifiers used across all layers.
//!
//! Every entity in a run (graph, node, edge, span, trace, run) gets an opaque,
//! copyable id. Using newtypes prevents mixing a `NodeId` with an `EdgeId` at
//! compile time — the kind of mistake that silently corrupts agent graphs.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Macro to declare a newtype id backed by a `Uuid` with the usual trait impls.
macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generate a fresh random id.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing uuid.
            pub fn from_uuid(u: Uuid) -> Self {
                Self(u)
            }

            /// The underlying uuid.
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}-{}", $prefix, self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a whole graph definition.
    GraphId,
    "graph"
);

id_type!(
    /// Identifies a node within a graph.
    NodeId,
    "node"
);

id_type!(
    /// Identifies an edge between two nodes.
    EdgeId,
    "edge"
);

id_type!(
    /// Identifies a single run (one execution of a graph).
    RunId,
    "run"
);

id_type!(
    /// Identifies a span within a causal trace (one node invocation).
    SpanId,
    "span"
);

id_type!(
    /// Identifies a causal trace as a whole.
    TraceId,
    "trace"
);

id_type!(
    /// Identifies a single agent instance.
    AgentId,
    "agent"
);

id_type!(
    /// Identifies a negotiation session between agents.
    NegotiationId,
    "neg"
);

id_type!(
    /// Identifies a human-review request.
    HumanRequestId,
    "human"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct() {
        let a = NodeId::new();
        let b = NodeId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn id_roundtrips_json() {
        let id = SpanId::new();
        let s = serde_json::to_string(&id).unwrap();
        let back: SpanId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn id_display_has_prefix() {
        let id = NodeId::from_uuid(Uuid::nil());
        assert!(id.to_string().starts_with("node-"));
    }
}
