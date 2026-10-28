//! The Google Drive replica (design/31 §6.1, lane google-drive): [`GdriveReplica`] implements
//! porter-sync's `Replica` over a folder of Drive's app data folder (`spaces=appDataFolder`, the
//! scope `drive.appdata`) or, given [`Root::Drive`], of My Drive (`spaces=drive`, the user's own
//! corpus, no shared drives; the scope `drive`). In My Drive, folders are items too, and Google's
//! own documents and shortcuts are items that open online (`ItemKind::Online`, with the Office
//! type a copy is saved as); only a file's first parent is followed.
//!
//! - **Changes** are `changes.list` with a page token as the anchor (`changes:<token>`). The
//!   first listing is `files.list` of the space, page by page, after the start token was taken
//!   (so nothing between is missed): while it runs the anchor is `list:<start>:<page>`, and the
//!   last page's anchor is `changes:<start>`. A token Drive refuses (`400`, `404`, `410` on a
//!   resumed anchor) is `AnchorExpired`. A change names a file by id and parent id, so paths are
//!   built from the folders seen (and asked for when one is not). The folders seen are a
//!   [`porter_sync::Folders`] a dataset may keep ([`GdriveReplica::folders`]) and hand back
//!   ([`GdriveReplica::with_folders`]).
//! - **Writes** are checked first: a new file looks for its name in the parent (a name already
//!   there is a `Conflict`: Drive allows two files of one name, this replica does not make
//!   them), a changed one or a removal reads the file's `version` and compares it with the base
//!   (a stale base is a `Conflict`, never an overwrite). Drive documents no conditional update,
//!   so another writer can still slip in between the check and the write: the window is one
//!   round trip (FINDINGS). A file up to [`SIMPLE_MAX`] bytes is one multipart request; a larger
//!   one goes in a resumable session, in chunks of a multiple of [`CHUNK_UNIT`] (256 KiB).
//! - **Whole-drive writes** ([`porter_sync::DriveWrite`], in My Drive): a folder or a file made
//!   in a parent by id, new content for a file by id, a rename or move and the trash as
//!   `files.update` (never `files.delete`), each checked against the file's `version` first; a
//!   large file in a resumable session that a new replica resumes ([`accepts_name`]: Drive
//!   takes any name but an empty one).
//! - **Quota** is `about`'s `storageQuota` (`usage`, `limit`).
//! - **The connection** is whatever [`porter_http::Http`] the replica is handed. In syncd it is
//!   `porter_http_relay::RelayHttp` over the relays of accountd's `Tokens.OpenAuthenticated`,
//!   whose relay adds the bearer, so syncd never holds a token. The upload host
//!   (`/upload/drive/v3`) and a resumable session's URL are the same origin as the API, so one
//!   relay serves all; a session URL on another origin is refused.
//! - **Version** is Drive's `version`, which grows with every change to the file (a rename too).
//!   There is no ETag to keep.
//!
//! The crate is pure: no runtime and no socket. It keeps its own vendor error reasons and wire reading on top of porter-sync's
//! `status` and dates its tombstones with porter-sync's `Clock`. Every
//! endpoint, field and status is written from Google's documentation, without network access:
//! unverified (FINDINGS).

mod addr;
mod drive;
mod feed;
mod json;
mod refuse;
mod replica;
mod upload;
mod write;

pub use addr::Root;
pub use drive::accepts_name;
pub use replica::{CHUNK_UNIT, EXPORT_LIMIT, GdriveReplica, SIMPLE_MAX, Uploads};
