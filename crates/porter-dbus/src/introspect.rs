//! The introspection XML of each bus, from the interfaces' own declarations: the checked-in
//! `dbus/*.xml` must equal it.

#[cfg(feature = "photos-picker")]
use crate::PhotosPickerServed;
use crate::{
    AccountServed, AccountsSheetServed, AgentsServed, IdentitiesServed, InferenceServed,
    KeysServed, LauncherServed, ManagerServed, RequestServed, SpacesServed, StorageServed,
    SyncServed, TailnetServed, Unserved,
};

/// One of porter's buses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Bus {
    /// `org.quire.Accounts1` (accountd): adding an identity and the identities' accounts, and
    /// the Request object every sheet method returns.
    Accounts,
    /// `org.quire.AccountsSheet1` (the sheet host).
    AccountsSheet,
    /// `org.quire.Sync1` (syncd), and `org.quire.Photos1.Picker` on the same name and object when
    /// the `photos-picker` feature is on (it is not part of the default surface).
    Sync,
    /// `org.quire.Inference1` (inferd).
    Inference,
    /// `org.quire.Inference1.Agents` (inferd's agent endpoints), on inferd's bus name.
    InferenceAgents,
    /// `org.quire.Inference1.Launcher` (the agent launcher's calls: sessions, agent sign-in,
    /// an agent's key and its process credentials), on inferd's bus name.
    InferenceLauncher,
    /// `org.quire.Inference1.Keys` (an app asking for a permit to use the person's AI keys, and
    /// adding a key), on inferd's bus name.
    InferenceKeys,
    /// `org.quire.Spaces1` (the desktop-wide Spaces), on accountd's bus name.
    Spaces,
    /// `org.quire.Identities1` (the identities signed in on this machine), on accountd's bus
    /// name.
    Identities,
    /// `org.quire.Tailnet1` (the person's computers on their Tailscale network), on accountd's
    /// bus name.
    Tailnet,
    /// `org.quire.Storage1` (each storage account's drive as a folder, for Files), on syncd's bus
    /// name.
    Storage,
}

impl Bus {
    /// The file under `dbus/` holding its introspection.
    pub fn file_name(self) -> &'static str {
        match self {
            Bus::Accounts => "org.quire.Accounts1.xml",
            Bus::AccountsSheet => "org.quire.AccountsSheet1.xml",
            Bus::Sync => "org.quire.Sync1.xml",
            Bus::Inference => "org.quire.Inference1.xml",
            Bus::InferenceAgents => "org.quire.Inference1.Agents.xml",
            Bus::InferenceLauncher => "org.quire.Inference1.Launcher.xml",
            Bus::InferenceKeys => "org.quire.Inference1.Keys.xml",
            Bus::Spaces => "org.quire.Spaces1.xml",
            Bus::Identities => "org.quire.Identities1.xml",
            Bus::Tailnet => "org.quire.Tailnet1.xml",
            Bus::Storage => "org.quire.Storage1.xml",
        }
    }
}

/// The introspection document of `bus`: every interface it serves, in one `<node>`. Each
/// interface writes its own element; this only puts them in a node, in the order the bus serves
/// them.
pub fn introspection(bus: Bus) -> String {
    let mut xml = String::from(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\"\n \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
    );
    let interfaces: &[fn(&mut String)] = match bus {
        Bus::Accounts => &[
            ManagerServed::<Unserved>::introspect,
            AccountServed::<Unserved>::introspect,
            RequestServed::<Unserved>::introspect,
        ],
        Bus::AccountsSheet => &[AccountsSheetServed::<Unserved>::introspect],
        Bus::Sync => SYNC,
        Bus::Inference => &[InferenceServed::<Unserved>::introspect],
        Bus::InferenceAgents => &[AgentsServed::<Unserved>::introspect],
        Bus::InferenceLauncher => &[LauncherServed::<Unserved>::introspect],
        Bus::InferenceKeys => &[KeysServed::<Unserved>::introspect],
        Bus::Spaces => &[SpacesServed::<Unserved>::introspect],
        Bus::Identities => &[IdentitiesServed::<Unserved>::introspect],
        Bus::Tailnet => &[TailnetServed::<Unserved>::introspect],
        Bus::Storage => &[StorageServed::<Unserved>::introspect],
    };
    for introspect in interfaces {
        introspect(&mut xml);
    }
    xml.push_str("</node>\n");
    xml
}

/// Sync1, and the Photos Picker beside it only with the `photos-picker` feature.
#[cfg(feature = "photos-picker")]
const SYNC: &[fn(&mut String)] = &[
    SyncServed::<Unserved>::introspect,
    PhotosPickerServed::<Unserved>::introspect,
];

#[cfg(not(feature = "photos-picker"))]
const SYNC: &[fn(&mut String)] = &[SyncServed::<Unserved>::introspect];
