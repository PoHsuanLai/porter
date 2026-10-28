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
