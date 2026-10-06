//! The ApiKey sign-in: one hidden field, the models listing as the check, a review.

use crate::key::{check, fault_of, review_of, signed};
use porter_core::SecretText;
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, SignInFault, SignInInput,
};
use porter_http::{Http, HyperHttp};
use porter_provider::{ProviderSpec, SignIn, SignInMode, SignInStep, Signed};
use std::sync::Arc;

#[derive(Debug)]
enum State {
    Fresh,
    /// The form is on screen.
    Asked,
    /// The key works; the person reviews what it opens.
    Reviewing(Box<Signed>),
    Ended,
}

/// The ApiKey sign-in conversation.
pub struct ApiKeySignIn<H = HyperHttp> {
    spec: ProviderSpec,
    http: Arc<H>,
    mode: SignInMode,
    state: State,
}

impl<H> std::fmt::Debug for ApiKeySignIn<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeySignIn")
            .field("spec", &self.spec.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<H> ApiKeySignIn<H> {
    pub(super) fn new(spec: ProviderSpec, http: Arc<H>, mode: SignInMode) -> Self {
        Self {
            spec,
            http,
            mode,
            state: State::Fresh,
        }
    }

    fn failed(&mut self, fault: SignInFault) -> SignInStep {
        self.state = State::Ended;
        SignInStep::Failed(fault)
    }
}

/// The key as typed, without the spaces a paste brings along.
fn typed_key(answers: &[FieldAnswer]) -> Option<SecretText> {
    answers
        .iter()
        .find(|a| a.kind == FieldKind::ApiKey)
        .and_then(|a| match &a.value {
            FieldValue::Secret(secret) => Some(secret.expose().trim().to_owned()),
            FieldValue::Plain(_) => None,
        })
        .filter(|key| !key.is_empty())
        .map(SecretText::new)
}

impl<H: Http + 'static> ApiKeySignIn<H> {
    async fn submitted(&mut self, answers: Vec<FieldAnswer>) -> SignInStep {
        let Some(key) = typed_key(&answers) else {
            return self.failed(SignInFault::Unreadable);
        };
        if let Err(error) = check(&*self.http, &self.spec, &key).await {
            return self.failed(fault_of(error));
        }
        let signed = signed(&self.spec, key);
        match self.mode {
            SignInMode::Add => {
                let step = review_of(&signed);
                self.state = State::Reviewing(Box::new(signed));
                step
            }
            SignInMode::Reauthenticate { .. } => {
                self.state = State::Ended;
                SignInStep::Done(signed)
            }
        }
    }
}

impl<H: Http + 'static> SignIn for ApiKeySignIn<H> {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let state = std::mem::replace(&mut self.state, State::Ended);
        match (state, input) {
            (_, SignInInput::Cancel) => SignInStep::Failed(SignInFault::Cancelled),
            (_, SignInInput::Start) => {
                self.state = State::Asked;
                SignInStep::AskFields(vec![FieldSpec {
                    kind: FieldKind::ApiKey,
                    entry: Entry::Secret,
                    presence: Presence::Required,
                    prefill: None,
                }])
            }
            (State::Asked, SignInInput::Fields(answers)) => self.submitted(answers).await,
            (State::Reviewing(signed), SignInInput::Confirm(_)) => SignInStep::Done(*signed),
            _ => self.failed(SignInFault::Unreadable),
        }
    }
}
