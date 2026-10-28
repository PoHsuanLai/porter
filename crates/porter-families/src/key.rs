//! What the two cloud-AI families share: the account's one secret is an API key that never
//! leaves accountd, checked by listing the provider's models with it. The listing is only a
//! check (a 200 with a readable body is a live key); nothing in it is kept, so no model list
//! sits in accountd or on any bus: inferd fetches its own with the key `Peer.ResolveKey` gives it.
//!
//! One provider file names no server: `openai-compatible`, a server of the person's own at an
//! address they type. Its Llm row has no endpoint, so the sign-in asks for the address
//! ([`asks_server`]); the address is kept with the account as a `ChatCompletions` endpoint
//! ([`own_server`]) and the key, which such a server may not need, is kept as an empty key when
//! there is none, so the account always has its one secret. An empty key is presented as no key.

use porter_capability::Capability;
use porter_capability::CapabilityKind;
use porter_capability::{Claim, Offer, Provenance, Subject};
use porter_core::AccountId;
use porter_core::{
    AccountLabel, Audience, Credential, EndpointUrl, Family, IssuedToken, Restriction,
    SecretPurpose, SecretText, ServiceEndpoint,
};
use porter_http::http::Method;
use porter_http::http::header::{ACCEPT, AUTHORIZATION, HeaderName};
use porter_http::http::request::Builder;
use porter_http::{Http, SensitiveHeader, request_to};
use porter_provider::{Presented, ProviderError, ProviderSession, ProviderSpec, Signed};
use porter_sheet::SignInFault;

/// The key a presented credential holds, or `Unauthorized` when the account has none.
pub(crate) fn key_of(presented: &Presented) -> Result<&SecretText, ProviderError> {
    match presented {
        Presented::Credential(Credential::ApiKey(key)) => Ok(key),
        _ => Err(ProviderError::Unauthorized),
    }
}

/// The path of the one call that checks a key, under the file's chat endpoint: the models list,
/// except at OpenRouter, whose list is public and so accepts any key; its own `key` call (the
/// key's label and limits) needs a live one.
fn check_path(provider: &porter_core::ProviderId) -> &'static str {
    match provider.as_str() {
        "openrouter" => "key",
        _ => "models",
    }
}

/// The check's target: the check URL under the file's chat endpoint, and how the company
/// wants the key presented, which its wire family says.
fn target_of(spec: &ProviderSpec) -> Result<(EndpointUrl, Family), ProviderError> {
    let (base, family) = spec
        .capabilities
        .iter()
        .filter(|row| row.capability.kind() == CapabilityKind::Llm)
        .find_map(|row| Some((row.endpoint.as_ref()?, row.family)))
        .ok_or(ProviderError::Unreadable)?;
    let url = EndpointUrl::parse(&format!(
        "{}/{}",
        base.0.trim_end_matches('/'),
        check_path(&spec.id)
    ))
    .map_err(|_| ProviderError::Unreadable)?;
    Ok((url, family))
}

/// Whether the file names no server, so that the person types the address: no language-model row
/// has an endpoint.
pub(crate) fn asks_server(spec: &ProviderSpec) -> bool {
    spec.capabilities
        .iter()
        .filter(|row| row.capability.kind() == CapabilityKind::Llm)
        .all(|row| row.endpoint.is_none())
}

/// The address of the server an account of such a file was made for, as it is kept: its
/// `ChatCompletions` endpoint.
pub(crate) fn own_server(endpoints: &[ServiceEndpoint]) -> Option<&ServiceEndpoint> {
    endpoints
        .iter()
        .find(|endpoint| endpoint.family == Family::ChatCompletions)
}

/// The version header Anthropic's API requires with every call.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// `request` carrying `key` the way the company documents: `x-api-key` and `anthropic-version`
/// for Anthropic's Messages API, `x-goog-api-key` for Gemini, a bearer token for the
/// OpenAI-compatible rest (OpenAI, Moonshot). An empty key is no key: the request goes without
/// (a server of the person's own may need none).
pub(crate) fn present_key(request: Builder, family: Family, key: &SecretText) -> Builder {
    if key.expose().is_empty() {
        return request;
    }
    match family {
        Family::Messages => request
            .sensitive_header(HeaderName::from_static("x-api-key"), key.expose())
            .header("anthropic-version", ANTHROPIC_VERSION),
        Family::GenerateContent => {
            request.sensitive_header(HeaderName::from_static("x-goog-api-key"), key.expose())
        }
        _ => request.sensitive_header(AUTHORIZATION, &format!("Bearer {}", key.expose())),
    }
}

