//! `org.quire.Accounts1.Keys` at `/org/quire/Accounts1`, on accountd's name: how an app asks the
//! person for the use of their AI keys. accountd keeps the keys and the permits (one grant
//! store), so the app asks accountd; accountd shows the consent sheet, and the answer is a
//! permit. An app never sees a key.
//!
//! The one method returns a Request object (the portal shape, [`crate::accounts::Sheet`]) on
//! accountd's name, under `/org/quire/Accounts1/request`
//! ([`crate::accounts::ServiceRoot::Accounts`]):
//! - `RequestKey(class, usage, parent_window, options)`: the consent sheet for the caller's use of
//!   one of the person's AI keys for data of `class`, used `usage` (`interactive`, `background`).
//!   Answered with the results of a `Choose` response (the chosen key's account) whose `grant` is
//!   the permit's id: a [`porter_core::GrantId`]. A caller that already holds a permit is
//!   answered with it at once and no sheet is drawn.
//!
//! Adding an AI key is `Manager.AddAccount`, the same sheet as for any account. There is no
//! listing and no revoking here: Settings lists and revokes permits through accountd's settings
//! rows. Any app may call; the agent, the agent launcher, the computer-use daemon, the terminal
//! and the compositor are refused `AccessDenied` (a role the table does not name fails closed).

use crate::Details;
use porter_dbus_macros::bus_interface;
use zbus::zvariant::OwnedObjectPath;

/// The interface, written once. `#[bus_interface]` makes [`KeysProxy`] for its callers from this
/// trait, [`KeysServer`] for the daemon to implement and [`KeysServed`] to register.
#[bus_interface(
    interface = "org.quire.Accounts1.Keys",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Keys {
    /// Shows the consent sheet for the caller's use of an AI key; returns a Request object.
    fn request_key(
        &self,
        class: &str,
        usage: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
}
