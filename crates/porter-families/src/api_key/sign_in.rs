//! The ApiKey sign-in flow: one hidden field, the models listing as the check, a review. A provider
//! file that names no server (`openai-compatible`) also asks for the server's address, and its
//! key may be left empty.

use crate::conversation::{Flow, Move};
use crate::key::{asks_server, check, check_server, fault_of, own_server, signed, signed_server};
use porter_core::{Family, LoginName, SecretText, ServiceEndpoint, Tls, server_address_shape};
use porter_http::Http;
use porter_http_reqwest::ReqwestHttp;
use porter_ids::UrlScheme;
use porter_provider::{ProviderSpec, SignInMode, SignInStep, Signed};
use porter_sheet::{
    Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, SignInFault, SignInInput,
};
use std::sync::Arc;

/// The ApiKey sign-in flow.
pub struct ApiKeyFlow<H = ReqwestHttp> {
    spec: ProviderSpec,
    http: Arc<H>,
    mode: SignInMode,
}

impl<H> std::fmt::Debug for ApiKeyFlow<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyFlow")
            .field("spec", &self.spec.id)
            .finish_non_exhaustive()
    }
}

impl<H> ApiKeyFlow<H> {
    pub(super) fn new(spec: ProviderSpec, http: Arc<H>, mode: SignInMode) -> Self {
        Self { spec, http, mode }
    }

    /// The server the account already has, when it signs in again.
    fn held_server(&self) -> Option<ServiceEndpoint> {
        match &self.mode {
            SignInMode::Reauthenticate { endpoints, .. } => own_server(endpoints).cloned(),
            SignInMode::Add => None,
            // a variant a newer porter adds: no held server, so the person is asked for one
            _ => None,
        }
    }

    /// The form: the key, and for a file that names no server the server's address before it.
    /// A server of the person's own may need no key.
    fn form(&self) -> Vec<FieldSpec> {
        let key = |presence| FieldSpec {
            kind: FieldKind::ApiKey,
            entry: Entry::Secret,
            presence,
            prefill: None,
        };
        match (asks_server(&self.spec), self.held_server()) {
            (false, _) => vec![key(Presence::Required)],
            (true, Some(_)) => vec![key(Presence::Optional)],
            (true, None) => vec![
                FieldSpec {
                    kind: FieldKind::ServerUrl,
                    entry: Entry::Plain,
                    presence: Presence::Required,
                    prefill: None,
                },
                key(Presence::Optional),
            ],
        }
    }

    fn finished(&self, signed: Signed) -> Move<()> {
        match self.mode {
            SignInMode::Add => Move::Review(signed),
            SignInMode::Reauthenticate { .. } => Move::Done(signed),
            // a variant a newer porter adds: the sign-in ends as unreadable
            _ => Move::Fail(SignInFault::Unreadable),
        }
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

/// The server's address as typed. The sheet's machine has already checked it against the rule
/// of `server_address` (it alone knows the person's own computers); this reads it, and only
/// requires a web address.
fn typed_server(answers: &[FieldAnswer]) -> Option<ServiceEndpoint> {
    let text = answers
        .iter()
        .find(|a| a.kind == FieldKind::ServerUrl)
        .and_then(|a| match &a.value {
            FieldValue::Plain(text) => Some(text.trim().to_owned()),
            FieldValue::Secret(_) => None,
        })?;
    let url = server_address_shape(&text).ok()?;
    let tls = match url.origin().scheme {
        UrlScheme::Https => Tls::Implicit,
        _ => Tls::Plain,
    };
    Some(ServiceEndpoint {
        family: Family::ChatCompletions,
        url,
        tls,
        login: LoginName(String::new()),
    })
}

/// A company's key, checked at the company's address.
async fn company_signed<H: Http>(
    http: &H,
    spec: &ProviderSpec,
    answers: &[FieldAnswer],
) -> Result<Signed, SignInFault> {
    let key = typed_key(answers).ok_or(SignInFault::Unreadable)?;
    check(http, spec, &key).await.map_err(fault_of)?;
    Ok(signed(spec, key))
}

/// A server of the person's own, checked at the address they gave (or the account's own). No
/// key is an empty key, which is presented as none.
async fn server_signed<H: Http>(
    http: &H,
    spec: &ProviderSpec,
    held: Option<ServiceEndpoint>,
    answers: &[FieldAnswer],
) -> Result<Signed, SignInFault> {
    let server = held
        .or_else(|| typed_server(answers))
        .ok_or(SignInFault::Unreadable)?;
    let key = typed_key(answers).unwrap_or_else(|| SecretText::new(String::new()));
    check_server(http, &server.url, &key)
        .await
        .map_err(fault_of)?;
    Ok(signed_server(spec, server, key))
}

impl<H: Http + 'static> ApiKeyFlow<H> {
    async fn submitted(&self, answers: Vec<FieldAnswer>) -> Move<()> {
        let signed = match asks_server(&self.spec) {
            true => server_signed(&*self.http, &self.spec, self.held_server(), &answers).await,
            false => company_signed(&*self.http, &self.spec, &answers).await,
        };
        match signed {
            Ok(signed) => self.finished(signed),
            Err(fault) => Move::Fail(fault),
        }
    }
}

impl<H: Http + 'static> Flow for ApiKeyFlow<H> {
    type Mid = ();

    async fn start(&mut self) -> Move<()> {
        Move::Next(SignInStep::AskFields(self.form()), ())
    }

    async fn resume(&mut self, (): (), input: SignInInput) -> Move<()> {
        match input {
            SignInInput::Fields(answers) => self.submitted(answers).await,
            _ => Move::Fail(SignInFault::Unreadable),
        }
    }
}
