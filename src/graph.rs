//! Graph topology (layer 1, part 1).
//!
//! A `Graph` is a set of `Node`s connected by `Edge`s. Nodes carry an agent
//! (or a nested subgraph, a human request, or a negotiation) and an optional
//! loop spec. Edges carry an optional condition (a predicate over the
//! blackboard) so the executor can route dynamically.
//!
//! This is the topology half of layer 1; iteration lives in `loop_spec` and
//! execution in `executor`.

use crate::error::AgentResult;
use crate::id::{EdgeId, GraphId, NodeId};
use crate::loop_spec::LoopSpec;
use crate::value::Value;
use std::collections::HashMap;

/// What a node represents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Runs an agent (by agent id label, resolved by the runtime).
    Agent(String),
    /// A nested subgraph (hierarchical composition). Runs a separate graph.
    Subgraph(GraphId),
    /// A human-in-the-loop review node (layer 13).
    Human(String),
    /// A negotiation between two or more agents (layer 9).
    Negotiation(String),
    /// A pure routing / branching node with no agent — just follows edges.
    Router,
}

/// A node in the graph.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    /// Optional loop wrapping this node (layer 1 iteration).
    pub loop_spec: Option<LoopSpec>,
    /// The blackboard slot this node writes its output to.
    pub output_slot: Option<String>,
    /// Priority for budget-aware scheduling (higher = more important).
    pub priority: i32,
}

impl Node {
    pub fn agent(label: impl Into<String>, agent_label: impl Into<String>) -> Self {
        Self {
            id: NodeId::new(),
            label: label.into(),
            kind: NodeKind::Agent(agent_label.into()),
            loop_spec: None,
            output_slot: None,
            priority: 0,
        }
    }

    pub fn router(label: impl Into<String>) -> Self {
        Self {
            id: NodeId::new(),
            label: label.into(),
            kind: NodeKind::Router,
            loop_spec: None,
            output_slot: None,
            priority: 0,
        }
    }

    pub fn with_output_slot(mut self, slot: impl Into<String>) -> Self {
        self.output_slot = Some(slot.into());
        self
    }

    pub fn with_loop(mut self, spec: LoopSpec) -> Self {
        self.loop_spec = Some(spec);
        self
    }

    pub fn with_priority(mut self, p: i32) -> Self {
        self.priority = p;
        self
    }
}

/// Edge kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Always follow on success.
    Normal,
    /// Follow only if the source output satisfied the condition.
    Conditional,
    /// Follow on failure / interrupt (exception edge).
    OnError,
}

/// An edge between two nodes. The condition is a predicate over a blackboard
/// snapshot `Value` and the source node's output `Value`.
pub struct Edge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub condition: Option<Box<dyn Fn(&Value, &Value) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for Edge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Edge")
            .field("id", &self.id)
            .field("from", &self.from)
            .field("to", &self.to)
            .field("kind", &self.kind)
            .field("has_condition", &self.condition.is_some())
            .finish()
    }
}

impl Edge {
    pub fn normal(from: NodeId, to: NodeId) -> Self {
        Self {
            id: EdgeId::new(),
            from,
            to,
            kind: EdgeKind::Normal,
            condition: None,
        }
    }

    pub fn conditional(
        from: NodeId,
        to: NodeId,
        cond: impl Fn(&Value, &Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: EdgeId::new(),
            from,
            to,
            kind: EdgeKind::Conditional,
            condition: Some(Box::new(cond)),
        }
    }

    pub fn on_error(from: NodeId, to: NodeId) -> Self {
        Self {
            id: EdgeId::new(),
            from,
            to,
            kind: EdgeKind::OnError,
            condition: None,
        }
    }

    /// Evaluate the edge's condition (defaults to true if none).
    pub fn passes(&self, snapshot: &Value, output: &Value) -> bool {
        match &self.condition {
            Some(c) => c(snapshot, output),
            None => true,
        }
    }
}

/// The graph.
#[derive(Default)]
pub struct Graph {
    pub id: GraphId,
    pub name: String,
    nodes: HashMap<NodeId, Node>,
    /// Outgoing edges per node.
    out_edges: HashMap<NodeId, Vec<Edge>>,
    /// Entry node ids.
    pub entries: Vec<NodeId>,
}

impl Graph {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: GraphId::new(),
            name: name.into(),
            nodes: HashMap::new(),
            out_edges: HashMap::new(),
            entries: Vec::new(),
        }
    }

    pub fn add_node(&mut self, node: Node) -> NodeId {
        let id = node.id;
        if self.nodes.is_empty() && self.entries.is_empty() {
            // First node added becomes the default entry.
            self.entries.push(id);
        }
        self.nodes.insert(id, node);
        self.out_edges.entry(id).or_default();
        id
    }

    pub fn add_edge(&mut self, edge: Edge) -> AgentResult<EdgeId> {
        if !self.nodes.contains_key(&edge.from) {
            return Err(crate::error::AgentError::Graph(format!("edge from unknown node {:?}", edge.from)));
        }
        if !self.nodes.contains_key(&edge.to) {
            return Err(crate::error::AgentError::Graph(format!("edge to unknown node {:?}", edge.to)));
        }
        let id = edge.id;
        self.out_edges.entry(edge.from).or_default().push(edge);
        Ok(id)
    }

    pub fn set_entry(&mut self, id: NodeId) {
        self.entries = vec![id];
    }

    pub fn add_entry(&mut self, id: NodeId) {
        if !self.entries.contains(&id) {
            self.entries.push(id);
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn out_edges(&self, id: NodeId) -> &[Edge] {
        self.out_edges.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.out_edges.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_small_graph() {
        let mut g = Graph::new("demo");
        let a = g.add_node(Node::agent("a", "echo"));
        let b = g.add_node(Node::agent("b", "echo").with_output_slot("out.b"));
        g.add_edge(Edge::normal(a, b)).unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert_eq!(g.out_edges(a).len(), 1);
        assert_eq!(g.entries, vec![a]);
    }

    #[test]
    fn conditional_edge_evaluates() {
        let mut g = Graph::new("cond");
        let a = g.add_node(Node::agent("a", "echo"));
        let b = g.add_node(Node::agent("b", "echo"));
        let c = g.add_node(Node::agent("c", "echo"));
        g.add_edge(Edge::conditional(a, b, |_, o| matches!(o, Value::Bool(true)))).unwrap();
        g.add_edge(Edge::conditional(a, c, |_, o| matches!(o, Value::Bool(false)))).unwrap();
        let edges = g.out_edges(a);
        assert!(edges[0].passes(&Value::null(), &Value::bool(true)));
        assert!(!edges[1].passes(&Value::null(), &Value::bool(true)));
    }

    #[test]
    fn edge_to_unknown_node_errors() {
        let mut g = Graph::new("bad");
        let a = g.add_node(Node::agent("a", "echo"));
        let err = g.add_edge(Edge::normal(a, NodeId::new())).unwrap_err();
        assert!(matches!(err, crate::error::AgentError::Graph(_)));
    }
}
