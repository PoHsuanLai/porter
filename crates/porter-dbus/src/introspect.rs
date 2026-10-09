//! The introspection XML of each bus, from the skeletons: the checked-in `dbus/*.xml` must
//! equal it.

use crate::account::AccountSkeleton;
use crate::agents::AgentsSkeleton;
use crate::grants::GrantsSkeleton;
use crate::inference::InferenceSkeleton;
use crate::manager::ManagerSkeleton;
use crate::peer::PeerSkeleton;
#[cfg(feature = "photos-picker")]
use crate::photos_picker::PickerSkeleton;
use crate::request::RequestSkeleton;
use crate::sheet_backend::AccountsSheetSkeleton;
use crate::spaces::SpacesSkeleton;
use crate::sync::SyncSkeleton;
use crate::tailnet::TailnetSkeleton;
use crate::tokens::TokensSkeleton;
use zbus::fdo;
use zbus::object_server::Interface;

/// One of porter's buses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    /// `org.quire.Accounts1` (accountd).
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
    /// `org.quire.Spaces1` (the desktop-wide Spaces), on accountd's bus name.
    Spaces,
    /// `org.quire.Tailnet1` (the person's computers on their Tailscale network), on accountd's
    /// bus name.
    Tailnet,
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
            Bus::Spaces => "org.quire.Spaces1.xml",
            Bus::Tailnet => "org.quire.Tailnet1.xml",
        }
    }
}

/// The introspection document of `bus`: every interface it serves, in one `<node>`.
pub fn introspection(bus: Bus) -> String {
    let interfaces: Vec<&dyn Interface> = match bus {
        Bus::Accounts => vec![
            &ManagerSkeleton,
            &AccountSkeleton,
            &GrantsSkeleton,
            &TokensSkeleton,
            &RequestSkeleton,
            &PeerSkeleton,
        ],
        Bus::AccountsSheet => vec![&AccountsSheetSkeleton],
        Bus::Sync => sync_interfaces(),
        Bus::Inference => vec![&InferenceSkeleton],
        Bus::InferenceAgents => vec![&AgentsSkeleton],
        Bus::Spaces => vec![&SpacesSkeleton],
        Bus::Tailnet => vec![&TailnetSkeleton],
    };
    let mut xml = String::from(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\"\n \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
    );
    for interface in interfaces {
        interface.introspect_to_writer(&mut xml, 1);
    }
    xml.push_str("</node>\n");
    xml
}

/// Sync1, and the Photos Picker beside it only with the `photos-picker` feature.
#[cfg(feature = "photos-picker")]
fn sync_interfaces() -> Vec<&'static dyn Interface> {
    vec![&SyncSkeleton, &PickerSkeleton]
}

#[cfg(not(feature = "photos-picker"))]
fn sync_interfaces() -> Vec<&'static dyn Interface> {
    vec![&SyncSkeleton]
}

/// The answer of every skeleton method: the interface is frozen, its behaviour not built.
pub(crate) fn frozen() -> fdo::Error {
    fdo::Error::NotSupported("porter: frozen interface, not implemented".into())
}
