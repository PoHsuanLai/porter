//! The AgentLogin sign-in flow: nothing is asked. It shows the agent and, once the person
//! confirms, ends in a `Signed` with no credential at all. (Signing in again goes through the
//! same two steps; the sheet confirms the review itself.)

use porter_core::AccountLabel;
use porter_core::Restriction;
use porter_family_kit::{Flow, Move};
use porter_provider::{ProviderSpec, Signed};
use porter_sheet::{SignInFault, SignInInput};

/// The AgentLogin sign-in flow.
#[derive(Debug)]
pub struct AgentLoginFlow {
    spec: ProviderSpec,
}

impl AgentLoginFlow {
    pub(super) fn new(spec: ProviderSpec) -> Self {
        Self { spec }
    }
}

impl Flow for AgentLoginFlow {
    type Mid = ();

    async fn start(&mut self) -> Move<()> {
        Move::Review(Signed::new(
            AccountLabel(self.spec.label.clone()),
            Vec::new(),
            super::claims(&self.spec),
            Vec::new(),
            Restriction::none(),
        ))
    }

    /// The flow never waits for an answer, so one is not the person's.
    async fn resume(&mut self, (): (), _input: SignInInput) -> Move<()> {
        Move::Fail(SignInFault::Unreadable)
    }
}
