//! `org.quire.Grants1` at `/org/quire/Grants1`, on accountd's connection: who may use which
//! account, and the relays a grant opens. An app finds the accounts it holds a grant for, asks the
//! person for one (the chooser and consent sheet), and opens a byte stream to one of the account's
//! servers that accountd has already signed in: the stream is a descriptor (`h`, a Unix stream
//! socket), and accountd runs the relay on its other end, so no token and no password crosses the
//! bus or reaches the app.
//!
//! Who may call: an app, and the porter daemons. The shell, Settings, the agent, the agent
//! launcher, the AI broker, the terminal and the compositor are `AccessDenied`, as is a role
//! porter does not know yet and a sender accountd does not know. Whose grants are read, asked for,
//! revoked and used is the caller's own app, judged by its connection.
//!
//! A `need` is the JSON text of a `porter_match::Need`; `class` and `usage` are the words of
//! `DataClass` and `Usage` (`files`, `background`, ...); `kinds` are words of `CapabilityKind`.
//! `Grants` and `Accounts` answer JSON text of the rows (a list of `Grant`, a list of the
//! accounts), because a grant or an account has no flat shape worth a second codec.
//!
//! Errors are `org.quire.Accounts1.Error.<Refusal>` (`UnknownGrant`, `NeedsConsent`,
//! `EndpointNotGranted`, ...), `InvalidArgs` for a text that is none of the above, and
//! `LimitsExceeded` while the caller has a sheet of this kind open.

use super::CandidateArg;
use crate::Details;
use crate::json_value::bad;
use porter_core::CoreError;
use porter_dbus_macros::bus_interface;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

/// The JSON text of `value`: how a need, a list of grants or a list of accounts crosses.
pub fn to_text<T: serde::Serialize>(value: &T) -> String {
    // The values porter sends are plain data: serialising them cannot fail.
    serde_json::to_string(value).unwrap_or_default()
}

/// The value the JSON text `text` carries, or why it is not one.
pub fn from_text<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, CoreError> {
    serde_json::from_str(text).map_err(|e| bad(&format!("grants: {e}")))
}

/// The key of `RequestGrant`'s options whose string names the one account to ask about.
pub const ACCOUNT_OPTION: &str = "account";

/// The interface, written once. `#[bus_interface]` makes [`GrantsProxy`] for its callers from this
/// trait, [`GrantsServer`] for the daemon to implement and [`GrantsServed`] to register.
#[bus_interface(
    interface = "org.quire.Grants1",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Grants1"
)]
pub trait Grants {
    /// The accounts that fit `need` for the caller and that the caller holds a grant for, best
    /// first, as candidates (the codec of [`crate::accounts::candidate_to_dbus`]).
    fn candidates(&self, need: &str, class: &str, usage: &str) -> zbus::Result<Vec<CandidateArg>>;
    /// Whether `need` can be met for the caller, revealing no identity: `granted`,
    /// `available_needs_consent`, `denied`, `needs_account` or `unsupported`.
    fn availability(&self, need: &str, class: &str, usage: &str) -> zbus::Result<String>;
    /// The grants the caller holds, as the JSON text of a list of `Grant`.
    fn grants(&self) -> zbus::Result<String>;
    /// Shows the chooser and consent sheet for `need`; returns a Request object
    /// (`org.quire.Accounts1.Request`) whose `Response` carries the candidate the person
    /// granted. The sheet is drawn each time: what the caller already holds is `Candidates`',
    /// and its answer to a sheet is kept, so a caller asks again only for another account.
    /// `options` may name the one account to ask about under [`ACCOUNT_OPTION`]: the sheet then
    /// has no chooser, and the answer is a grant for that account or a refusal, never another
    /// account. `parent_window` is the app's window the sheet sits over (empty for none).
    fn request_grant(
        &self,
        need: &str,
        class: &str,
        usage: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
    /// Withdraws one of the caller's own grants. Errors: `UnknownGrant` when the caller holds
    /// none of that id.
    fn revoke(&self, grant: &str) -> zbus::Result<()>;
    /// A byte stream to `endpoint`, one of the grant's account's servers, as a Unix stream socket
    /// (`h`) whose other end accountd's relay holds: the relay signs in at the server and strips
    /// what the app sends of its own credentials. Errors: `UnknownGrant`, `EndpointNotGranted`,
    /// `NeedsReauth`, `Unavailable`.
    fn open_authenticated(&self, grant: &str, endpoint: &str) -> zbus::Result<OwnedFd>;
    /// A byte stream to `origin`, a host the account's provider file declares as one its
    /// pre-authenticated links point at (`h`, as `OpenAuthenticated`); the relay adds no
    /// credential.
    fn open_linked(&self, grant: &str, origin: &str) -> zbus::Result<OwnedFd>;
    /// The accounts the person has, as the JSON text of a list of `[account, provider label]`
    /// (the label is empty when accountd has none): for the porter daemons that keep a switch per
    /// account (the sync daemon's Settings rows). Only a porter daemon, Settings or the shell may
    /// ask; an app learns of accounts through its grants.
    fn accounts(&self) -> zbus::Result<String>;
    /// Asks the person, in a sign-in of its own, for the access the account's provider asks only
    /// when it is first used, for `kinds` (the whole of a drive, for "Show in Files"). The call
    /// waits for the person. Errors: `UnknownGrant` for an account accountd does not hold,
    /// `Unavailable` when its provider asks none of `kinds` on use, `Dismissed`, `Denied`;
    /// `LimitsExceeded` while the caller has such a sheet open.
    fn extend_access(&self, account: &str, kinds: &[&str], parent_window: &str)
    -> zbus::Result<()>;
    /// Whether `provider` (a provider file's id) asks some access of `kind` only on use, which
    /// `ExtendAccess` can ask for.
    fn asks_on_use(&self, provider: &str, kind: &str) -> zbus::Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::consent::Availability;

    #[test]
    fn a_value_survives_its_text_and_a_text_that_is_none_is_refused() {
        let text = to_text(&Availability::NeedsAccount);
        assert_eq!(text, "\"needs_account\"");
        assert_eq!(
            from_text::<Availability>(&text),
            Ok(Availability::NeedsAccount)
        );
        assert!(from_text::<Availability>("nonsense").is_err());
    }
}
