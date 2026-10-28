//! A [keyring-core](keyring_core) credential store that keeps a secret too long for one entry in
//! several entries, and puts it back together when it is read.
//!
//! The Windows Credential Manager holds at most 2560 bytes in an entry, and a password is kept
//! there as UTF-16, so 1280 code units. A Microsoft sign-in's access token alone is a JWT of one
//! to two thousand characters, and an OAuth credential is that, its refresh token and its expiry;
//! an S/MIME key in PEM is longer still. Stored whole, `set_password` fails with the store's
//! `TooLong`.
//!
//! [`ChunkedStore`] wraps any other store. A value over its [`Limit`] is cut into parts, each in
//! an entry of its own named `<user>#1`, `<user>#2` and so on, and the entry `<user>` holds only
//! a head: the store's marker followed by how many parts there are, as in `porter-parts:3`. A
//! value that fits is stored whole in `<user>`, exactly as the wrapped store would have stored
//! it, so a store with no limit (the Keychain, the Secret Service) holds what it always held.
//!
//! - **Shrinking.** Parts left over from a longer value stored earlier are removed once the head
//!   names the new count, and so is any run of parts past them that an earlier write left.
//! - **A write that fails part-way.** When the old value was in parts and some of them were
//!   already overwritten, the head and the parts are removed, so the entry reads as missing
//!   rather than as a mix of two values. When the old value was whole, it is left as it was and
//!   the new parts are removed. Either way the error is returned. Should that clean-up fail too,
//!   a read can return a mix; values that check themselves (JSON) then do not parse.
//! - **A damaged head or a missing part** reads as [`Error::BadDataFormat`], whose source is a
//!   [`ChunkError`], never as a shorter value. Deleting the entry still removes every part.
//! - **A value that begins with the marker** is always stored in parts, so it can never be
//!   mistaken for a head.
//!
//! ```
//! use keyring_chunked::{ChunkedStore, Limit};
//! use keyring_core::api::CredentialStoreApi;
//!
//! let mock = keyring_core::mock::Store::new().unwrap();
//! let store = ChunkedStore::new(mock.clone(), "app-parts:", Limit::Utf16Units(16)).unwrap();
//! let entry = store.build("app", "token", None).unwrap();
//! entry.set_password("a value well over sixteen units long").unwrap();
//! assert_eq!(entry.get_password().unwrap(), "a value well over sixteen units long");
//!
//! // Underneath: a head and three parts.
//! let head = mock.build("app", "token", None).unwrap();
//! assert_eq!(head.get_password().unwrap(), "app-parts:3");
//! let part = mock.build("app", "token#3", None).unwrap();
//! assert_eq!(part.get_password().unwrap(), "long");
//! ```

#![warn(missing_docs)]

mod credential;
mod pieces;
mod store;

pub use credential::ChunkedCredential;
pub use keyring_core::Error;
pub use store::ChunkedStore;

/// How many parts one value may be cut into. More than this in a head is a damaged head, not a
/// credential: sixty-four parts are some eighty thousand characters on Windows.
pub const MOST_PARTS: usize = 64;

/// How long one entry's value may be in the wrapped store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Any length: every value is stored whole (the Keychain, the Secret Service).
    None,
    /// At most this many UTF-16 code units in a password, and twice as many bytes in a secret
    /// given as bytes: what the Windows Credential Manager holds at 1280.
    Utf16Units(usize),
}

impl Limit {
    /// The limit of the Windows Credential Manager: 2560 bytes, 1280 UTF-16 code units.
    pub const WINDOWS: Limit = Limit::Utf16Units(1280);
}

/// What is wrong with a value stored in parts, or with a store's settings.
///
/// A read returns it as the source of an [`Error::BadDataFormat`]; downcast the boxed error to
/// tell it from the wrapped store's own.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ChunkError {
    /// The head names more parts than were found.
    #[error("the stored value is incomplete: part {index} of {count} is missing")]
    MissingPart {
        /// The first part that was not found, from 1.
        index: usize,
        /// How many parts the head names.
        count: usize,
    },
    /// The head begins with the marker but does not name a part count from 2 to
    /// [`MOST_PARTS`].
    #[error(
        "the stored value is unreadable: its head names no part count from 2 to {}",
        MOST_PARTS
    )]
    BadHead,
    /// The marker given to [`ChunkedStore::new`] is empty, so every value would begin with it.
    #[error("the marker of a chunked store cannot be empty")]
    EmptyMarker,
}
