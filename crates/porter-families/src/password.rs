//! What the two password families (Nextcloud's app password, a generic server's password) share:
//! the form fields, reading what was typed, server addresses, HTTP Basic, and the session a
//! password account opens.
//!
//! A password never leaves accountd, so a session of these families mints no token: an app
//! reaches its servers through an authenticated relay (`OpenAuthenticated`), which reads the
//! credential itself.

use base64::Engine;
use porter_core::capability::CapabilityKind;
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, SignInFault,
};
use porter_core::{Claim, Credential, EndpointUrl, Offer, Provenance, SecretText, Subject, Tls};
use porter_core::{ServiceEndpoint, UrlScheme};
use porter_http::HttpError;
use porter_provider::{Presented, ProviderError, ProviderSpec};

/// A form field.
pub(crate) fn field(kind: FieldKind, entry: Entry, presence: Presence) -> FieldSpec {
    FieldSpec {
        kind,
        entry,
        presence,
        prefill: None,
    }
}

/// The required, shown field of this kind.
pub(crate) fn plain(kind: FieldKind) -> FieldSpec {
    field(kind, Entry::Plain, Presence::Required)
}

/// The required, hidden password field.
pub(crate) fn password() -> FieldSpec {
    field(FieldKind::Password, Entry::Secret, Presence::Required)
}

/// What a request that went nowhere says about the sign-in.
pub(crate) fn fault_of(error: HttpError) -> SignInFault {
    match error {
        HttpError::Unreachable | HttpError::Tls | HttpError::TimedOut => SignInFault::Unreachable,
        HttpError::TooLarge | HttpError::Malformed => SignInFault::Unreadable,
    }
}

/// The plain text typed for `kind`, trimmed, when it is not empty.
pub(crate) fn text_of(answers: &[FieldAnswer], kind: FieldKind) -> Option<String> {
    answers
        .iter()
        .find(|a| a.kind == kind)
        .map(|a| match &a.value {
            FieldValue::Plain(text) => text.trim().to_owned(),
            FieldValue::Secret(secret) => secret.expose().trim().to_owned(),
        })
        .filter(|text| !text.is_empty())
}

/// The secret typed for `kind`, exactly as typed (a password may start or end with a space).
pub(crate) fn secret_of(answers: &[FieldAnswer], kind: FieldKind) -> Option<SecretText> {
    answers
        .iter()
        .find(|a| a.kind == kind)
        .map(|a| match &a.value {
            FieldValue::Secret(secret) => secret.clone(),
            FieldValue::Plain(text) => SecretText::new(text.clone()),
        })
        .filter(|secret| !secret.expose().is_empty())
}

/// The server a person typed: a host (`cloud.example.org`), or an address with a scheme, a port
/// and a path (`https://example.org/nextcloud`). A bare host is `https`; plain `http` is for
/// this computer only.
pub(crate) fn parse_server(text: &str) -> Option<EndpointUrl> {
    let text = text.trim().trim_end_matches('/');
    if text.is_empty() {
        return None;
    }
    let with_scheme = match text.contains("://") {
        true => text.to_owned(),
        false => format!("https://{text}"),
    };
    let url = EndpointUrl::parse(&with_scheme).ok()?;
    let origin = url.origin();
    match origin.scheme {
        UrlScheme::Https => Some(url),
        UrlScheme::Http if origin.is_loopback() => Some(url),
        _ => None,
    }
}

/// `path` under `base`.
pub(crate) fn join(base: &EndpointUrl, path: &str) -> Option<EndpointUrl> {
    let base = base.as_str().trim_end_matches('/');
    EndpointUrl::parse(&format!("{base}/{}", path.trim_start_matches('/'))).ok()
}

/// What an `href` names, on `base`'s server: a path on its origin, or a URL of that origin.
pub(crate) fn resolve(base: &EndpointUrl, href: &str) -> Option<EndpointUrl> {
    let href = href.trim();
    let url = match href.starts_with('/') {
        true => EndpointUrl::parse(&format!("{}{href}", root_of(base))),
        false => EndpointUrl::parse(href),
    }
    .ok()?;
    (url.origin() == base.origin()).then_some(url)
}

/// `scheme://authority` of `url`, as written.
fn root_of(url: &EndpointUrl) -> &str {
    let text = url.as_str();
    let after = text.find("://").map_or(0, |at| at + 3);
    text[after..]
        .find('/')
        .map_or(text, |at| &text[..after + at])
}

/// A path segment, percent-encoded (an `@` or a space in a user id is not a URL character).
pub(crate) fn segment(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The TLS mode a URL's scheme means.
pub(crate) fn tls_of(url: &EndpointUrl) -> Tls {
    match url.origin().scheme {
        UrlScheme::Https | UrlScheme::Imaps | UrlScheme::Smtps => Tls::Implicit,
        _ => Tls::Plain,
    }
}

/// A coherent endpoint, or `None`.
pub(crate) fn endpoint(
    family: porter_core::Family,
    url: EndpointUrl,
    login: &porter_core::LoginName,
) -> Option<ServiceEndpoint> {
    let endpoint = ServiceEndpoint {
        tls: tls_of(&url),
        family,
        url,
        login: login.clone(),
    };
    endpoint.check().ok().map(|()| endpoint)
}

/// The `Authorization` value of HTTP Basic.
pub(crate) fn basic(login: &str, password: &SecretText) -> String {
    let token =
        base64::engine::general_purpose::STANDARD.encode(format!("{login}:{}", password.expose()));
    format!("Basic {token}")
}

/// What the provider file declares, as claims at `Declared` provenance.
pub(crate) fn declared(spec: &ProviderSpec) -> Vec<Claim> {
    spec.capabilities
        .iter()
        .map(|row| Claim {
            subject: Subject::Account,
            offer: Offer::Present(row.capability.clone()),
            provenance: Provenance::Declared,
        })
        .collect()
}

/// Whether `claims` offer `kind`.
pub(crate) fn offers(claims: &[Claim], kind: CapabilityKind) -> bool {
    claims
        .iter()
        .any(|c| matches!(&c.offer, Offer::Present(cap) if cap.kind() == kind))
}

/// The password an account presents: the credential, when it is one.
pub(crate) fn password_of(presented: &Presented) -> Result<&SecretText, ProviderError> {
    match presented {
        Presented::Credential(Credential::Password(password)) => Ok(password),
        Presented::Anonymous => Err(ProviderError::Unauthorized),
        Presented::Credential(_) => Err(ProviderError::Unreadable),
    }
}

#[cfg(test)]
mod tests;
