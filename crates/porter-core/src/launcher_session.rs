//! The id of one launcher session (agent-session ask S4, lane session-scope): the span a grant
//! scoped "This session only" lasts. The agent launcher chooses it (the ACP session id it runs
//! the agent under, mapped to this grammar) and tells accountd when it begins and ends
//! (`Peer.BeginSession`, `Peer.EndSession`).

use crate::error::CoreError;
use crate::id::slug_id;
use serde::{Deserialize, Serialize};
use std::fmt;

slug_id!(
    /// One launcher session. The same grammar as every id of ours (lowercase letters, digits,
    /// `.`, `_`, `-`; at most 64 bytes), so it is safe in a file name, an audit line and a
    /// D-Bus string. A session id that is not in it (an ACP session id with capitals) is mapped
    /// by the launcher, for instance to its lowercase hex digest; accountd never sees the
    /// original.
    LauncherSession,
    "launcher session id"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_is_the_id_grammar() {
        for good in ["sess-1", "0b1c2d3e-4f50", "a.b_c"] {
            assert_eq!(LauncherSession::parse(good).expect(good).as_str(), good);
        }
        for bad in ["", "Sess", "a b", "../x", "-x", &"a".repeat(65)] {
            assert!(LauncherSession::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn it_reads_and_writes_as_its_text_and_a_malformed_one_does_not_read() {
        let id = LauncherSession::parse("sess-7").expect("id");
        assert_eq!(serde_json::to_string(&id).expect("json"), r#""sess-7""#);
        assert_eq!(
            serde_json::from_str::<LauncherSession>(r#""sess-7""#).expect("id"),
            id
        );
        assert!(serde_json::from_str::<LauncherSession>(r#""Sess 7""#).is_err());
    }
}
