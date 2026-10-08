//! `org.quire.Spaces1` at `/org/quire/Spaces1`, on accountd's connection: the registry of
//! desktop-wide Spaces, the ones an app's own Space can link to (porter-core `space`).
//!
//! Who may call: any identified app may `List` and `Create`; `Rename`, `SetLook` and `Remove`
//! only Settings and the shell (`CallerRole::Settings`, `CallerRole::SheetHost`), and anyone else
//! is `AccessDenied`, as is a sender accountd does not know.

use crate::args::Details;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Spaces1",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Spaces1"
)]
pub trait Spaces {
    /// Every desktop-wide Space: its id, then `name` (`s`), `look` (`s`, opaque, as `Create` or
    /// `SetLook` was given it) and `created` (`x`, Unix seconds), in the order they were made.
    fn list(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Makes a desktop-wide Space named `name` (the person's words, 1 to 64 characters) that
    /// looks like `look` (at most 1024 bytes, never read by porter; empty is the default look),
    /// and answers its id, minted here and kept across renames. Errors: `InvalidArgs` for a name
    /// or look out of bounds; `LimitsExceeded` when the caller made too many Spaces within the
    /// last minute.
    fn create(&self, name: &str, look: &str) -> zbus::Result<String>;
    /// Renames one. Errors: `InvalidArgs` for a bad name or a Space that is not there.
    fn rename(&self, id: &str, name: &str) -> zbus::Result<()>;
    /// Gives one a new look. Errors: `InvalidArgs` for a look too long or a Space that is not
    /// there.
    fn set_look(&self, id: &str, look: &str) -> zbus::Result<()>;
    /// Removes one: every grant scoped to that Space alone ends. Errors: `InvalidArgs` for a
    /// Space that is not there.
    fn remove(&self, id: &str) -> zbus::Result<()>;
    /// A Space changed: `what` is `created`, `renamed`, `look` or `removed`. Sent to each
    /// identified connection that has called accountd, so a client calls `List` once to be told.
    #[zbus(signal)]
    fn changed(&self, id: &str, what: &str) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct SpacesSkeleton;

#[zbus::interface(name = "org.quire.Spaces1")]
impl SpacesSkeleton {
    fn list(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn create(&self, name: String, look: String) -> fdo::Result<String> {
        let _ = (name, look);
        Err(crate::introspect::frozen())
    }

    fn rename(&self, id: String, name: String) -> fdo::Result<()> {
        let _ = (id, name);
        Err(crate::introspect::frozen())
    }

    fn set_look(&self, id: String, look: String) -> fdo::Result<()> {
        let _ = (id, look);
        Err(crate::introspect::frozen())
    }

    fn remove(&self, id: String) -> fdo::Result<()> {
        let _ = id;
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>, id: &str, what: &str) -> zbus::Result<()>;
}
