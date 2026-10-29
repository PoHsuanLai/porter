//! accountd's interfaces and vocabulary: the add-account sheet (`Manager`, `Account`, and the
//! `Request` object every sheet service returns), the sheet host's side (`AccountsSheet`), the
//! identities signed in on this machine (`Identities`), the desktop-wide Spaces (`Spaces`), the
//! person's computers on their Tailscale network (`Tailnet`), and the AI keys accountd keeps (`Keys`
//! for an app's permit, `Launcher` for the agent launcher, `Broker` for the daemon that sends a
//! key to an AI company), with the bus names, object paths and row keys that go with them; and the portal shape of a sheet's answer ([`sheet`], [`Sheet`]),
//! which the three services that show a sheet share ([`ServiceRoot`]).

mod account;
mod broker;
mod candidate;
mod identities;
mod keys;
mod launcher;
mod launcher_fault;
mod machines;
mod manager;
mod names;
mod pending;
mod refusal;
mod request;
mod service_root;
pub mod sheet;
mod sheet_backend;
mod spaces;
mod tailnet;

use crate::Document;
use crate::Unserved;

pub use account::{AccountProxy, AccountServed, AccountServer};
pub use broker::{
    BrokerProxy, BrokerServed, BrokerServer, Changed as BrokerChanged,
    ChangedStream as BrokerChangedStream, RowArg, ai_account_from_dbus, ai_account_to_dbus,
    own_server_from_dbus, own_server_to_dbus, verdict_from_dbus, verdict_to_dbus,
};
pub use candidate::{CandidateArg, candidate_from_dbus, candidate_to_dbus};
pub use identities::{
    IDENTITY_KEY_ADDRESS, IDENTITY_KEY_KIND, IDENTITY_KEY_NAME, IDENTITY_KEY_PROVIDER,
    IdentitiesProxy, IdentitiesServed, IdentitiesServer, IdentityRemoved, IdentityRemovedStream,
    identity_from_dbus, identity_to_dbus,
};
pub use keys::{KeysProxy, KeysServed, KeysServer};
pub use launcher::{
    AgentLoginRequested, AgentLoginRequestedStream, AgentLogoutRequested,
    AgentLogoutRequestedStream, LauncherProxy, LauncherServed, LauncherServer,
    ProcessCredentialRevoked, ProcessCredentialRevokedStream,
};
pub use launcher_fault::{
    launcher_fault_error_name, launcher_fault_error_name_in, launcher_fault_from_error_name,
    launcher_fault_of,
};
pub use machines::{
    MACHINE_KEY_ADDRESSES, MACHINE_KEY_DNS, MACHINE_KEY_LAST_SEEN, MACHINE_KEY_NAME,
    MACHINE_KEY_NODE, MACHINE_KEY_ONLINE, MACHINE_KEY_OS, MACHINE_KEY_OWNER, MACHINE_KEY_SSH,
    MACHINE_KEY_SSH_HOST_KEYS, machine_from_dbus, machine_to_dbus,
};
pub use manager::{
    AccountAdded, AccountAddedStream, AccountRemoved, AccountRemovedStream, ManagerProxy,
    ManagerServed, ManagerServer,
};
pub use names::*;
pub use pending::{Abandon, Closer, Sheet, SheetError};
pub use refusal::{
    REFUSAL_ERROR_PREFIX, refusal_error_name, refusal_error_name_in, refusal_from_error_name,
    refusal_of,
};
pub use request::{RequestProxy, RequestServed, RequestServer};
pub use service_root::ServiceRoot;
pub use sheet_backend::{AccountsSheetProxy, AccountsSheetServed, AccountsSheetServer};
pub use spaces::{
    Changed as SpaceChanged, ChangedStream as SpaceChangedStream, SpacesProxy, SpacesServed,
    SpacesServer,
};
pub use tailnet::{
    Changed as TailnetChanged, ChangedStream as TailnetChangedStream, TailnetProxy, TailnetServed,
    TailnetServer,
};

/// `org.quire.Accounts1`: the add-account sheet, the accounts' objects and the Request object.
pub const ACCOUNTS: Document = Document {
    file_name: "org.quire.Accounts1.xml",
    interfaces: &[
        ManagerServed::<Unserved>::introspect,
        AccountServed::<Unserved>::introspect,
        RequestServed::<Unserved>::introspect,
    ],
};
/// `org.quire.Accounts1.Keys`, on accountd's bus name.
pub const KEYS: Document = Document {
    file_name: "org.quire.Accounts1.Keys.xml",
    interfaces: &[KeysServed::<Unserved>::introspect],
};
/// `org.quire.Accounts1.Launcher`, on accountd's bus name.
pub const LAUNCHER: Document = Document {
    file_name: "org.quire.Accounts1.Launcher.xml",
    interfaces: &[LauncherServed::<Unserved>::introspect],
};
/// `org.quire.Accounts1.Broker`, on accountd's bus name.
pub const BROKER: Document = Document {
    file_name: "org.quire.Accounts1.Broker.xml",
    interfaces: &[BrokerServed::<Unserved>::introspect],
};
/// `org.quire.AccountsSheet1`, the sheet host's.
pub const ACCOUNTS_SHEET: Document = Document {
    file_name: "org.quire.AccountsSheet1.xml",
    interfaces: &[AccountsSheetServed::<Unserved>::introspect],
};
/// `org.quire.Spaces1`, on accountd's bus name.
pub const SPACES: Document = Document {
    file_name: "org.quire.Spaces1.xml",
    interfaces: &[SpacesServed::<Unserved>::introspect],
};
/// `org.quire.Identities1`, on accountd's bus name.
pub const IDENTITIES: Document = Document {
    file_name: "org.quire.Identities1.xml",
    interfaces: &[IdentitiesServed::<Unserved>::introspect],
};
/// `org.quire.Tailnet1`, on accountd's bus name.
pub const TAILNET: Document = Document {
    file_name: "org.quire.Tailnet1.xml",
    interfaces: &[TailnetServed::<Unserved>::introspect],
};
