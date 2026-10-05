//! `org.quire.Accounts1.Peer` at `/org/quire/Accounts1`: what the other daemons ask accountd on
//! behalf of an app (porter PLAN G3). Callable only by a connection whose caller role is
//! `PorterDaemon` (inferd, syncd); accountd refuses every other sender `AccessDenied`. The app
//! is named by the calling daemon from its own connection, never by the app. There is no
//! `OpenCredential`: syncd opens an authenticated stream like any app (`Tokens`).

use crate::args::{AppArg, Details, NeedArg, VerdictArg};
use zbus::fdo;
use zbus::zvariant::OwnedFd;

/// The daemon's side of the conversation.
#[zbus::proxy(
    interface = "org.quire.Accounts1.Peer",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Peer {
    /// What the consent store says for `app` on each account whose offer fits `need`, so inferd
    /// can route to a cloud account only when the app holds a grant. Reveals no secret.
    fn verdicts(
        &self,
        app: &AppArg,
        need: &NeedArg,
        class: &str,
        usage: &str,
    ) -> zbus::Result<Vec<VerdictArg>>;
    /// The API key of a granted account, on a sealed memfd and never a string on the bus.
    fn resolve_key(&self, grant: &str) -> zbus::Result<OwnedFd>;
    /// Reports a probed local runtime as an account of `provider` with its models as claims
    /// (kind slug and the claim's fields by name, as `Account.Capabilities`), in `state`
    /// (`ok`, `offline`); returns the account id.
    fn report_local(
        &self,
        provider: &str,
        claims: Vec<(String, Details)>,
        state: &str,
    ) -> zbus::Result<String>;
}

/// accountd's side.
#[derive(Debug, Default)]
pub struct PeerSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Peer")]
impl PeerSkeleton {
    fn verdicts(
        &self,
        app: AppArg,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> fdo::Result<Vec<VerdictArg>> {
        let _ = (app, need, class, usage);
        Err(crate::introspect::frozen())
    }

    fn resolve_key(&self, grant: String) -> fdo::Result<OwnedFd> {
        let _ = grant;
        Err(crate::introspect::frozen())
    }

    fn report_local(
        &self,
        provider: String,
        claims: Vec<(String, Details)>,
        state: String,
    ) -> fdo::Result<String> {
        let _ = (provider, claims, state);
        Err(crate::introspect::frozen())
    }
}
