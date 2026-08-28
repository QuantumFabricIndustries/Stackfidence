//! Negotiation protocol between agents (layer 9, part 2).
//!
//! Drives bid / offer / accept / reject / renegotiate over a `Contract`. This
//! is what turns hierarchical dispatch into actual cooperation: a provider can
//! push back on scope, request more context, or refuse — and the requester can
//! counter or escalate. Sits on top of the coordination substrate (layer 2) but
//! is a distinct concern: it's about *how agents transact*, not where state
//! lives.

use crate::contract::{Contract, ContractStatus};
use crate::error::{AgentError, AgentResult};
use crate::value::Value;

/// What a provider wants to do this round.
#[derive(Clone, Debug)]
pub enum ProviderAction {
    /// Accept the current offer and deliver this output.
    Accept(Value),
    /// Make a counter-offer and keep negotiating.
    Counter(Value),
    /// Reject and walk away.
    Reject(String),
}

/// The outcome of a negotiation.
#[derive(Clone, Debug)]
pub enum NegotiationOutcome {
    /// Both parties agreed and the provider delivered.
    Fulfilled(Value),
    /// The provider delivered but it failed acceptance (breach).
    Breached(String),
    /// A party rejected and no deal was reached (carries the reason).
    Rejected(String),
    /// Could not reach a deal within the iteration cap.
    Deadlocked,
}

/// A negotiation session: the requester and a provider take turns making
/// offers until one is accepted and delivered, or the cap is hit.
pub struct Negotiation {
    pub contract: Contract,
    /// Max back-and-forth rounds before declaring deadlock.
    pub max_rounds: u32,
}

impl Negotiation {
    pub fn new(contract: Contract, max_rounds: u32) -> Self {
        Self { contract, max_rounds }
    }

    /// Run a negotiation given a provider function. The provider is called each
    /// round with (round, last_offer) and returns a `ProviderAction`.
    pub fn run<F>(&self, mut provider: F) -> AgentResult<NegotiationOutcome>
    where
        F: FnMut(u32, &Value) -> ProviderAction,
    {
        let mut round = 0u32;
        let mut last_offer = self.contract.request.clone();

        loop {
            if round >= self.max_rounds {
                return Ok(NegotiationOutcome::Deadlocked);
            }
            match provider(round, &last_offer) {
                ProviderAction::Accept(output) => {
                    self.contract.accept();
                    return match self.contract.deliver(output) {
                        Ok(()) => Ok(NegotiationOutcome::Fulfilled(
                            self.contract.delivered().unwrap_or(Value::null()),
                        )),
                        Err(e) => Ok(NegotiationOutcome::Breached(format!("{}", e))),
                    };
                }
                ProviderAction::Counter(offer) => {
                    self.contract.counter(offer.clone());
                    last_offer = offer;
                    round += 1;
                }
                ProviderAction::Reject(reason) => {
                    self.contract.reject();
                    return Ok(NegotiationOutcome::Rejected(reason));
                }
            }
        }
    }

    pub fn status(&self) -> ContractStatus {
        self.contract.status()
    }
}

/// Convenience: a one-shot negotiation where the provider immediately delivers.
pub fn one_shot(contract: Contract, output: Value) -> AgentResult<NegotiationOutcome> {
    let neg = Negotiation::new(contract, 1);
    let out = output.clone();
    neg.run(move |_r, _offer| ProviderAction::Accept(out.clone()))
}

/// Convenience error: deadlock.
pub fn deadlock_error() -> AgentError {
    AgentError::Negotiation("negotiation deadlocked".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fulfilled_in_one_round() {
        let c = Contract::new(Value::str("compute 10"), |v| matches!(v, Value::Int(i) if *i == 10));
        let neg = Negotiation::new(c, 3);
        let out = neg.run(|_r, _o| ProviderAction::Accept(Value::int(10))).unwrap();
        match out {
            NegotiationOutcome::Fulfilled(v) => assert_eq!(v, Value::int(10)),
            _ => panic!("expected fulfilled"),
        }
    }

    #[test]
    fn rejected_when_provider_errors() {
        let c = Contract::new(Value::str("x"), |_| true);
        let neg = Negotiation::new(c, 3);
        let out = neg.run(|_r, _o| ProviderAction::Reject("nope".into())).unwrap();
        assert!(matches!(out, NegotiationOutcome::Rejected(_)));
    }

    #[test]
    fn breached_when_output_fails_criteria() {
        let c = Contract::new(Value::str("compute 10"), |v| matches!(v, Value::Int(i) if *i == 10));
        let neg = Negotiation::new(c, 3);
        let out = neg.run(|_r, _o| ProviderAction::Accept(Value::int(9))).unwrap();
        assert!(matches!(out, NegotiationOutcome::Breached(_)));
    }

    #[test]
    fn counter_then_accept() {
        let c = Contract::new(Value::str("compute 10"), |v| matches!(v, Value::Int(i) if *i == 10));
        let neg = Negotiation::new(c, 3);
        let out = neg.run(|r, _o| {
            if r == 0 {
                ProviderAction::Counter(Value::str("I can do 10"))
            } else {
                ProviderAction::Accept(Value::int(10))
            }
        }).unwrap();
        assert!(matches!(out, NegotiationOutcome::Fulfilled(_)));
    }

    #[test]
    fn deadlocks_when_cap_hit() {
        let c = Contract::new(Value::str("x"), |_| true);
        let neg = Negotiation::new(c, 2);
        let out = neg.run(|_r, _o| ProviderAction::Counter(Value::str("again"))).unwrap();
        assert!(matches!(out, NegotiationOutcome::Deadlocked));
    }
}
