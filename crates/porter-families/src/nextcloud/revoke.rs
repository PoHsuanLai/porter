//! Taking an app password back: `DELETE /ocs/v2.php/core/apppassword`, authenticated with the
//! app password itself, deletes the one it authenticates with.

use crate::io::Io;
use crate::password::{basic, join};
use porter_core::{EndpointUrl, LoginName, SecretText};
use porter_http::http::Method;
use porter_http::http::header::{ACCEPT, AUTHORIZATION};
use porter_http::{Http, SensitiveHeader, request_to};
use porter_provider::{ProviderError, RevokeOutcome};

/// Deletes the app password. A server that already refuses it has nothing left to revoke; one
/// without the endpoint (before Nextcloud 13) sends the person to their security settings.
pub(super) async fn revoke(
    io: &Io,
    server: &EndpointUrl,
    login: &LoginName,
    password: &SecretText,
) -> Result<RevokeOutcome, ProviderError> {
    let url = join(server, "/ocs/v2.php/core/apppassword").ok_or(ProviderError::Unreadable)?;
    let request = request_to(Method::DELETE, &url)
        .map_err(|_| ProviderError::Unreadable)?
        .sensitive_header(AUTHORIZATION, &basic(&login.0, password))
        .header("OCS-APIRequest", "true")
        .header(ACCEPT, "application/json")
        .body(Vec::new())
        .map_err(|_| ProviderError::Unreachable)?;
    let response = io
        .http
        .send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)?;
    match response.status().as_u16() {
        200..=299 | 401 | 403 => Ok(RevokeOutcome::Revoked),
        404 => join(server, "/settings/user/security")
            .map(RevokeOutcome::Manual)
            .ok_or(ProviderError::Unreadable),
        500..=599 => Err(ProviderError::Unreachable),
        _ => Err(ProviderError::Unreadable),
    }
}
