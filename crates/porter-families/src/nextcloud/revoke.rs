//! Taking an app password back: `DELETE /ocs/v2.php/core/apppassword`, authenticated with the
//! app password itself, deletes the one it authenticates with.

use crate::io::Io;
use crate::password::{basic, join};
use porter_core::{EndpointUrl, LoginName, SecretText};
use porter_http::{Http, HttpRequest, Method};
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
    let request = HttpRequest::to(Method::Delete, &url)
        .map_err(|_| ProviderError::Unreadable)?
        .with_header("Authorization", basic(&login.0, password))
        .with_header("OCS-APIRequest", "true")
        .with_header("Accept", "application/json");
    let response = io
        .http
        .send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)?;
    match response.status.0 {
        200..=299 | 401 | 403 => Ok(RevokeOutcome::Revoked),
        404 => join(server, "/settings/user/security")
            .map(RevokeOutcome::Manual)
            .ok_or(ProviderError::Unreadable),
        500..=599 => Err(ProviderError::Unreachable),
        _ => Err(ProviderError::Unreadable),
    }
}
