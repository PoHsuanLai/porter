//! Which accounts syncd may mirror Storage from or send Photos to: the ones it holds a grant
//! for, by what the grant is for ([`StorageKind`]). Like the PIM grants, the answer comes from
//! `Accounts::find`, which lists only grants syncd already holds: a grant exists when the person
//! makes one in Settings (or a test seeds one), since a daemon cannot draw a consent sheet.
//!
//! Which service an account is reached through is read from the endpoints its candidate lists,
//! never from the provider's name: a `graph` endpoint is OneDrive's app folder (and, for Photos,
//! its Photos folders), a `google_drive` endpoint is Drive's app data folder, and the
//! `google_photos_upload` and `google_photos_picker` endpoints are Google Photos.

use crate::datasets::pim::AccountdUnavailable;
use porter_client::{Accounts, Found, Transport};
use porter_core::capability::{
    Access, Albums, Delta, LibraryRead, Offered, QuotaReport, StorageScope,
};
use porter_core::consent::Usage;
use porter_core::need::{PhotosNeed, StorageNeed};
use porter_core::{Candidate, DataClass, Family, Need, ServiceEndpoint};
use std::future::Future;
use std::sync::Arc;

/// What a Storage grant of syncd's is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StorageKind {
    /// The account's app folder, mirrored two ways into a local folder (`DataClass::Files`).
    AppFolder,
    /// The Photos dataset's two folders in the app folder (`DataClass::Photos`), behind the
    /// [`crate::datasets::photos::PhotosSwitch`].
    Photos,
    /// Google Photos (`DataClass::Photos`, a `Photos`-kind grant): the upload folder, and the
    /// picker import. Behind the same switch.
    GooglePhotos,
}

impl StorageKind {
    /// Every kind.
    pub const ALL: [StorageKind; 3] = [
        StorageKind::AppFolder,
        StorageKind::Photos,
        StorageKind::GooglePhotos,
    ];

    /// The data class the grant covers.
    pub fn class(self) -> DataClass {
        match self {
            StorageKind::AppFolder => DataClass::Files,
            StorageKind::Photos | StorageKind::GooglePhotos => DataClass::Photos,
        }
    }

    /// Whether the kind is one of the Photos datasets, which the switch gates.
    pub fn is_photos(self) -> bool {
        !matches!(self, StorageKind::AppFolder)
    }
}

/// The need syncd asks accounts to meet: read and write the app folder and poll it.
pub fn storage_need() -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

/// The need syncd asks Google Photos to meet: upload into albums the app made, and the picker.
pub fn photos_need() -> Need {
    Need::Photos(PhotosNeed {
        library_read: LibraryRead::PickerOnly,
        upload: Offered::Present,
        albums: Albums::AppCreated,
        video: Offered::Absent,
        delta: Delta::None,
    })
}

/// The need of an account where the person granted the picker scope alone: the picker import,
/// no upload.
pub fn picker_need() -> Need {
    Need::Photos(PhotosNeed {
        library_read: LibraryRead::PickerOnly,
        upload: Offered::Absent,
        albums: Albums::None,
        video: Offered::Absent,
        delta: Delta::None,
    })
}

/// Whether the candidate is reached over Graph (OneDrive): the endpoint to dial.
pub fn graph_endpoint(candidate: &Candidate) -> Option<&ServiceEndpoint> {
    candidate
        .endpoints
        .iter()
        .find(|e| e.family == Family::Graph)
}

/// The store an app folder mirror runs over, by the endpoint the candidate lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppFolderStore<'a> {
    /// OneDrive's app folder, over Graph.
    Graph(&'a ServiceEndpoint),
    /// Drive's app data folder.
    Drive(&'a ServiceEndpoint),
}

/// The app folder store a candidate is reached through: Graph, else Drive.
pub fn app_folder_store(candidate: &Candidate) -> Option<AppFolderStore<'_>> {
    let by = |family: Family| candidate.endpoints.iter().find(|e| e.family == family);
    by(Family::Graph)
        .map(AppFolderStore::Graph)
        .or_else(|| by(Family::GoogleDrive).map(AppFolderStore::Drive))
}

/// The two Google Photos endpoints of a candidate, upload API and picker. Each stands alone: an
/// account where the person granted only the picker scope has no upload endpoint (accountd's
/// family leaves it out), and the other way round. `None` when it has neither.
pub fn google_photos_endpoints(
    candidate: &Candidate,
) -> Option<(Option<&ServiceEndpoint>, Option<&ServiceEndpoint>)> {
    let by = |family: Family| candidate.endpoints.iter().find(|e| e.family == family);
    let (upload, picker) = (
        by(Family::GooglePhotosUpload),
        by(Family::GooglePhotosPicker),
    );
    (upload.is_some() || picker.is_some()).then_some((upload, picker))
}

/// Whether a candidate is one `kind` can run over.
fn runs_over(kind: StorageKind, candidate: &Candidate) -> bool {
    match kind {
        StorageKind::AppFolder => app_folder_store(candidate).is_some(),
        StorageKind::Photos => graph_endpoint(candidate).is_some(),
        StorageKind::GooglePhotos => google_photos_endpoints(candidate).is_some(),
    }
}

/// The granted accounts of a kind.
pub trait StorageGrants: Send + Sync {
    /// Every account syncd holds a grant of this kind for that is reached over a service it
    /// mirrors to (Graph, Drive's app data folder, Google Photos), one candidate each; none is
    /// an empty list. `Err` is that accountd could not be asked:
    /// nothing is known, so nothing is stopped, and the next look asks again.
    fn granted(
        &self,
        kind: StorageKind,
    ) -> impl Future<Output = Result<Vec<Candidate>, AccountdUnavailable>> + Send;
}

/// [`StorageGrants`] over an accountd connection.
#[derive(Debug)]
pub struct ClientStorageGrants<T> {
    accounts: Arc<Accounts<T>>,
}

impl<T> ClientStorageGrants<T> {
    /// Grants as `accounts` finds them.
    pub fn new(accounts: Arc<Accounts<T>>) -> Self {
        Self { accounts }
    }
}

impl<T: Transport> StorageGrants for ClientStorageGrants<T> {
    async fn granted(&self, kind: StorageKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        // Google Photos: an account that uploads, or one that only picks (each scope stands
        // alone); the same grant answers both, and `seen` keeps one candidate per account.
        let needs = match kind {
            StorageKind::GooglePhotos => vec![photos_need(), picker_need()],
            StorageKind::AppFolder | StorageKind::Photos => vec![storage_need()],
        };
        let mut candidates = Vec::new();
        for need in needs {
            let found = self
                .accounts
                .find(&need, kind.class(), Usage::Background)
                .await
                .map_err(|_| AccountdUnavailable)?;
            match found {
                Found::One(one) => candidates.push(one),
                Found::Several(several) => candidates.extend(several),
                Found::NeedsConsent(_) | Found::None(_) => {}
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        candidates.retain(|c| runs_over(kind, c) && seen.insert(c.account.clone()));
        Ok(candidates)
    }
}
