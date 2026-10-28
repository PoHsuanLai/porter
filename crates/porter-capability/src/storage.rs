//! Storage (design/31 §2.2; R2).

use super::terms::{Access, Delta};
use serde::{Deserialize, Serialize};

/// A file store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StorageCap {
    /// Reading and writing files.
    pub access: Access,
    /// How changes become known.
    pub delta: Delta,
    /// Whether used and total space are reported.
    #[serde(with = "crate::terms::reported")]
    pub quota: bool,
    /// How much of the store the account can see.
    pub scope: StorageScope,
    /// The content hash the store reports, if any.
    pub hashes: HashKind,
    /// Partial downloads (HTTP ranges).
    #[serde(with = "crate::terms::present")]
    pub ranges: bool,
    /// Resumable uploads in chunks.
    #[serde(with = "crate::terms::present")]
    pub chunked_upload: bool,
}

/// How much of a store the account reaches. Ordered: a need for `AppFolder` is met by `Full`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageScope {
    /// Only the files this desktop created (Drive `drive.file`, Dropbox app folder).
    AppFolder,
    /// The whole store.
    Full,
}

/// The content hash a store reports beside each file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HashKind {
    /// None; identity is by etag or version only.
    None,
    /// MD5.
    Md5,
    /// SHA-1.
    Sha1,
    /// SHA-256.
    Sha256,
    /// OneDrive's QuickXorHash.
    QuickXor,
    /// Dropbox's content hash.
    Dropbox,
}
