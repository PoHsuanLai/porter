//! The Graph replica as syncd runs it: the same relays as the WebDAV replica
//! ([`crate::webdav::RelayDial`]: one authenticated stream per connection from accountd's
//! `Tokens.OpenAuthenticated`), so syncd never holds the Microsoft token. The relay adds
//! `Authorization: Bearer` and reaches the endpoint's origin (`https://graph.microsoft.com`).
//!
//! Graph's pre-authenticated links (an upload session's `uploadUrl`, the `downloadUrl` a content
//! request redirects to) are on other hosts. Requests to them go through [`LinkedDial`]:
//! accountd's `Tokens.OpenLinked`, a relay that adds no credential and that accountd opens only
//! for an origin the provider file declares (`linked_origins`). [`graph_replica`] builds the
//! replica of a dataset's folder in the account's OneDrive app folder over both. Which grant and
//! endpoint a Storage account on a Microsoft account is reached through is accountd's candidate:
//! the `graph` endpoint under a Storage grant.

use crate::webdav::{BuildError, RelayDial, RelayStream};
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, Origin, WebUrl};
use std::sync::Arc;
use storage_graph::{Clock, Dial, GraphReplica, Routed, StreamHttp, StreamLimits};

/// Opens relays to one linked origin of one grant (`OpenLinked`): no credential is added.
#[derive(Debug)]
pub struct LinkedDial<T> {
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    origin: Option<EndpointUrl>,
}

impl<T> LinkedDial<T> {
    /// Relays to `origin` under `grant`, opened through `accounts`. An origin `OpenLinked` cannot
    /// name (`None`) is one no connection opens to.
    pub fn new(accounts: Arc<Accounts<T>>, grant: GrantId, origin: Option<EndpointUrl>) -> Self {
        Self {
            accounts,
            grant,
            origin,
        }
    }
}

impl<T: Transport> Dial for LinkedDial<T> {
    type Stream = RelayStream;

    async fn dial(&self) -> Result<RelayStream, porter_http::HttpError> {
        let origin = self
            .origin
            .as_ref()
            .ok_or(porter_http::HttpError::Unreachable)?;
        let stream = self
            .accounts
            .open_linked(&self.grant, origin)
            .await
            .map_err(|_| porter_http::HttpError::Unreachable)?;
        RelayStream::from_relay(stream).map_err(|_| porter_http::HttpError::Unreachable)
    }
}

/// The origin as the bare URL `OpenLinked` takes (`https://host[:port]`, an IPv6 host bracketed).
pub(crate) fn bare_url(origin: &Origin) -> Option<EndpointUrl> {
    let host = match origin.host.contains(':') {
        true => format!("[{}]", origin.host),
        false => origin.host.clone(),
    };
    let scheme = match origin.scheme {
        porter_core::UrlScheme::Https => "https",
        porter_core::UrlScheme::Http => "http",
        _ => return None,
    };
    EndpointUrl::parse(&format!("{scheme}://{host}:{}", origin.port)).ok()
}

/// Makes the client for one linked origin.
type MakeLinked<T> = Box<dyn Fn(&Origin) -> StreamHttp<LinkedDial<T>> + Send + Sync>;

/// What the replica sends through: the Graph host over the authenticated relay, any other origin
/// over a linked one.
pub type GraphHttp<T> = Routed<StreamHttp<RelayDial<T>>, StreamHttp<LinkedDial<T>>, MakeLinked<T>>;

/// The replica of `folder` (a path in the app folder, `Photos/Originals`; empty for the app
/// folder itself) of the account the grant covers, over relays accountd opens to `endpoint` and,
/// for the links Graph hands out, to the origins of those links.
pub fn graph_replica<T: Transport + 'static>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    folder: &str,
    clock: Clock,
) -> Result<GraphReplica<GraphHttp<T>>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let home = StreamHttp::new(
        RelayDial::new(Arc::clone(&accounts), grant.clone(), endpoint),
        StreamLimits::default(),
    );
    let linked: MakeLinked<T> = Box::new(move |origin| {
        StreamHttp::new(
            LinkedDial::new(Arc::clone(&accounts), grant.clone(), bare_url(origin)),
            StreamLimits::default(),
        )
    });
    Ok(GraphReplica::new(
        Routed::new(home, &base, linked),
        &base,
        folder,
        clock,
    ))
}
