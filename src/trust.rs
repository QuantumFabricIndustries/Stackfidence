//! Trust propagation (layer 4).
//!
//! When Agent A calls Agent B which calls Agent C, the permission context
//! collapses in most frameworks. There's no formal model for how authority,
//! constraints, or identity flow through a call chain. Here a `TrustContext`
//! travels with each invocation and *narrows* as it descends: a child agent
//! can never hold more authority than its parent, and constraints accumulate.
//! This is the AgentGuard problem, solved structurally rather than with ad-hoc
//! per-node checks.

use crate::id::AgentId;
use std::collections::HashSet;

/// A capability an agent may exercise, e.g. "network", "filesystem:write",
/// "spend:money", "call:external_api". Strings are free-form but should be
/// namespaced with `:` for clarity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Authority(pub String);

impl Authority {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// Does this authority grant `requested`? Supports prefix matching so
    /// `filesystem` grants `filesystem:write`.
    pub fn grants(&self, requested: &str) -> bool {
        let a = &self.0;
        a == requested || requested.starts_with(&format!("{}:", a))
    }
}

/// A constraint an agent must respect, e.g. "max_tokens:1000", "data_class:pii".
/// Constraints accumulate (union) as the context narrows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Constraint(pub String);

impl Constraint {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// The trust context inherited by an invocation.
#[derive(Clone, Debug)]
pub struct TrustContext {
    /// Who is invoking (the agent identity).
    pub actor: AgentId,
    /// Capabilities currently held. A child can only hold a *subset* of the
    /// parent's capabilities.
    pub authorities: HashSet<Authority>,
    /// Constraints accumulated down the chain.
    pub constraints: HashSet<Constraint>,
    /// Depth in the call chain (root = 0).
    pub depth: u32,
}

impl TrustContext {
    /// A root context with full authority (use sparingly — for the top-level
    /// orchestrator only).
    pub fn root(actor: AgentId, authorities: Vec<Authority>) -> Self {
        Self {
            actor,
            authorities: authorities.into_iter().collect(),
            constraints: HashSet::new(),
            depth: 0,
        }
    }

    /// A permissive root (all common authorities). For dev/test.
    pub fn permissive_root() -> Self {
        Self::root(
            AgentId::new(),
            vec![
                Authority::new("network"),
                Authority::new("filesystem"),
                Authority::new("compute"),
                Authority::new("call"),
            ],
        )
    }

    /// Does this context grant `requested` authority?
    pub fn has(&self, requested: &str) -> bool {
        self.authorities.iter().any(|a| a.grants(requested))
    }

    /// Narrow the context for a child invocation: the child gets the
    /// intersection of the parent's authorities and the explicitly granted set,
    /// plus any additional constraints. Depth increments.
    pub fn narrow(
        &self,
        child: AgentId,
        grant: &[Authority],
        extra_constraints: &[Constraint],
    ) -> TrustContext {
        let mut child_auth = HashSet::new();
        for a in grant {
            // Child can only receive authorities the parent actually grants
            // (prefix match: parent "filesystem" grants child "filesystem:write").
            if self.authorities.iter().any(|p| p.grants(&a.0)) {
                child_auth.insert(a.clone());
            }
        }
        let mut constraints = self.constraints.clone();
        for c in extra_constraints {
            constraints.insert(c.clone());
        }
        TrustContext {
            actor: child,
            authorities: child_auth,
            constraints,
            depth: self.depth + 1,
        }
    }

    /// Check a constraint is present.
    pub fn constrained_to(&self, c: &str) -> bool {
        self.constraints.iter().any(|x| x.0 == c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_prefix_grant() {
        let a = Authority::new("filesystem");
        assert!(a.grants("filesystem"));
        assert!(a.grants("filesystem:write"));
        assert!(!a.grants("network"));
    }

    #[test]
    fn narrow_reduces_authority() {
        let root = TrustContext::root(
            AgentId::new(),
            vec![Authority::new("filesystem"), Authority::new("network")],
        );
        let child = root.narrow(
            AgentId::new(),
            &[Authority::new("filesystem:write"), Authority::new("compute")],
            &[Constraint::new("max_tokens:100")],
        );
        // child gets filesystem:write (parent has filesystem) but NOT compute
        // (parent doesn't have it).
        assert!(child.has("filesystem:write"));
        assert!(!child.has("compute"));
        assert_eq!(child.depth, 1);
        assert!(child.constrained_to("max_tokens:100"));
    }

    #[test]
    fn constraints_accumulate() {
        let root = TrustContext::root(AgentId::new(), vec![Authority::new("compute")]);
        let c1 = root.narrow(AgentId::new(), &[], &[Constraint::new("a")]);
        let c2 = c1.narrow(AgentId::new(), &[], &[Constraint::new("b")]);
        assert!(c2.constrained_to("a"));
        assert!(c2.constrained_to("b"));
    }
}
