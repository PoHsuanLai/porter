//! porter's D-Bus API (design/31 §4.4): three buses, portal-shaped (a method that shows UI
//! returns a Request object whose `Response` signal carries the answer). Each interface is
//! declared twice from one table: a proxy trait for callers and a skeleton for the daemons,
//! whose introspection is the checked-in `dbus/*.xml` (see `tests/introspection.rs`).
//! The skeletons are signatures only (every method answers `NotSupported`); accountd serves the
//! real objects. `sheet` and `pending` are the portal shape of a sheet's answer, shared by both
//! sides: the request path, the response codes and results, and the caller's listener.
//! [`serve_ready`] is the daemons' start: the wait between registering their objects and
//! claiming their name.

mod account;
mod agents;
mod args;
mod callers;
#[cfg(feature = "callers-file")]
mod callers_file;
mod codec;
mod codec_grants;
mod failure;
mod grants;
mod inference;
mod introspect;
mod json_value;
mod launcher;
mod manager;
mod names;
mod peer;
mod pending;
#[cfg(feature = "photos-picker")]
mod photos_picker;
mod ready;
mod refusal;
mod request;
mod sheet;
mod sheet_backend;
mod spaces;
mod sync;
mod tokens;

pub use account::{AccountProxy, AccountSkeleton};
pub use agents::{
    AGENT_ERROR_PREFIX, AgentsProxy, AgentsSkeleton, EndpointArg, MODELS_ANY, MODELS_LISTED,
    ROUTE_ACCOUNT, ROUTE_MODEL, RouteArg,
};
pub use args::{AppArg, CandidateArg, Details, NeedArg, TokenArg, VerdictArg};
pub use callers::{
    AppTitle, Caller, CallerRole, CallerRow, CallerTable, Callers, MainPids, ProcCallers,
};
#[cfg(feature = "callers-file")]
pub use callers_file::{CallerFileError, load_callers, table_from_file, table_from_toml};
pub use codec::{candidate_from_dbus, candidate_to_dbus, need_from_dbus, need_to_dbus};
pub use codec_grants::{grant_from_dbus, grant_to_dbus, token_from_dbus, token_to_dbus};
pub use failure::{
    BusFailure, PLACE_ERROR_PREFIX, classify, is_invalid_args, is_limits_exceeded,
    launcher_fault_of, place_refusal_of, refusal_of,
};
pub use grants::{GrantsProxy, GrantsSkeleton};
pub use inference::{InferenceProxy, InferenceSkeleton};
pub use introspect::{Bus, introspection};
pub use json_value::{from_vardict, to_vardict};
pub use launcher::LauncherFault;
pub use manager::{ManagerProxy, ManagerSkeleton};
pub use names::{
    ACCOUNTS_BUS, ACCOUNTS_PATH, ACCOUNTS_SETTINGS_PATH, COMPUTER_ERROR_PREFIX, COMPUTER_KEY_KEY,
    COMPUTER_KEY_PORT, COMPUTER_KEY_SOCKET, CONFLICT_KEY_NUMBER, INFERENCE_BUS, INFERENCE_PATH,
    INFERENCE_SETTINGS_PATH, OPTION_PLACE_MODELS, OPTION_PLACES, OPTION_TRACEPARENT, OPTION_USAGE,
    PLACE_KEY_KIND, PLACE_KEY_MODELS, PLACE_KEY_NAME, PLACE_KEY_PROVIDER, PLACE_KEY_READY,
    RESOLVE_KEEP_LOCAL, RESOLVE_KEEP_REMOTE, SHEET_BUS, SHEET_PATH, SPACE_KEY_CREATED,
    SPACE_KEY_LOOK, SPACE_KEY_NAME, SPACES_PATH, STATUS_KEY_QUOTA, SYNC_BUS,
    SYNC_ERROR_NO_SUCH_CONFLICT, SYNC_ERROR_NOTHING_HELD, SYNC_ERROR_PREFIX, SYNC_PATH,
    account_path,
};
pub use peer::{
    AgentLoginRequested, AgentLoginRequestedStream, AgentLogoutRequested,
    AgentLogoutRequestedStream, PeerProxy, PeerSkeleton,
};
pub use pending::{Closer, Sheet, SheetError};
#[cfg(feature = "photos-picker")]
pub use photos_picker::{
    PICKER_ERROR_NO_SUCH_SESSION, PICKER_ERROR_NOT_YET, PICKER_ERROR_PREFIX, PICKER_PICKED,
    PICKER_WAITING, PhotosPickerProxy, PickerSkeleton,
};
pub use ready::serve_ready;
pub use refusal::{REFUSAL_ERROR_PREFIX, refusal_error_name, refusal_from_error_name};
pub use request::{RequestProxy, RequestSkeleton};
pub use sheet::{
    OPTION_HANDLE_TOKEN, REQUEST_INTERFACE, REQUEST_PATH_ROOT, Response, ResponseCode, SheetKind,
    is_handle_token, reply_of, request_namespace, request_path, response_of, sender_segment,
};
pub use sheet_backend::{AccountsSheetProxy, AccountsSheetSkeleton};
pub use spaces::{
    Changed as SpaceChanged, ChangedStream as SpaceChangedStream, SpacesProxy, SpacesSkeleton,
};
pub use sync::{SyncProxy, SyncSkeleton};
pub use tokens::{
    ProcessCredentialRevoked, ProcessCredentialRevokedStream, TokensProxy, TokensSkeleton,
};
/// The session-bus connection transports and daemons hold.
pub use zbus::Connection as BusConnection;
/// The bus library's error, which [`classify`] reads.
pub use zbus::Error as BusError;
/// The stream trait the proxies' signal streams implement.
pub use zbus::export::futures_core::Stream as BusStream;
/// The value types of a vardict, so a caller builds `Details` without its own zbus edge.
pub use zbus::zvariant;
