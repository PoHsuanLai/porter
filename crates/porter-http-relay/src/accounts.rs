//! [`RelayDial`]: the connections of a client are relays accountd opens.

use crate::client::Dial;
use crate::stream::RelayStream;
use porter_client::{Accounts, ClientError, Transport};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_core::wire::Refusal;
use porter_http::HttpError;
use porter_ids::{Origin, UrlScheme};
use std::sync::Arc;

/// Why a relay could not be had, as the client takes it: access the account has not given yet
/// (`NeedsConsent`, a whole drive before the person turned it on) is its own error, so the
/// dataset says so instead of retrying; anything else is a connection that did not open.
pub fn dial_error(error: ClientError) -> HttpError {
    match error {
        ClientError::Refused(Refusal::NeedsConsent) => HttpError::NotGranted,
        _ => HttpError::Unreachable,
    }
}

/// The origin as the bare URL `OpenLinked` and `OpenAuthenticated` take (`https://host[:port]`,
/// an IPv6 host bracketed), or `None` for a scheme that is not HTTP.
pub fn bare_url(origin: &Origin) -> Option<EndpointUrl> {
    let host = match origin.host.contains(':') {
        true => format!("[{}]", origin.host),
        false => origin.host.clone(),
    };
    let scheme = match origin.scheme {
        UrlScheme::Https => "https",
        UrlScheme::Http => "http",
        _ => return None,
    };
    EndpointUrl::parse(&format!("{scheme}://{host}:{}", origin.port)).ok()
}

/// What a dial opens.
#[derive(Debug)]
enum Kind {
    /// `OpenAuthenticated` to an endpoint: the relay adds the credential.
    Authenticated(EndpointUrl),
    /// `OpenLinked` to an origin (`None`: one it cannot name, to which no connection opens): the
    /// relay adds no credential.
    Linked(Option<EndpointUrl>),
}

/// Opens relays to one endpoint, or one linked origin, of one grant.
#[derive(Debug)]
pub struct RelayDial<T> {
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    kind: Kind,
}

impl<T> RelayDial<T> {
    /// Relays to `endpoint` under `grant`, authenticated by the daemon at the far end
    /// (`OpenAuthenticated`), opened through `accounts`.
    pub fn authenticated(
        accounts: Arc<Accounts<T>>,
        grant: GrantId,
        endpoint: EndpointUrl,
    ) -> Self {
        Self {
            accounts,
            grant,
            kind: Kind::Authenticated(endpoint),
        }
    }

    /// Relays to `origin` under `grant`, with no credential added (`OpenLinked`: a
    /// pre-authenticated link on another host). An origin `OpenLinked` cannot name (`None`) is
    /// one no connection opens to.
    pub fn linked(accounts: Arc<Accounts<T>>, grant: GrantId, origin: Option<EndpointUrl>) -> Self {
        Self {
            accounts,
            grant,
            kind: Kind::Linked(origin),
        }
    }
}

impl<T: Transport> Dial for RelayDial<T> {
    type Stream = RelayStream;

    async fn dial(&self) -> Result<RelayStream, HttpError> {
        let stream = match &self.kind {
            Kind::Authenticated(endpoint) => {
                self.accounts
                    .open_authenticated(&self.grant, endpoint)
                    .await
            }
            Kind::Linked(origin) => {
                let origin = origin.as_ref().ok_or(HttpError::Unreachable)?;
                self.accounts.open_linked(&self.grant, origin).await
            }
        }
        .map_err(dial_error)?;
        RelayStream::from_relay(stream).map_err(|_| HttpError::Unreachable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_not_given_reaches_the_client_as_its_own_error() {
        assert_eq!(
            dial_error(ClientError::Refused(Refusal::NeedsConsent)),
            HttpError::NotGranted
        );
        for other in [Refusal::NeedsReauth, Refusal::Denied, Refusal::UnknownGrant] {
            assert_eq!(
                dial_error(ClientError::Refused(other)),
                HttpError::Unreachable,
                "{other:?}"
            );
        }
    }
}