/// Lists the models with `key`. A refused key is `Unauthorized` (or `Forbidden`), a server that
/// is down or busy `Unreachable`, an answer that is not a list `Unreadable`. The list is dropped.
pub(crate) async fn check<H: Http>(
    http: &H,
    spec: &ProviderSpec,
    key: &SecretText,
) -> Result<(), ProviderError> {
    let (url, family) = target_of(spec)?;
    check_at(http, &url, family, key).await
}

/// The models list of the server of the person's own at `server`: `<server address>/models`.
pub(crate) fn models_url(server: &EndpointUrl) -> Result<EndpointUrl, ProviderError> {
    EndpointUrl::parse(&format!("{}/models", server.as_str().trim_end_matches('/')))
        .map_err(|_| ProviderError::Unreadable)
}

/// [`check`] of the server of the person's own at `server`, which speaks the OpenAI chat
/// protocol: `GET <server address>/models`, with the bearer key when there is one.
pub(crate) async fn check_server<H: Http>(
    http: &H,
    server: &EndpointUrl,
    key: &SecretText,
) -> Result<(), ProviderError> {
    check_at(http, &models_url(server)?, Family::ChatCompletions, key).await
}

/// The one call that checks a key, at `url`, presented the way `family` wants.
async fn check_at<H: Http>(
    http: &H,
    url: &EndpointUrl,
    family: Family,
    key: &SecretText,
) -> Result<(), ProviderError> {
    let request = request_to(Method::GET, url)
        .map_err(|_| ProviderError::Unreachable)?
        .header(ACCEPT, "application/json");
    // A key that cannot be a header's value is never sent; the request fails as an unreachable
    // server would, as it did when the client refused it.
    let request = present_key(request, family, key)
        .body(Vec::new())
        .map_err(|_| ProviderError::Unreachable)?;
    let response = http
        .send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)?;
    match response.status().as_u16() {
        200..=299 => match is_listing(response.body()) {
            true => Ok(()),
            false => Err(ProviderError::Unreadable),
        },
        401 => Err(ProviderError::Unauthorized),
        403 => Err(ProviderError::Forbidden),
        408 | 425 | 429 | 500..=599 => Err(ProviderError::Unreachable),
        _ => Err(ProviderError::Unreadable),
    }
}

/// Whether `body` looks like a models list or a key's description: a JSON object with `data` (OpenAI, Anthropic,
/// Moonshot) or `models` (Gemini). A cheap structural check; nothing is parsed or kept.
fn is_listing(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    let text = text.trim_start();
    text.starts_with('{') && (text.contains("\"data\"") || text.contains("\"models\""))
}

/// What a live key can do: the file's language-model rows, shown by a small real call, and the
/// agent programs it names.
pub(crate) fn claims(spec: &ProviderSpec) -> Vec<Claim> {
    let models = spec
        .capabilities
        .iter()
        .filter(|row| row.capability.kind() == CapabilityKind::Llm)
        .map(|row| Claim {
            subject: Subject::Account,
            offer: Offer::Present(row.capability.clone()),
            provenance: Provenance::Probed,
        });
    // The agent programs the file names may run on this key. Declared, not probed: the file says
    // so, and the key's one call checks the key, not the programs.
    let agents = spec
        .capabilities
        .iter()
        .filter_map(|row| match &row.capability {
            Capability::Agent(agent) => Some(Claim {
                subject: Subject::Agent(agent.program.clone()),
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            }),
            _ => None,
        });
    models.chain(agents).collect()
}

