//! Access policy for the blackboard (part of layer 2).
//!
//! Defines who may read / write / lock which slots. The executor consults the
//! policy before handing a context to an agent, and the blackboard enforces it
//! on every access. Without this, "shared state" degenerates into a global
//! mutable dict that anyone clobbers.

use crate::id::AgentId;
use parking_lot::Mutex;

/// A permission on a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    Read,
    Write,
    Lock,
}

/// A rule: agent `agent` has `perm` on slot pattern `pattern`.
///
/// Patterns support a trailing `*` wildcard, e.g. `results.*` matches
/// `results.a` and `results.b`.
#[derive(Clone, Debug)]
pub struct Rule {
    pub agent: AgentId,
    pub pattern: String,
    pub perm: Permission,
}

/// A policy is a set of rules, evaluated in order; first match wins.
/// An empty policy denies everything (fail-closed).
#[derive(Default)]
pub struct AccessPolicy {
    rules: Mutex<Vec<Rule>>,
    /// If true, missing rules default to Allow (fail-open). Default false.
    default_allow: Mutex<bool>,
}

impl AccessPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fail-open mode: grants access when no rule matches. Use only for
    /// permissive dev setups.
    pub fn with_default_allow(self, allow: bool) -> Self {
        *self.default_allow.lock() = allow;
        self
    }

    pub fn add(&self, agent: AgentId, pattern: impl Into<String>, perm: Permission) {
        self.rules.lock().push(Rule {
            agent,
            pattern: pattern.into(),
            perm,
        });
    }

    /// Grant all permissions on a pattern to an agent.
    pub fn grant_all(&self, agent: AgentId, pattern: impl Into<String>) {
        let p = pattern.into();
        for perm in [Permission::Read, Permission::Write, Permission::Lock] {
            self.rules.lock().push(Rule {
                agent,
                pattern: p.clone(),
                perm,
            });
        }
    }

    pub fn check(&self, agent: AgentId, slot: &str, perm: Permission) -> bool {
        let rules = self.rules.lock();
        for r in rules.iter() {
            if r.agent == agent && r.perm == perm && pattern_matches(&r.pattern, slot) {
                return true;
            }
        }
        *self.default_allow.lock()
    }
}

fn pattern_matches(pattern: &str, slot: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        slot.starts_with(prefix)
    } else {
        pattern == slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_closed_by_default() {
        let p = AccessPolicy::new();
        let a = AgentId::new();
        assert!(!p.check(a, "x", Permission::Read));
    }

    #[test]
    fn wildcard_grant() {
        let p = AccessPolicy::new();
        let a = AgentId::new();
        p.grant_all(a, "results.*");
        assert!(p.check(a, "results.a", Permission::Read));
        assert!(p.check(a, "results.b", Permission::Write));
        assert!(!p.check(a, "other", Permission::Read));
    }

    #[test]
    fn exact_match() {
        let p = AccessPolicy::new();
        let a = AgentId::new();
        p.add(a, "count", Permission::Write);
        assert!(p.check(a, "count", Permission::Write));
        assert!(!p.check(a, "count", Permission::Read));
    }
}
