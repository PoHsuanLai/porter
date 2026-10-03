//! porter's D-Bus API (design/31 §4.4): three buses, portal-shaped (a method that shows UI
//! returns a Request object whose `Response` signal carries the answer). Each interface is
//! declared twice from one table: a proxy trait for callers and a skeleton for the daemons,
//! whose introspection is the checked-in `dbus/*.xml` (see `tests/introspection.rs`).
//! Signatures only: every skeleton method answers `NotSupported`.

mod account;
mod args;
mod codec;
mod failure;
mod grants;
mod inference;
mod introspect;
mod json_value;
mod manager;
mod names;
mod request;
mod sync;
mod tokens;

pub use account::{AccountProxy, AccountSkeleton};
pub use args::{CandidateArg, Details, NeedArg, TokenArg};
pub use codec::{candidate_from_dbus, candidate_to_dbus, need_from_dbus, need_to_dbus};
pub use failure::{BusFailure, classify};
pub use grants::{GrantsProxy, GrantsSkeleton};
pub use inference::{InferenceProxy, InferenceSkeleton};
pub use introspect::{Bus, introspection};
pub use manager::{ManagerProxy, ManagerSkeleton};
pub use names::{
    ACCOUNTS_BUS, ACCOUNTS_PATH, INFERENCE_BUS, INFERENCE_PATH, INFERENCE_SETTINGS_PATH,
    OPTION_TRACEPARENT, SYNC_BUS, SYNC_PATH, account_path,
};
pub use request::{RequestProxy, RequestSkeleton};
pub use sync::{SyncProxy, SyncSkeleton};
pub use tokens::{TokensProxy, TokensSkeleton};
/// The session-bus connection transports and daemons hold.
pub use zbus::Connection as BusConnection;
/// The bus library's error, which [`classify`] reads.
pub use zbus::Error as BusError;
/// The value types of a vardict, so a caller builds `Details` without its own zbus edge.
pub use zbus::zvariant;
