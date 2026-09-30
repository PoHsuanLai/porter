//! `org.quire.Inference1` at `/org/quire/Inference1`: inferd. Requests travel on the fd `Open`
//! returns, as frames of `porter_infer::InferRequest` and `InferReply`.

use crate::args::{Details, NeedArg};
use zbus::fdo;
use zbus::zvariant::OwnedFd;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Inference1",
    default_service = "org.quire.Inference1",
    default_path = "/org/quire/Inference1"
)]
pub trait Inference {
    /// Whether an AI need can be met for this class, revealing no identity.
    fn availability(&self, need: &NeedArg, class: &str) -> zbus::Result<String>;
    /// A framed request/stream session for `need`, `class` and `tier`.
    fn open(&self, need: &NeedArg, class: &str, tier: &str) -> zbus::Result<OwnedFd>;
    /// The caller's usage this period, by name (tokens, spend, cap).
    fn usage(&self) -> zbus::Result<Details>;
    /// Probes local runtimes again.
    fn rescan(&self) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct InferenceSkeleton;

#[zbus::interface(name = "org.quire.Inference1")]
impl InferenceSkeleton {
    fn availability(&self, need: NeedArg, class: String) -> fdo::Result<String> {
        let _ = (need, class);
        Err(crate::introspect::frozen())
    }

    fn open(&self, need: NeedArg, class: String, tier: String) -> fdo::Result<OwnedFd> {
        let _ = (need, class, tier);
        Err(crate::introspect::frozen())
    }

    fn usage(&self) -> fdo::Result<Details> {
        Err(crate::introspect::frozen())
    }

    fn rescan(&self) -> fdo::Result<()> {
        Err(crate::introspect::frozen())
    }
}
