//! porter's D-Bus API (design/31 §4.4): the buses of accountd (identities, Spaces, Tailnet),
//! inferd (inference, agents, the agent launcher, AI keys) and syncd, portal-shaped (a method
//! that shows UI returns a Request object whose `Response` signal carries the answer). Whoever
//! uses a credential holds it, so no bus carries a token or a grant: the app that wants an
//! identity asks `Identities1`, the one that wants an AI key asks inferd's `Keys`. Each interface
//! is declared twice from one table: a proxy trait for callers and a skeleton for the daemons,
//! whose introspection is the checked-in `dbus/*.xml` (see `tests/introspection.rs`).
//! The skeletons are signatures only (every method answers `NotSupported`); the daemons serve the
//! real objects. `sheet` and `pending` are the portal shape of a sheet's answer, shared by both
//! sides: the request path, the response codes and results, and the caller's listener. The
//! Request interface is one shape for the three services that show sheets; [`ServiceRoot`] says
//! which service's bus name, request path root and error prefix a call is under.
//! [`serve_ready`] is the daemons' start: the wait between registering their objects and
//! claiming their name.
//!
//! Three groups have a module of their own, and are named through it:
//! - [`names`]: the bus names, object paths, option keys and error prefixes;
//! - [`callers`]: who is calling (`CallerTable`, `ProcCallers`, `CallerRole`, `Caller`, and with
//!   feature `callers-file` `load_callers`);
//! - [`sheet`]: the portal shape of a sheet's answer (the request path helpers, the response
//!   codes).

mod account;
mod agents;
mod args;
pub mod callers;
#[cfg(feature = "callers-file")]
mod callers_file;
mod codec;
mod failure;
mod frozen;
mod identities;
mod inference;
mod introspect;
mod json_value;
mod keys;
mod launcher;
mod launcher_iface;
mod local_engines;
mod machines;
mod manager;
pub mod names;
mod pending;
#[cfg(feature = "photos-picker")]
mod photos_picker;
mod ready;
mod refusal;
mod request;
mod running;
mod service_root;
pub mod sheet;
mod sheet_backend;
mod spaces;
mod storage;
mod sync;
mod tailnet;
mod target;

pub use account::{AccountProxy, AccountSkeleton};
pub use agents::{
    AGENT_ERROR_PREFIX, AgentsProxy, AgentsSkeleton, EndpointArg, MODELS_ANY, MODELS_LISTED,
    ROUTE_ACCOUNT, ROUTE_MODEL, RouteArg,
};
pub use args::{CandidateArg, Details, NeedArg};
pub use codec::{candidate_from_dbus, candidate_to_dbus, need_from_dbus, need_to_dbus};
pub use failure::{
    BusFailure, PLACE_ERROR_PREFIX, classify, computer_refusal_of, is_invalid_args,
    is_limits_exceeded, launcher_fault_of, place_refusal_of, refusal_of,
};
pub use identities::{
    IDENTITY_KEY_ADDRESS, IDENTITY_KEY_KIND, IDENTITY_KEY_NAME, IDENTITY_KEY_PROVIDER,
    IdentitiesProxy, IdentitiesSkeleton, IdentityRemoved, IdentityRemovedStream,
    identity_from_dbus, identity_to_dbus,
};
pub use inference::{
    GuestAsks, GuestAsksStream, GuestsChanged, GuestsChangedStream, InferenceProxy,
    InferenceSkeleton,
};
pub use introspect::{Bus, introspection};
pub use json_value::{from_vardict, to_vardict};
pub use keys::{KeysProxy, KeysSkeleton};
pub use launcher::LauncherFault;
pub use launcher_iface::{
    AgentLoginRequested, AgentLoginRequestedStream, AgentLogoutRequested,
    AgentLogoutRequestedStream, LauncherProxy, LauncherSkeleton, ProcessCredentialRevoked,
    ProcessCredentialRevokedStream,
};
pub use local_engines::{
    ENGINE_KEY_NAME, ENGINE_KEY_PATH, ENGINE_KEY_PLACE, ENGINE_KEY_SLOTS, ENGINE_KEY_STATE,
    ENGINE_KEY_VERSION, EnginePlace, EngineRowError, EngineState, LocalEngine,
    local_engine_from_dbus, local_engine_to_dbus,
};
pub use machines::{
    MACHINE_KEY_ADDRESSES, MACHINE_KEY_DNS, MACHINE_KEY_LAST_SEEN, MACHINE_KEY_NAME,
    MACHINE_KEY_NODE, MACHINE_KEY_ONLINE, MACHINE_KEY_OS, MACHINE_KEY_OWNER, MACHINE_KEY_SSH,
    MACHINE_KEY_SSH_HOST_KEYS, machine_from_dbus, machine_to_dbus,
};
pub use manager::{AccountRemoved, AccountRemovedStream, ManagerProxy, ManagerSkeleton};
pub use pending::{Closer, Sheet, SheetError};
#[cfg(feature = "photos-picker")]
pub use photos_picker::{
    PICKER_ERROR_NO_SUCH_SESSION, PICKER_ERROR_NOT_YET, PICKER_ERROR_PREFIX, PICKER_PICKED,
    PICKER_WAITING, PhotosPickerProxy, PickerSkeleton,
};
pub use ready::serve_ready;
pub use refusal::{
    REFUSAL_ERROR_PREFIX, refusal_error_name, refusal_error_name_in, refusal_from_error_name,
};
pub use request::{RequestProxy, RequestSkeleton};
pub use running::{
    RUNNING_KEY_ENGINE, RUNNING_KEY_MEMORY_MIB, RUNNING_KEY_REASON, RUNNING_KEY_SINCE,
    RUNNING_KEY_STATE, RUNNING_KEY_WINDOW, RunState, RunningModel, RunningRowError,
    running_from_dbus, running_to_dbus,
};
pub use service_root::ServiceRoot;
pub use sheet_backend::{AccountsSheetProxy, AccountsSheetSkeleton};
pub use spaces::{
    Changed as SpaceChanged, ChangedStream as SpaceChangedStream, SpacesProxy, SpacesSkeleton,
};
pub use storage::{
    DRIVE_KEY_ACCOUNT, DRIVE_KEY_LABEL, DRIVE_KEY_LOCAL, DRIVE_KEY_PATH, DRIVE_KEY_PROVIDER,
    DRIVE_KEY_STATE, DRIVE_KEY_TOTAL, DRIVE_KEY_USED, DRIVE_KEY_WORDS, DriveState, DrivesChanged,
    DrivesChangedStream, ITEM_KEY_FRACTION, ITEM_KEY_KEPT, ITEM_KEY_KIND, ITEM_KEY_OTHER,
    ITEM_KEY_STATE, ITEM_KEY_URL, ITEM_KEY_WORDS, ItemDetails, ItemKind, ItemState, ItemsChanged,
    ItemsChangedStream, Refused, Saved, StorageProxy, StorageSkeleton,
};
pub use sync::{SyncProxy, SyncSkeleton};
pub use tailnet::{
    Changed as TailnetChanged, ChangedStream as TailnetChangedStream, TailnetProxy, TailnetSkeleton,
};
pub use target::BusTarget;
/// The session-bus connection transports and daemons hold.
pub use zbus::Connection as BusConnection;
/// The bus library's error, which [`classify`] reads.
pub use zbus::Error as BusError;
/// The stream trait the proxies' signal streams implement.
pub use zbus::export::futures_core::Stream as BusStream;
/// The value types of a vardict, so a caller builds `Details` without its own zbus edge.
pub use zbus::zvariant;
