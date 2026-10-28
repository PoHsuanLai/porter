//! Nextcloud's `/ocs/v2.php/cloud/capabilities`: the request, and the answer read as claims.

use crate::{Error, authorized, judged, web};
use http::header::ACCEPT;
use http::{HeaderValue, Method, Request, Response};
use porter_capability::{AbsentReason, Claim, Offer, Provenance, Subject};
use porter_capability::{
    Access, Capability, CapabilityKind, Delta, HashKind, NotesCap, NotesTransport, PimCap,
    PimTransport, StorageCap, StorageScope,
};
use porter_core::EndpointUrl;
use serde_json::{Map, Value};

/// The capabilities request, with the account's `authorization`.
pub fn request(
    server: &EndpointUrl,
    authorization: HeaderValue,
) -> Result<Request<Vec<u8>>, Error> {
    let url = web(&crate::paths::capabilities(server).ok_or(Error::Unreadable)?)?;
    let request = Request::builder()
        .method(Method::GET)
        .uri(url.as_str())
        .header("OCS-APIRequest", "true")
        .header(ACCEPT, "application/json")
        .body(Vec::new())
        .map_err(|_| Error::Unreadable)?;
    Ok(authorized(request, authorization))
}

/// What the capabilities `response` says the account can do, or why it says nothing.
pub fn capabilities(response: &Response<Vec<u8>>) -> Result<Vec<Claim>, Error> {
    let response = judged(response)?;
    parse_capabilities(&String::from_utf8_lossy(response.body()))
}

/// What a Nextcloud's OCS capabilities answer says the account can do: files and quota always;
/// calendar, contacts, tasks and notes when their apps are on the server (an app that is not
/// there is `Absent { NotOnServer }`).
///
/// The server's own DAV app serves calendars and address books in every install, so `dav` or the
/// app's own key (`calendar`, `contacts`) marks them present; tasks need the `tasks` key or the
/// calendar app (which keeps VTODO), notes need the `notes` key. Chunked upload is present when
/// `dav.chunking` is advertised.
pub fn parse_capabilities(json: &str) -> Result<Vec<Claim>, Error> {
    let root: Value = serde_json::from_str(json).map_err(|_| Error::Unreadable)?;
    let ocs = root.get("ocs").ok_or(Error::Unreadable)?;
    let status = ocs
        .pointer("/meta/statuscode")
        .and_then(Value::as_u64)
        .ok_or(Error::Unreadable)?;
    if !matches!(status, 100 | 200) {
        return Err(Error::Unreadable);
    }
    let caps = ocs
        .pointer("/data/capabilities")
        .and_then(Value::as_object)
        .ok_or(Error::Unreadable)?;
    let has = |key: &str| caps.contains_key(key);
    let pim = |transport| PimCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        transport,
        collections: true,
    };
    let dav_or = |key: &str| has("dav") || has(key);
    let chunked = chunking(caps);
    let claims = vec![
        present(Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: true,
            scope: StorageScope::Full,
            hashes: HashKind::Sha1,
            ranges: true,
            chunked_upload: chunked,
        })),
        app(
            dav_or("calendar"),
            CapabilityKind::Calendar,
            Capability::Calendar(pim(PimTransport::CalDav)),
        ),
        app(
            dav_or("contacts"),
            CapabilityKind::Contacts,
            Capability::Contacts(pim(PimTransport::CardDav)),
        ),
        app(
            has("tasks") || has("calendar"),
            CapabilityKind::Tasks,
            Capability::Tasks(pim(PimTransport::CalDav)),
        ),
        app(
            has("notes"),
            CapabilityKind::Notes,
            Capability::Notes(NotesCap {
                access: Access::ReadWrite,
                delta: Delta::Poll,
                transport: NotesTransport::NextcloudNotes,
            }),
        ),
    ];
    Ok(claims)
}

fn chunking(caps: &Map<String, Value>) -> bool {
    caps.get("dav")
        .and_then(|dav| dav.get("chunking"))
        .is_some()
}

fn present(capability: Capability) -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(capability),
        provenance: Provenance::Discovered,
    }
}

/// `capability` when the app is there, else the kind marked missing on this server.
fn app(installed: bool, kind: CapabilityKind, capability: Capability) -> Claim {
    match installed {
        true => present(capability),
        false => Claim {
            subject: Subject::Account,
            offer: Offer::Absent {
                kind,
                reason: AbsentReason::NotOnServer,
            },
            provenance: Provenance::Discovered,
        },
    }
}

#[cfg(test)]
mod tests;
