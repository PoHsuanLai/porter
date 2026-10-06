//! What the two cloud-AI families share: the account's one secret is an API key that never
//! leaves accountd, checked by listing the provider's models with it. The listing is only a
//! check (a 200 with a readable body is a live key); nothing in it is kept, so no model list
//! sits in accountd or on any bus: inferd fetches its own with the key `Peer.ResolveKey` gives it.

use porter_core::capability::CapabilityKind;
use porter_core::sheet::SignInFault;
use porter_core::{
    AccountId, AccountLabel, Audience, Claim, Credential, EndpointUrl, Family, IssuedToken, Offer,
    Provenance, Restriction, SecretPurpose, SecretText, Subject,
};
use porter_http::{Http, HttpRequest, Method};
use porter_provider::{
    Presented, ProviderError, ProviderSession, ProviderSpec, SignInStep, Signed,
};

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

/// The version header Anthropic's API requires with every call.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// `request` carrying `key` the way the company documents: `x-api-key` and `anthropic-version`
/// for Anthropic's Messages API, `x-goog-api-key` for Gemini, a bearer token for the
/// OpenAI-compatible rest (OpenAI, Moonshot).
pub(crate) fn present_key(request: HttpRequest, family: Family, key: &SecretText) -> HttpRequest {
    match family {
        Family::Messages => request
            .with_header("x-api-key", key.expose())
            .with_header("anthropic-version", ANTHROPIC_VERSION),
        Family::GenerateContent => request.with_header("x-goog-api-key", key.expose()),
        _ => request.with_header("Authorization", format!("Bearer {}", key.expose())),
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
    let request = HttpRequest::to(Method::Get, &url)
        .map_err(|_| ProviderError::Unreachable)?
        .with_header("Accept", "application/json");
    let response = http
        .send(present_key(request, family, key))
        .await
        .map_err(|_| ProviderError::Unreachable)?;
    match response.status.0 {
        200..=299 => match is_listing(&response.body) {
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

/// What a live key can do: the file's language-model rows, shown by a small real call.
pub(crate) fn claims(spec: &ProviderSpec) -> Vec<Claim> {
    spec.capabilities
        .iter()
        .filter(|row| row.capability.kind() == CapabilityKind::Llm)
        .map(|row| Claim {
            subject: Subject::Account,
            offer: Offer::Present(row.capability.clone()),
            provenance: Provenance::Probed,
        })
        .collect()
}

/// The finished sign-in of a live key.
pub(crate) fn signed(spec: &ProviderSpec, key: SecretText) -> Signed {
    Signed {
        label: AccountLabel(spec.label.clone()),
        credentials: vec![(SecretPurpose::ApiKey, Credential::ApiKey(key))],
        claims: claims(spec),
        endpoints: Vec::new(),
        restriction: Restriction::none(),
    }
}

/// The review of `signed`.
pub(crate) fn review_of(signed: &Signed) -> SignInStep {
    SignInStep::Review {
        claims: signed.claims.clone(),
        endpoints: signed.endpoints.clone(),
        restriction: signed.restriction.clone(),
        label: signed.label.clone(),
    }
}

/// What a failed call says to the person.
pub(crate) fn fault_of(error: ProviderError) -> SignInFault {
    match error {
        ProviderError::Unauthorized => SignInFault::Refused,
        ProviderError::Forbidden => SignInFault::Forbidden,
        ProviderError::Unreachable => SignInFault::Unreachable,
        ProviderError::Unreadable => SignInFault::Unreadable,
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
        let url = porter_core::WebUrl::parse("https://api.example.org/v1/models").expect("url");
        let key = SecretText::new("KEY");
        let headers = |family| {
            present_key(HttpRequest::new(Method::Get, url.clone()), family, &key)
                .headers
                .iter()
                .map(|h| (h.name.as_str().to_ascii_lowercase(), h.value.0.clone()))
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
