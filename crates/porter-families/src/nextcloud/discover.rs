//! What a signed-in Nextcloud account can do and where its servers are: the OCS capabilities
//! (which apps are on), the DAV principal (who the user id is, where the calendar and address
//! book homes are) and the files root with its quota. Every request carries the new app
//! password, so a refused one is found here, before anything is stored. The requests and the
//! reading of the answers are nextcloud-proto's and dav-proto's; this sends them.

use super::fault;
use crate::io::Io;
use crate::password::{authorize, basic, fault_of, sensitive};
use dav_proto::Depth;
use dav_proto::discover::{Error as WalkError, Found as Walked, Home, Next, Reach, Scope, Walk};
use dav_proto::http::bytes;
use dav_proto::names::{QUOTA_AVAILABLE, QUOTA_USED};
use nextcloud_proto::{endpoints, judged, ocs, paths};
use porter_capability::Claim;
use porter_core::{EndpointUrl, LoginName, SecretText, ServiceEndpoint};
use porter_http::http::{Request, Response, Uri};
use porter_http::{Http, uri, web_url};
use porter_sheet::SignInFault;

/// Who is signed in, and where.
#[derive(Debug, Clone)]
pub(super) struct Who {
    pub(super) server: EndpointUrl,
    pub(super) login: LoginName,
    pub(super) password: SecretText,
}

/// What was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) claims: Vec<Claim>,
    pub(super) endpoints: Vec<ServiceEndpoint>,
}

impl Who {
    /// The request for the OCS capabilities.
    fn capabilities(&self) -> Result<Request<Vec<u8>>, SignInFault> {
        let authorization = sensitive(&basic(&self.login.0, &self.password)).map_err(fault_of)?;
        ocs::request(&self.server, authorization).map_err(fault)
    }

    fn propfind(&self, url: &EndpointUrl, props: &[&str]) -> Result<Request<Vec<u8>>, SignInFault> {
        let uri = uri(&web_url(url).map_err(fault_of)?).map_err(fault_of)?;
        self.signed(bytes::propfind(uri, Depth::Zero, props))
    }

    /// `request` with the new app password.
    fn signed(&self, mut request: Request<Vec<u8>>) -> Result<Request<Vec<u8>>, SignInFault> {
        authorize(&mut request, &basic(&self.login.0, &self.password)).map_err(fault_of)?;
        Ok(request)
    }
}

/// Sends `request`; a success is returned, anything else is the fault it means.
async fn send(io: &Io, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, SignInFault> {
    let response = io.http.send(request).await.map_err(fault_of)?;
    judged(&response).map_err(fault)?;
    Ok(response)
}

/// The principal and the homes it names, found by the DAV walk from the DAV root. The first
/// reply is also the first call that needs the password, so a refusal shows here; an answer
/// that is not a multistatus means no principal, and a later request that fails means the homes
/// are not known (the conventional paths stand in for them).
async fn walked(
    io: &Io,
    who: &Who,
    dav: &EndpointUrl,
) -> Result<(Option<Uri>, Walked), SignInFault> {
    let root = uri(&web_url(dav).map_err(fault_of)?).map_err(fault_of)?;
    let wants = [Home::Calendar, Home::Addressbook];
    let (mut walk, first) = Walk::start(root, Reach::Homes, &wants, Scope::SameOrigin);
    let reply = io.http.send(who.signed(first)?).await.map_err(fault_of)?;
    let mut next = match walk.advance(&reply) {
        Ok(next) => next,
        Err(WalkError::Unauthorized) => return Err(SignInFault::Refused),
        Err(WalkError::Status(500..=599)) => return Err(SignInFault::Unreachable),
        Err(WalkError::Status(_)) => return Err(SignInFault::Unreadable),
        Err(_) => return Ok((None, Walked::default())),
    };
    let found = loop {
        match next {
            Next::Done(found) => break found,
            Next::Ask(request) => {
                let Ok(request) = who.signed(request) else {
                    break Walked::default();
                };
                let Ok(reply) = io.http.send(request).await else {
                    break Walked::default();
                };
                match walk.advance(&reply) {
                    Ok(more) => next = more,
                    Err(_) => break Walked::default(),
                }
            }
        }
    };
    Ok((walk.principal().cloned(), found))
}

/// Finds out what the account can do.
pub(super) async fn discover(io: &Io, who: &Who) -> Result<Found, SignInFault> {
    let unreadable = || SignInFault::Unreadable;
    let dav = paths::dav_root(&who.server).ok_or_else(unreadable)?;

    let (principal, found) = walked(io, who, &dav).await?;
    let user = principal
        .and_then(|uri| paths::user_of(uri.path()))
        .unwrap_or_else(|| who.login.0.clone());

    let capabilities = io.http.send(who.capabilities()?).await.map_err(fault_of)?;
    let claims = ocs::capabilities(&capabilities).map_err(fault)?;

    let files = paths::files(&who.server, &user).ok_or_else(unreadable)?;
    // The files root answers or the account has no storage worth offering; the quota is the
    // question asked (the claim says it is reported).
    send(io, who.propfind(&files, &[QUOTA_USED, QUOTA_AVAILABLE])?).await?;

    let first = |home| {
        found
            .homes_of(home)
            .iter()
            .find_map(|uri| EndpointUrl::parse(&uri.to_string()).ok())
    };
    let endpoints = endpoints(
        &who.server,
        &user,
        &who.login,
        &claims,
        first(Home::Calendar),
        first(Home::Addressbook),
    )
    .ok_or_else(unreadable)?;
    Ok(Found { claims, endpoints })
}
