//! The introspection XML of each bus, from the skeletons: the checked-in `dbus/*.xml` must
//! equal it.

use crate::account::AccountSkeleton;
use crate::grants::GrantsSkeleton;
use crate::inference::InferenceSkeleton;
use crate::manager::ManagerSkeleton;
use crate::request::RequestSkeleton;
use crate::sync::SyncSkeleton;
use crate::tokens::TokensSkeleton;
use zbus::fdo;
use zbus::object_server::Interface;

/// One of porter's buses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    /// `org.quire.Accounts1` (accountd).
    Accounts,
    /// `org.quire.Sync1` (syncd).
    Sync,
    /// `org.quire.Inference1` (inferd).
    Inference,
}

impl Bus {
    /// The file under `dbus/` holding its introspection.
    pub fn file_name(self) -> &'static str {
        match self {
            Bus::Accounts => "org.quire.Accounts1.xml",
            Bus::Sync => "org.quire.Sync1.xml",
            Bus::Inference => "org.quire.Inference1.xml",
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
        ],
        Bus::Sync => vec![&SyncSkeleton],
        Bus::Inference => vec![&InferenceSkeleton],
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

/// The answer of every skeleton method: the interface is frozen, its behaviour not built.
pub(crate) fn frozen() -> fdo::Error {
    fdo::Error::NotSupported("porter: frozen interface, not implemented".into())
}
