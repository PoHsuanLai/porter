//! What a signed-in Nextcloud account can do and where its servers are: the OCS capabilities
//! (which apps are on), the DAV principal (who the user id is, where the calendar and address
//! book homes are) and the files root with its quota. Every request carries the new app
//! password, so a refused one is found here, before anything is stored.

use crate::io::Io;
use crate::password::{authorize, basic, endpoint, fault_of, join, offers, segment};
use dav_proto::Depth;
use dav_proto::discover::{Error as WalkError, Found as Walked, Home, Next, Reach, Scope, Walk};
use dav_proto::http::bytes;
use dav_proto::names::{QUOTA_AVAILABLE, QUOTA_USED};
use porter_capability::CapabilityKind;
use porter_capability::Claim;
use porter_core::{EndpointUrl, Family, LoginName, SecretText, ServiceEndpoint};
use porter_discover::parse_ocs_capabilities;
use porter_http::http::header::{ACCEPT, AUTHORIZATION};
use porter_http::http::request::Builder;
use porter_http::http::{Method, Request, Response, Uri};
use porter_http::{Http, HttpError, SensitiveHeader, request_to, uri, web_url};
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
    fn get(&self, url: &EndpointUrl) -> Result<Builder, SignInFault> {
        Ok(request_to(Method::GET, url)
            .map_err(fault_of)?
            .sensitive_header(AUTHORIZATION, &basic(&self.login.0, &self.password)))
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

/// A reply that is a login refusal, a success, or something else.
fn judged(response: Response<Vec<u8>>) -> Result<Response<Vec<u8>>, SignInFault> {
    match response.status().as_u16() {
        401 | 403 => Err(SignInFault::Refused),
        200..=299 => Ok(response),
        500..=599 => Err(SignInFault::Unreachable),
        _ => Err(SignInFault::Unreadable),
    }
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

async fn send(io: &Io, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, SignInFault> {
    judged(io.http.send(request).await.map_err(fault_of)?)
}

/// Finds out what the account can do.
pub(super) async fn discover(io: &Io, who: &Who) -> Result<Found, SignInFault> {
    let unreadable = || SignInFault::Unreadable;
    let dav = join(&who.server, "/remote.php/dav/").ok_or_else(unreadable)?;

    let (principal, found) = walked(io, who, &dav).await?;
    let principal = principal.and_then(|uri| EndpointUrl::parse(&uri.to_string()).ok());
    let user = principal
        .as_ref()
        .and_then(|url| user_of(url.path()))
        .unwrap_or_else(|| who.login.0.clone());

    let capabilities = {
        let request = who
            .get(&join(&who.server, "/ocs/v2.php/cloud/capabilities").ok_or_else(unreadable)?)?
            .header("OCS-APIRequest", "true")
            .header(ACCEPT, "application/json")
            .body(Vec::new())
            .map_err(|error| fault_of(HttpError::from(error)))?;
        send(io, request).await?
    };
    let claims = parse_ocs_capabilities(&text(&capabilities))
        .map_err(|_| SignInFault::Unreadable)?
        .claims;

    let files = join(
        &who.server,
        &format!("/remote.php/dav/files/{}/", segment(&user)),
    )
    .ok_or_else(unreadable)?;
    // The files root answers or the account has no storage worth offering; the quota is the
    // question asked (the claim says it is reported).
    send(io, who.propfind(&files, &[QUOTA_USED, QUOTA_AVAILABLE])?).await?;

    let first = |home| {
        found
            .homes_of(home)
            .iter()
            .find_map(|uri| EndpointUrl::parse(&uri.to_string()).ok())
    };
    let calendars = first(Home::Calendar).or_else(|| conventional(&who.server, "calendars", &user));
    let books =
        first(Home::Addressbook).or_else(|| conventional(&who.server, "addressbooks/users", &user));
    let notes = join(&who.server, "/index.php/apps/notes/api/v1/");

    let rows = [
        (Family::WebDav, Some(files), true),
        (
            Family::CalDav,
            calendars,
            offers(&claims, CapabilityKind::Calendar) || offers(&claims, CapabilityKind::Tasks),
        ),
        (
            Family::CardDav,
            books,
            offers(&claims, CapabilityKind::Contacts),
        ),
        (
            Family::NextcloudNotes,
            notes,
            offers(&claims, CapabilityKind::Notes),
        ),
    ];
    let endpoints = rows
        .into_iter()
        .filter(|(_, _, wanted)| *wanted)
        .map(|(family, url, _)| url.and_then(|u| endpoint(family, u, &who.login)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(unreadable)?;
    Ok(Found { claims, endpoints })
}

fn conventional(server: &EndpointUrl, tree: &str, user: &str) -> Option<EndpointUrl> {
    join(
        server,
        &format!("/remote.php/dav/{tree}/{}/", segment(user)),
    )
}

/// The user id at the end of a principal path (`/remote.php/dav/principals/users/ada/`), decoded.
fn user_of(path: &str) -> Option<String> {
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let decoded = porter_ids::percent::decode_strict(last)?;
    (!decoded.is_empty()).then_some(decoded)
}

fn text(response: &Response<Vec<u8>>) -> String {
    String::from_utf8_lossy(response.body()).into_owned()
}

#[cfg(test)]
mod tests;
