//! `org.quire.Photos1.Picker` on syncd's bus name and object (`org.quire.Sync1` at
//! `/org/quire/Sync1`): the Google Photos Picker as the Photos app calls it. A session is made
//! (`Start`), the person picks in the browser at `picker_uri`, the app polls (`Poll`) until
//! `picked`, then `Import` fetches what was picked into the account's `picked/<session>`
//! folder and answers the files' paths; `Cancel` ends a session. The interface exists only
//! while syncd runs Google Photos (`SYNCD_PHOTOS`); only the Photos app (`org.quire.Photos`)
//! may call it. No token, no URL of Google's bytes, and no picked file's content ever crosses it.

use zbus::fdo;

/// The prefix of the errors this interface names itself (a refusal of accountd's vocabulary
/// keeps its `org.quire.Accounts1.Error.` name).
pub const PICKER_ERROR_PREFIX: &str = "org.quire.Photos1.Error.";
/// `Import` before the person has finished picking (`org.quire.Photos1.Error.NotYet`).
pub const PICKER_ERROR_NOT_YET: &str = "org.quire.Photos1.Error.NotYet";
/// `Poll` or `Import` for a session Google no longer has (`org.quire.Photos1.Error.NoSuchSession`).
pub const PICKER_ERROR_NO_SUCH_SESSION: &str = "org.quire.Photos1.Error.NoSuchSession";
/// `Poll`'s answer while the person is still picking.
pub const PICKER_WAITING: &str = "waiting";
/// `Poll`'s answer once the person has finished.
pub const PICKER_PICKED: &str = "picked";

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Photos1.Picker",
    default_service = "org.quire.Sync1",
    default_path = "/org/quire/Sync1"
)]
pub trait PhotosPicker {
    /// Starts a session for `account` (the account directory name, as in a dataset name
    /// `<account>/<dataset>`): its id, the page the app opens in the browser, and how often to
    /// poll, in seconds.
    fn start(&self, account: &str) -> zbus::Result<(String, String, u32)>;
    /// `waiting` or `picked`.
    fn poll(&self, account: &str, session: &str) -> zbus::Result<String>;
    /// Fetches what was picked into `picked/<session>` of the account's photos folder and
    /// deletes the session at Google; the paths of the files. `org.quire.Photos1.Error.NotYet`
    /// before the person has finished.
    fn import(&self, account: &str, session: &str) -> zbus::Result<Vec<String>>;
    /// Ends a session without importing.
    fn cancel(&self, account: &str, session: &str) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct PickerSkeleton;

#[zbus::interface(name = "org.quire.Photos1.Picker")]
impl PickerSkeleton {
    fn start(&self, account: String) -> fdo::Result<(String, String, u32)> {
        let _ = account;
        Err(crate::introspect::frozen())
    }

    fn poll(&self, account: String, session: String) -> fdo::Result<String> {
        let _ = (account, session);
        Err(crate::introspect::frozen())
    }

    fn import(&self, account: String, session: String) -> fdo::Result<Vec<String>> {
        let _ = (account, session);
        Err(crate::introspect::frozen())
    }

    fn cancel(&self, account: String, session: String) -> fdo::Result<()> {
        let _ = (account, session);
        Err(crate::introspect::frozen())
    }
}
