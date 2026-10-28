//! What the two password families (Nextcloud's app password, a generic server's password) share:
//! the form fields, reading what was typed, server addresses, HTTP Basic, and the session a
//! password account opens.
//!
//! A password never leaves accountd, so a session of these families mints no token: an app
//! reaches its servers through an authenticated relay (`OpenAuthenticated`), which reads the
//! credential itself.

use base64::Engine;
use porter_capability::{Claim, Offer, Provenance, Subject};
use porter_core::{Credential, SecretText};
use porter_http::HttpError;
use porter_http::http::Request;
use porter_http::http::header::{AUTHORIZATION, HeaderValue};
use porter_provider::{Presented, ProviderError, ProviderSpec};
use porter_sheet::{Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, SignInFault};

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

/// The password field a provider asks for: an app password when its sign-in is `app_password`
/// (iCloud, Fastmail, Yahoo: the service takes no account password from a mail app), else a
/// password.
#[cfg(feature = "generic")]
pub(crate) fn password_for(spec: &ProviderSpec) -> FieldSpec {
    match spec.auth.kind {
        porter_core::AuthKind::AppPassword => {
            field(FieldKind::AppPassword, Entry::Secret, Presence::Required)
        }
        _ => password(),
    }
}

/// The password typed, in either field: an app password, else a password (a host may send
/// either kind for the field it was shown).
#[cfg(feature = "generic")]
pub(crate) fn typed_password(answers: &[FieldAnswer]) -> Option<SecretText> {
    secret_of(answers, FieldKind::AppPassword).or_else(|| secret_of(answers, FieldKind::Password))
}

/// What a request that went nowhere says about the sign-in.
pub(crate) fn fault_of(error: HttpError) -> SignInFault {
    match error {
        HttpError::Unreachable | HttpError::Tls | HttpError::TimedOut | HttpError::NotGranted => {
            SignInFault::Unreachable
        }
        // too large, malformed, and a failure a later porter-http adds
        _ => SignInFault::Unreadable,
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

/// The `Authorization` value of HTTP Basic.
pub(crate) fn basic(login: &str, password: &SecretText) -> String {
    let token =
        base64::engine::general_purpose::STANDARD.encode(format!("{login}:{}", password.expose()));
    format!("Basic {token}")
}

/// `value` as a header value marked sensitive, so a request's `Debug` never shows it. A value
/// that cannot be a header's is `Malformed`.
pub(crate) fn sensitive(value: &str) -> Result<HeaderValue, HttpError> {
    let mut value = HeaderValue::from_str(value).map_err(|_| HttpError::Malformed)?;
    value.set_sensitive(true);
    Ok(value)
}

/// Puts `value` on a built request as its `Authorization`, marked sensitive so the request's
/// `Debug` never shows it. A value that cannot be a header's is `Malformed`.
pub(crate) fn authorize(request: &mut Request<Vec<u8>>, value: &str) -> Result<(), HttpError> {
    request
        .headers_mut()
        .insert(AUTHORIZATION, sensitive(value)?);
    Ok(())
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

/// The password an account presents: the credential, when it is one.
pub(crate) fn password_of(presented: &Presented) -> Result<&SecretText, ProviderError> {
    match presented {
        Presented::Credential(Credential::Password(password)) => Ok(password),
        Presented::Anonymous => Err(ProviderError::Unauthorized),
        Presented::Credential(_) => Err(ProviderError::Unreadable),
    }
}

/// The secret a generic account presents: a password, or a bearer token the person pasted.
pub(crate) fn secret_presented(presented: &Presented) -> Result<&SecretText, ProviderError> {
    match presented {
        Presented::Credential(Credential::Bearer(token)) => Ok(token),
        other => password_of(other),
    }
}

#[cfg(test)]
mod tests;
