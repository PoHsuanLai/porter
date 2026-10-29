//! The account service hosted in the app, as a transport for `porter-client`: the app brings its
//! providers, its secret store and its sheet host, and [`hosted`] gives the [`Porter`] over a
//! service built from them. A daemon with seams of its own (syncd's bus sheets, file store and audit
//! file) builds an `AccountService` itself and uses [`Hosted::new`].
//!
//! The app keeps its own registry and its own credentials: the service is given a `RegistryStore`
//! (none, by default: the registry then lives for the process) and a `Secrets` filed under a
//! namespace of its own. This crate builds neither.
//!
//! A hosted service answers the accounts and the grants. It does not answer the identities, the
//! Spaces or the AI keys, which are accountd's: `Hosted` does not implement their link traits.

mod hosted;
mod relays;

pub use hosted::{HostConfig, Hosted, Hosting, hosted};
#[cfg(feature = "relays")]
pub use relays::PorterRelays;
pub use relays::{NoRelays, RelayHost};
