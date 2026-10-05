//! Line framing shared by the line-based machines (IMAP, SMTP, ManageSieve): what a machine
//! holds until a whole line has arrived, and the effects every machine builds the same way.

use crate::fault::RelayFault;
use crate::step::{Effect, RelayEnd, Side};

/// The longest line a machine buffers before it gives up on the peer. Generous: an IMAP
/// capability line or an SMTP extension list is a few hundred bytes.
pub(crate) const MAX_LINE: usize = 64 * 1024;

/// The most bytes an app may send before the relay is ready for it (a client that does not wait
/// for its greeting).
pub(crate) const MAX_EARLY: usize = 64 * 1024;

/// Removes and returns the first line of `buffer` without its terminator (`CRLF`, or a bare
/// `LF` as some servers write), or `None` while no whole line has arrived.
pub(crate) fn take_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = buffer.iter().position(|b| *b == b'\n')?;
    let mut line: Vec<u8> = buffer.drain(..=end).collect();
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(line)
}

/// Whether `buffer` holds more than a line may.
pub(crate) fn overlong(buffer: &[u8]) -> bool {
    buffer.len() > MAX_LINE
}

/// A line as text; bytes that are not UTF-8 become replacement characters (the machines only
/// compare ASCII keywords).
pub(crate) fn text(line: &[u8]) -> String {
    String::from_utf8_lossy(line).into_owned()
}

/// `line` and its `CRLF`, for a side.
pub(crate) fn send_line(to: Side, line: &str) -> Effect {
    Effect::Send {
        to,
        data: format!("{line}\r\n").into_bytes(),
    }
}

/// Raw bytes for a side.
pub(crate) fn send(to: Side, data: &[u8]) -> Effect {
    Effect::Send {
        to,
        data: data.to_vec(),
    }
}

/// Stop with this fault.
pub(crate) fn fail(fault: RelayFault) -> Vec<Effect> {
    vec![Effect::Close(RelayEnd::Failed(fault))]
}

/// Stop, both sides having finished.
pub(crate) fn finished() -> Vec<Effect> {
    vec![Effect::Close(RelayEnd::Finished)]
}

/// Whether `effects` ends the relay.
pub(crate) fn closes(effects: &[Effect]) -> bool {
    effects.iter().any(|e| matches!(e, Effect::Close(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_come_out_whole_and_in_order() {
        let mut buffer = b"a\r\nb\nc".to_vec();
        assert_eq!(take_line(&mut buffer), Some(b"a".to_vec()));
        assert_eq!(take_line(&mut buffer), Some(b"b".to_vec()));
        assert_eq!(take_line(&mut buffer), None);
        assert_eq!(buffer, b"c");
        buffer.extend_from_slice(b"\r\n");
        assert_eq!(take_line(&mut buffer), Some(b"c".to_vec()));
        assert!(buffer.is_empty());
    }
}
