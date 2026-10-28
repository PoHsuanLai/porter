//! What a daemon needs to serve an interface that `#[bus_interface]` declared: the call it answers
//! and the type that stands in for it when only the interface's shape is wanted.

use zbus::Connection;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;

/// The call a server method answers: who sent it and where, and the connection to answer over.
/// zbus gives these to a method by attribute; the interface macro gathers them so that a daemon's
/// method names the ones it uses (`Call { header, .. }: Call<'_>`).
#[derive(Debug)]
pub struct Call<'a> {
    /// The message's header: the sender, the object path, the member.
    pub header: Header<'a>,
    /// The emitter of the signals of the object the call is for.
    pub emitter: SignalEmitter<'a>,
    /// The connection the call came in on.
    pub connection: &'a Connection,
}

/// The type no object is ever registered over. Each interface's server trait is implemented for
/// it by `#[bus_interface]` so that the interface's introspection can be written without a
/// daemon; its methods never answer.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unserved;

/// The introspection document of one object: the interfaces it serves, in one `<node>`. Each
/// interface writes its own element (`<Name>Served::<Unserved>::introspect`); a document is only
/// the list of them, in the order the object serves them, and the file under `dbus/` that holds
/// the result. A group lists its own documents (`accounts::ACCOUNTS`), so nothing here knows the
/// interfaces.
#[derive(Debug, Clone, Copy)]
pub struct Document {
    /// The file under `dbus/` that holds it.
    pub file_name: &'static str,
    /// The interfaces' own writers.
    pub interfaces: &'static [fn(&mut String)],
}

impl Document {
    /// The document as the daemon's object introspects.
    pub fn introspection(&self) -> String {
        let mut xml = String::from(
            "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\"\n \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
        );
        for introspect in self.interfaces {
            introspect(&mut xml);
        }
        xml.push_str("</node>\n");
        xml
    }
}
