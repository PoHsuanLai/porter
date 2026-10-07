//! porter's D-Bus API (design/31 §4.4): three buses, portal-shaped (a method that shows UI
//! returns a Request object whose `Response` signal carries the answer). Each interface is
//! declared twice from one table: a proxy trait for callers and a skeleton for the daemons,
//! whose introspection is the checked-in `dbus/*.xml` (see `tests/introspection.rs`).
//! The skeletons are signatures only (every method answers `NotSupported`); accountd serves the
//! real objects. `sheet` and `pending` are the portal shape of a sheet's answer, shared by both
//! sides: the request path, the response codes and results, and the caller's listener.

mod account;
mod args;
mod callers;
mod codec;
mod codec_grants;
mod codec_legacy;
mod failure;
mod grants;
mod inference;
mod introspect;
mod json_value;
mod manager;
mod names;
mod peer;
mod pending;
mod refusal;
mod request;
mod sheet;
mod sheet_backend;
mod sync;
mod tokens;

pub use account::{AccountProxy, AccountSkeleton};
pub use args::{AppArg, CandidateArg, Details, NeedArg, TokenArg, VerdictArg};
pub use callers::{AppTitle, Caller, CallerRole, CallerRow, CallerTable, Callers, ProcCallers};
pub use codec::{candidate_from_dbus, candidate_to_dbus, need_from_dbus, need_to_dbus};
pub use codec_grants::{grant_from_dbus, grant_to_dbus, token_from_dbus, token_to_dbus};
pub use codec_legacy::{legacy_from_dbus, legacy_to_dbus};
pub use failure::{BusFailure, classify, refusal_of};
pub use grants::{GrantsProxy, GrantsSkeleton};
pub use inference::{InferenceProxy, InferenceSkeleton};
pub use introspect::{Bus, introspection};
pub use json_value::{from_vardict, to_vardict};
pub use manager::{ManagerProxy, ManagerSkeleton};
pub use names::{
    ACCOUNTS_BUS, ACCOUNTS_PATH, ACCOUNTS_SETTINGS_PATH, CONFLICT_KEY_NUMBER, INFERENCE_BUS,
    INFERENCE_PATH, INFERENCE_SETTINGS_PATH, OPTION_TRACEPARENT, OPTION_USAGE, RESOLVE_KEEP_LOCAL,
    RESOLVE_KEEP_REMOTE, SHEET_BUS, SHEET_PATH, STATUS_KEY_QUOTA, SYNC_BUS,
    SYNC_ERROR_NO_SUCH_CONFLICT, SYNC_ERROR_PREFIX, SYNC_PATH, account_path,
};
pub use peer::{PeerProxy, PeerSkeleton};
pub use pending::{Closer, Sheet, SheetError};
pub use refusal::{REFUSAL_ERROR_PREFIX, refusal_error_name, refusal_from_error_name};
pub use request::{RequestProxy, RequestSkeleton};
pub use sheet::{
    OPTION_HANDLE_TOKEN, REQUEST_INTERFACE, REQUEST_PATH_ROOT, Response, ResponseCode, SheetKind,
    is_handle_token, reply_of, request_namespace, request_path, response_of, sender_segment,
};
pub use sheet_backend::{AccountsSheetProxy, AccountsSheetSkeleton};
pub use sync::{SyncProxy, SyncSkeleton};
pub use tokens::{TokensProxy, TokensSkeleton};
/// The session-bus connection transports and daemons hold.
pub use zbus::Connection as BusConnection;
/// The bus library's error, which [`classify`] reads.
pub use zbus::Error as BusError;
/// The value types of a vardict, so a caller builds `Details` without its own zbus edge.
pub use zbus::zvariant;
