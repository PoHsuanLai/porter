//! `org.quire.Inference1.Keys` at `/org/quire/Inference1` on inferd's name: how an app gets the
//! person's yes to use their AI keys, and how a key is added. Whoever uses a credential holds
//! it, so the AI keys are inferd's (an app never sees one): the app asks inferd, inferd shows the
//! consent sheet, and the answer is a permit.
//!
//! Both methods return a Request object (the portal shape, [`crate::Sheet`]) on inferd's name,
//! under `/org/quire/Inference1/request` ([`crate::ServiceRoot::Inference`]):
//! - `RequestKey(class, usage, parent_window, options)`: the consent sheet for the caller's use of
//!   one of the person's AI keys for data of `class`, used `usage` (`interactive`, `background`).
//!   Answered with the results of a `Choose` response (the chosen key's account) whose `grant` is
//!   the permit's id: a [`porter_core::GrantId`].
//! - `AddAccount(provider_hint, parent_window, options)`: the sheet that adds an AI key or an
//!   agent's account. Answered as `Manager.AddAccount` is (`account`, `path`).
//!
//! There is no listing and no revoking here: Settings lists and revokes permits through inferd's
//! `SettingsModule1` rows. Any app may call; the agent launcher, the computer-use daemon, the
//! terminal and the compositor are refused `AccessDenied` (a role the table does not name fails
//! closed).

use crate::args::Details;
use zbus::fdo;
use zbus::zvariant::OwnedObjectPath;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Inference1.Keys",
    default_service = "org.quire.Inference1",
    default_path = "/org/quire/Inference1"
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
    /// Shows the sheet that adds an AI key or an agent's account; returns a Request object.
    fn add_account(
        &self,
        provider_hint: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
}

/// inferd's side: the interface's shape, every method answering `NotSupported`.
#[derive(Debug, Default)]
pub struct KeysSkeleton;

#[zbus::interface(name = "org.quire.Inference1.Keys")]
impl KeysSkeleton {
    fn request_key(
        &self,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (class, usage, parent_window, options);
        Err(crate::frozen::error())
    }

    fn add_account(
        &self,
        provider_hint: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (provider_hint, parent_window, options);
        Err(crate::frozen::error())
    }
}

/// Writes this interface's introspection into `xml`, as an `<interface>` element one level in:
/// the bus's introspection collects each interface's own.
pub(crate) fn introspect(xml: &mut String) {
    zbus::object_server::Interface::introspect_to_writer(&KeysSkeleton, xml, 1);
}
