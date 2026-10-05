//! What a signed-in Nextcloud account can do and where its servers are: the OCS capabilities
//! (which apps are on), the DAV principal (who the user id is, where the calendar and address
//! book homes are) and the files root with its quota. Every request carries the new app
//! password, so a refused one is found here, before anything is stored.

use crate::io::Io;
use crate::password::{basic, endpoint, fault_of, join, offers, resolve, segment};
use porter_core::capability::CapabilityKind;
use porter_core::sheet::SignInFault;
use porter_core::{Claim, EndpointUrl, Family, LoginName, SecretText, ServiceEndpoint};
use porter_dav::names::{
    ADDRESSBOOK_HOME_SET, CALENDAR_HOME_SET, CURRENT_USER_PRINCIPAL, QUOTA_AVAILABLE, QUOTA_USED,
};
use porter_dav::{Depth, Home, home_set, parse_multistatus, propfind};
use porter_discover::parse_ocs_capabilities;
use porter_http::{Http, HttpRequest, HttpResponse, Method};

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
    fn get(&self, url: EndpointUrl) -> HttpRequest {
        HttpRequest::new(Method::Get, url)
            .with_header("Authorization", basic(&self.login.0, &self.password))
    }

    fn propfind(&self, url: EndpointUrl, props: &[&str]) -> HttpRequest {
        let mut request = propfind(url, Depth::Zero, props);
        request.headers.push(porter_http::Header::new(
            "Authorization",
            basic(&self.login.0, &self.password),
        ));
        request
    }
}

/// A reply that is a login refusal, a success, or something else.
fn judged(response: HttpResponse) -> Result<HttpResponse, SignInFault> {
    match response.status.0 {
        401 | 403 => Err(SignInFault::Refused),
        200..=299 => Ok(response),
        500..=599 => Err(SignInFault::Unreachable),
        _ => Err(SignInFault::Unreadable),
    }
}

async fn send(io: &Io, request: HttpRequest) -> Result<HttpResponse, SignInFault> {
    judged(io.http.send(request).await.map_err(fault_of)?)
}

/// Finds out what the account can do.
pub(super) async fn discover(io: &Io, who: &Who) -> Result<Found, SignInFault> {
    let unreadable = || SignInFault::Unreadable;
    let dav = join(&who.server, "/remote.php/dav/").ok_or_else(unreadable)?;

    // The principal is also the first call that needs the password, so a refusal shows here.
    let principal = send(io, who.propfind(dav.clone(), &[CURRENT_USER_PRINCIPAL])).await?;
    let principal = parse_multistatus(&text(&principal))
        .ok()
        .and_then(|status| porter_dav::current_user_principal(&status))
        .and_then(|href| resolve(&who.server, &href));
    let user = principal
        .as_ref()
        .and_then(|url| user_of(url.path()))
        .unwrap_or_else(|| who.login.0.clone());

    let capabilities = {
        let request = who
            .get(join(&who.server, "/ocs/v2.php/cloud/capabilities").ok_or_else(unreadable)?)
            .with_header("OCS-APIRequest", "true")
            .with_header("Accept", "application/json");
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
    send(
        io,
        who.propfind(files.clone(), &[QUOTA_USED, QUOTA_AVAILABLE]),
    )
    .await?;

    let homes = homes(io, who, principal.as_ref()).await;
    let calendars = homes
        .0
        .or_else(|| conventional(&who.server, "calendars", &user));
    let books = homes
        .1
        .or_else(|| conventional(&who.server, "addressbooks/users", &user));
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

/// The calendar and address book homes the principal names, when it names them.
async fn homes(
    io: &Io,
    who: &Who,
    principal: Option<&EndpointUrl>,
) -> (Option<EndpointUrl>, Option<EndpointUrl>) {
    let Some(principal) = principal else {
        return (None, None);
    };
    let request = who.propfind(
        principal.clone(),
        &[CALENDAR_HOME_SET, ADDRESSBOOK_HOME_SET],
    );
    let Ok(response) = send(io, request).await else {
        return (None, None);
    };
    let Ok(status) = parse_multistatus(&text(&response)) else {
        return (None, None);
    };
    let first = |home| {
        home_set(&status, home)
            .into_iter()
            .find_map(|href| resolve(&who.server, &href))
    };
    (first(Home::Calendar), first(Home::Addressbook))
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
    let decoded = percent_decode(last)?;
    (!decoded.is_empty()).then_some(decoded)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let hex = text.get(at + 1..at + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                at += 3;
            }
            other => {
                out.push(other);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn text(response: &HttpResponse) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

#[cfg(test)]
mod tests;
