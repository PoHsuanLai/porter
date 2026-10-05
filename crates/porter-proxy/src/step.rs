//! What every protocol machine takes and gives: bytes from one side in, bytes for a side out.

use crate::fault::RelayFault;
use std::fmt;

/// One end of the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The app, on the stream it was handed.
    App,
    /// The endpoint, on the connection the relay dialled.
    Server,
}

/// What reached the machine. `Debug` shows how many bytes, not which: a stream may carry a
/// credential.
#[derive(Clone, PartialEq, Eq)]
pub enum Input {
    /// The connection to the server is up; begin.
    Start,
    /// Bytes arrived from a side.
    Bytes {
        /// Which side sent them.
        from: Side,
        /// What it sent.
        data: Vec<u8>,
    },
    /// The host upgraded the server connection to TLS, as `StartTls` asked.
    TlsReady,
    /// A side finished writing.
    Closed(Side),
}

/// How the relay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayEnd {
    /// Both sides finished.
    Finished,
    /// It failed.
    Failed(RelayFault),
}

/// What the host must do. `Debug` shows how many bytes, not which: a line toward the server
/// may carry the credential.
#[derive(Clone, PartialEq, Eq)]
pub enum Effect {
    /// Write these bytes to a side.
    Send {
        /// The side to write to.
        to: Side,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Upgrade the server connection to TLS, then feed `Input::TlsReady`.
    StartTls,
    /// Stop: close both sides.
    Close(RelayEnd),
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::Start => f.write_str("Start"),
            Input::Bytes { from, data } => write!(f, "Bytes({from:?}, {} bytes)", data.len()),
            Input::TlsReady => f.write_str("TlsReady"),
            Input::Closed(side) => write!(f, "Closed({side:?})"),
        }
    }
}

impl fmt::Debug for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Send { to, data } => write!(f, "Send({to:?}, {} bytes)", data.len()),
            Effect::StartTls => f.write_str("StartTls"),
            Effect::Close(end) => write!(f, "Close({end:?})"),
        }
    }
}

/// A protocol's relay as a pure machine.
pub trait Relaying: Sized {
    /// The machine after `input`, and what the host must do about it.
    fn step(self, input: Input) -> (Self, Vec<Effect>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_machines_exchange_never_shows_its_bytes_in_debug() {
        let data = b"A001 LOGIN ada hunter2\r\n".to_vec();
        let shown = format!(
            "{:?} {:?}",
            Effect::Send {
                to: Side::Server,
                data: data.clone()
            },
            Input::Bytes {
                from: Side::App,
                data
            }
        );
        assert!(
            !shown.contains("hunter2") && !shown.contains("LOGIN"),
            "{shown}"
        );
        assert!(shown.contains("24 bytes"), "{shown}");
    }
}