/// The finished sign-in of a live key.
pub(crate) fn signed(spec: &ProviderSpec, key: SecretText) -> Signed {
    Signed::new(
        AccountLabel(spec.label.clone()),
        vec![(SecretPurpose::ApiKey, Credential::ApiKey(key))],
        claims(spec),
        Vec::new(),
        Restriction::none(),
    )
}

/// The finished sign-in of a server of the person's own that answered: the account is named for
/// the server (`llm.example.test`, with its port when it is not the usual one), and keeps its
/// address.
pub(crate) fn signed_server(
    spec: &ProviderSpec,
    server: ServiceEndpoint,
    key: SecretText,
) -> Signed {
    let origin = server.url.origin();
    let label = match origin.port == origin.scheme.default_port() {
        true => origin.host.clone(),
        false => format!("{}:{}", origin.host, origin.port),
    };
    Signed::new(
        AccountLabel(label),
        vec![(SecretPurpose::ApiKey, Credential::ApiKey(key))],
        claims(spec),
        vec![server],
        Restriction::none(),
    )
}

/// What a failed call says to the person.
pub(crate) fn fault_of(error: ProviderError) -> SignInFault {
    match error {
        ProviderError::Unauthorized => SignInFault::Refused,
        ProviderError::Forbidden => SignInFault::Forbidden,
        ProviderError::Unreachable => SignInFault::Unreachable,
        ProviderError::Unreadable => SignInFault::Unreadable,
        // a variant a newer porter adds: the sign-in ends as unreadable
        _ => SignInFault::Unreadable,
    }
}

/// An open account of either family. An API key is never handed to an app, whatever the
/// audience: only `Peer.ResolveKey` reads it, and only for a porter daemon.
#[derive(Debug)]
pub struct KeySession {
    account: AccountId,
}

impl KeySession {
    pub(crate) fn new(account: AccountId) -> Self {
        Self { account }
    }

    /// The account it is open for.
    pub fn account(&self) -> &AccountId {
        &self.account
    }
}

impl ProviderSession for KeySession {
    async fn access_token(&self, _audience: &Audience) -> Result<IssuedToken, ProviderError> {
        Err(ProviderError::Forbidden)
    }

    fn renewed(&self) -> Option<Credential> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_is_an_object_with_data() {
        const CASES: &[(&str, bool)] = &[
            (r#"{"data":[]}"#, true),
            (r#"{"models":[{"name":"models/gemini"}]}"#, true),
            (r#"  {"object":"list","data":[{"id":"m"}]}"#, true),
            ("<html>captive portal</html>", false),
            (r#"{"error":{"message":"x"}}"#, false),
            ("", false),
        ];
        for (body, want) in CASES {
            assert_eq!(is_listing(body.as_bytes()), *want, "{body}");
        }
    }

    #[test]
    fn each_company_gets_its_documented_header() {
        let url = porter_ids::WebUrl::parse("https://api.example.org/v1/models").expect("url");
        let key = SecretText::new("KEY");
        let headers = |family| {
            present_key(porter_http::request(Method::GET, &url), family, &key)
                .body(Vec::<u8>::new())
                .expect("request")
                .headers()
                .iter()
                .map(|(name, value)| {
                    let value = value.to_str().expect("text").to_owned();
                    (name.as_str().to_owned(), value)
                })
                .collect::<Vec<_>>()
        };
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            headers(Family::Messages),
            [
                pair("x-api-key", "KEY"),
                pair("anthropic-version", "2023-06-01")
            ]
        );
        assert_eq!(
            headers(Family::GenerateContent),
            [pair("x-goog-api-key", "KEY")]
        );
        assert_eq!(
            headers(Family::ChatCompletions),
            [pair("authorization", "Bearer KEY")]
        );
    }

    #[test]
    fn a_status_is_a_fault_the_person_can_read() {
        const CASES: &[(ProviderError, SignInFault)] = &[
            (ProviderError::Unauthorized, SignInFault::Refused),
            (ProviderError::Forbidden, SignInFault::Forbidden),
            (ProviderError::Unreachable, SignInFault::Unreachable),
            (ProviderError::Unreadable, SignInFault::Unreadable),
        ];
        for (error, want) in CASES {
            assert_eq!(fault_of(*error), *want);
        }
    }
}
