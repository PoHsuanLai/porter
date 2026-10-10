//! The AgentLogin sign-in: nothing is asked. It shows the agent and, once the person confirms,
//! ends in a `Signed` with no credential at all. (Signing in again goes through the same two
//! steps; the sheet confirms the review itself.)

use porter_core::AccountLabel;
use porter_core::Restriction;
use porter_core::sheet::{SignInFault, SignInInput};
use porter_provider::{ProviderSpec, SignIn, SignInStep, Signed};

#[derive(Debug)]
enum State {
    Fresh,
    Reviewing,
    Ended,
}

/// The AgentLogin sign-in conversation.
#[derive(Debug)]
pub struct AgentLoginSignIn {
    spec: ProviderSpec,
    state: State,
}

impl AgentLoginSignIn {
    pub(super) fn new(spec: ProviderSpec) -> Self {
        Self {
            spec,
            state: State::Fresh,
        }
    }

    fn signed(&self) -> Signed {
        Signed::new(
            AccountLabel(self.spec.label.clone()),
            Vec::new(),
            super::claims(&self.spec),
            Vec::new(),
            Restriction::none(),
        )
    }
}

impl SignIn for AgentLoginSignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let state = std::mem::replace(&mut self.state, State::Ended);
        match (state, input) {
            (_, SignInInput::Cancel) => SignInStep::Failed(SignInFault::Cancelled),
            (State::Fresh, SignInInput::Start) => {
                self.state = State::Reviewing;
                let signed = self.signed();
                SignInStep::Review {
                    claims: signed.claims,
                    endpoints: signed.endpoints,
                    restriction: signed.restriction,
                    label: signed.label,
                }
            }
            (State::Reviewing, SignInInput::Confirm(_)) => SignInStep::Done(self.signed()),
            _ => SignInStep::Failed(SignInFault::Unreadable),
        }
    }
}
