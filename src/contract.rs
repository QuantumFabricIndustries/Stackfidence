//! Contracts between agents (layer 9, part 1).
//!
//! When A calls B in most frameworks, it's "do this". There's no protocol for B
//! to push back on scope, request more context, refuse, or renegotiate. A
//! `Contract` is the formal unit of that transaction: it states what the
//! requester wants, what the provider agrees to deliver, the acceptance
//! criteria, and the current status. The negotiation protocol (negotiation.rs)
//! drives bid/offer/accept/reject/renegotiate over a contract.

use crate::error::AgentResult;
use crate::id::NegotiationId;
use crate::value::Value;
use parking_lot::Mutex;

/// Where a contract is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractStatus {
    /// Requester has issued a bid; no provider has accepted yet.
    Proposed,
    /// A provider made a counter-offer.
    CounterOffered,
    /// Both parties agreed.
    Accepted,
    /// A party rejected.
    Rejected,
    /// An accepted contract was reopened for renegotiation.
    Renegotiating,
    /// The provider delivered the agreed output.
    Fulfilled,
    /// The provider failed to deliver.
    Breached,
}

/// A contract.
pub struct Contract {
    pub id: NegotiationId,
    /// What the requester wants (a description + expected output shape).
    pub request: Value,
    /// What the provider currently offers (may differ from request).
    pub offer: Mutex<Value>,
    /// Acceptance criteria: a predicate over the delivered output.
    pub accept: Mutex<Box<dyn Fn(&Value) -> bool + Send + Sync>>,
    pub status: Mutex<ContractStatus>,
    /// The delivered output, once fulfilled.
    pub delivered: Mutex<Option<Value>>,
}

impl std::fmt::Debug for Contract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contract")
            .field("id", &self.id)
            .field("status", &*self.status.lock())
            .finish()
    }
}

impl Contract {
    pub fn new(
        request: Value,
        accept: impl Fn(&Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: NegotiationId::new(),
            request,
            offer: Mutex::new(Value::null()),
            accept: Mutex::new(Box::new(accept)),
            status: Mutex::new(ContractStatus::Proposed),
            delivered: Mutex::new(None),
        }
    }

    /// Provider makes a counter-offer.
    pub fn counter(&self, offer: Value) {
        *self.offer.lock() = offer;
        *self.status.lock() = ContractStatus::CounterOffered;
    }

    /// Requester accepts the current offer.
    pub fn accept(&self) {
        *self.status.lock() = ContractStatus::Accepted;
    }

    /// A party rejects.
    pub fn reject(&self) {
        *self.status.lock() = ContractStatus::Rejected;
    }

    /// Reopen for renegotiation.
    pub fn renegotiate(&self) {
        *self.status.lock() = ContractStatus::Renegotiating;
    }

    /// Provider delivers output. Fulfilled if acceptance passes, else Breached.
    pub fn deliver(&self, output: Value) -> AgentResult<()> {
        let ok = (self.accept.lock())(&output);
        if ok {
            *self.delivered.lock() = Some(output);
            *self.status.lock() = ContractStatus::Fulfilled;
            Ok(())
        } else {
            *self.status.lock() = ContractStatus::Breached;
            Err(crate::error::AgentError::Negotiation(
                "delivered output failed acceptance criteria".into(),
            ))
        }
    }

    pub fn status(&self) -> ContractStatus {
        *self.status.lock()
    }

    pub fn delivered(&self) -> Option<Value> {
        self.delivered.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fulfilled_when_accepted() {
        let c = Contract::new(Value::str("sum to 10"), |v| matches!(v, Value::Int(i) if *i == 10));
        c.counter(Value::str("I'll sum to 10"));
        c.accept();
        c.deliver(Value::int(10)).unwrap();
        assert_eq!(c.status(), ContractStatus::Fulfilled);
    }

    #[test]
    fn breached_when_criteria_fail() {
        let c = Contract::new(Value::str("sum to 10"), |v| matches!(v, Value::Int(i) if *i == 10));
        c.accept();
        let err = c.deliver(Value::int(9)).unwrap_err();
        assert!(matches!(err, crate::error::AgentError::Negotiation(_)));
        assert_eq!(c.status(), ContractStatus::Breached);
    }

    #[test]
    fn counter_then_reject() {
        let c = Contract::new(Value::str("x"), |_| true);
        c.counter(Value::str("y"));
        assert_eq!(c.status(), ContractStatus::CounterOffered);
        c.reject();
        assert_eq!(c.status(), ContractStatus::Rejected);
    }
}
