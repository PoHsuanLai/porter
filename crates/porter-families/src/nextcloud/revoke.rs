//! Taking an app password back, sent. The request and the reading of the answer are
//! nextcloud-proto's.

use crate::io::Io;
use crate::password::{basic, sensitive};
use nextcloud_proto::Error;
use nextcloud_proto::revoke::{self, Revoked};
use porter_core::{EndpointUrl, LoginName, SecretText};
use porter_http::Http;
use porter_provider::{ProviderError, RevokeOutcome};

/// Deletes the app password. A server that already refuses it has nothing left to revoke; one
/// without the endpoint (before Nextcloud 13) sends the person to their security settings.
pub(super) async fn revoke(
    io: &Io,
    server: &EndpointUrl,
    login: &LoginName,
    password: &SecretText,
) -> Result<RevokeOutcome, ProviderError> {
    let authorization =
        sensitive(&basic(&login.0, password)).map_err(|_| ProviderError::Unreadable)?;
    let request = revoke::request(server, authorization).map_err(|_| ProviderError::Unreadable)?;
    let response = io
        .http
        .send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)?;
    match revoke::revoked(server, &response) {
        Ok(Revoked::Done) => Ok(RevokeOutcome::Revoked),
        Ok(Revoked::Manual(page)) => Ok(RevokeOutcome::Manual(page)),
        Err(Error::Unreachable) => Err(ProviderError::Unreachable),
        Err(_) => Err(ProviderError::Unreadable),
    }
}
