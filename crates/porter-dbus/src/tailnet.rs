//! `org.quire.Tailnet1` at `/org/quire/Tailnet1`, on accountd's connection: the person's own
//! computers on their Tailscale network, for the shell, Settings and temor. temor never asks
//! Tailscale itself: accountd does, once, through the account the person added.
//!
//! Who may call: the shell (`CallerRole::SheetHost`), Settings (`CallerRole::Settings`) and the
//! terminal (`CallerRole::Terminal`, temor, by its unit as the systemd manager says its main
//! process is). Anyone else, and a sender accountd does not know, is `AccessDenied`.

use crate::args::Details;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Tailnet1",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Tailnet1"
)]
pub trait Tailnet {
    /// The other computers on the person's network, one row per computer and none for this one:
    /// the row's first part is the computer's Tailscale node id (kept across renames and address
    /// changes), then a vardict of `node` (`s`, the same id), `name` (`s`, what people call it),
    /// `dns` (`s`, its MagicDNS name without the dot at the end: what to connect to),
    /// `addresses` (`as`, its Tailscale addresses), `os` (`s`), `online` (`b`), `last_seen`
    /// (`x`, Unix seconds; absent while it is online and when unknown), `ssh` (`b`, Tailscale's
    /// SSH is on) and `ssh_host_keys` (`as`), and `owner` (`s`: `mine`, `shared` or `tagged`).
    /// With no Tailscale account in porter, or one that is not working, the list is empty: the
    /// account's own state says why.
    fn machines(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// The computers changed. No payload: a listener calls `Machines` again. A burst of changes
    /// is one signal, and there is at most one a second. Sent to each allowed connection that
    /// has called accountd.
    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct TailnetSkeleton;

#[zbus::interface(name = "org.quire.Tailnet1")]
impl TailnetSkeleton {
    fn machines(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
