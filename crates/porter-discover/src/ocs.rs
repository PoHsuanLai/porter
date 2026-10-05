//! Nextcloud's `/ocs/v2.php/cloud/capabilities` answer.

use crate::found::{DiscoverFault, Found, Source};
use porter_core::capability::{
    Access, Capability, CapabilityKind, Delta, HashKind, NotesCap, NotesTransport, Offered, PimCap,
    PimTransport, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{AbsentReason, Claim, Offer, Provenance, Subject};
use serde_json::{Map, Value};

/// What a Nextcloud's OCS capabilities answer says the account can do: files and quota always;
/// calendar, contacts, tasks and notes when their apps are on the server (an app that is not
/// there is `Absent { NotOnServer }`).
///
/// The server's own DAV app serves calendars and address books in every install, so `dav` or the
/// app's own key (`calendar`, `contacts`) marks them present; tasks need the `tasks` key or the
/// calendar app (which keeps VTODO), notes need the `notes` key. Chunked upload is present when
/// `dav.chunking` is advertised.
pub fn parse_ocs_capabilities(json: &str) -> Result<Found, DiscoverFault> {
    let root: Value = serde_json::from_str(json).map_err(|_| DiscoverFault::Unreadable)?;
    let ocs = root.get("ocs").ok_or(DiscoverFault::Unreadable)?;
    let status = ocs
        .pointer("/meta/statuscode")
        .and_then(Value::as_u64)
        .ok_or(DiscoverFault::Unreadable)?;
    if !matches!(status, 100 | 200) {
        return Err(DiscoverFault::NoServers);
    }
    let caps = ocs
        .pointer("/data/capabilities")
        .and_then(Value::as_object)
        .ok_or(DiscoverFault::Unreadable)?;
    let has = |key: &str| caps.contains_key(key);
    let pim = |transport| PimCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        transport,
        collections: Offered::Present,
    };
    let dav_or = |key: &str| has("dav") || has(key);
    let chunked = match chunking(caps) {
        true => Offered::Present,
        false => Offered::Absent,
    };
    let claims = vec![
        present(Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            scope: StorageScope::Full,
            hashes: HashKind::Sha1,
            ranges: Offered::Present,
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
    Ok(Found {
        endpoints: Vec::new(),
        claims,
        source: Source::Ocs,
    })
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
