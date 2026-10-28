//! The token naming one session of an app that asks for access: the span a grant scoped "This
//! session only" lasts. Whoever hosts the app's sessions chooses it and says when each begins and
//! ends (the agent launcher maps the ACP session id it runs an agent under); consent only
//! compares tokens, so it knows nothing of the host.

use porter_ids::slug_id;

slug_id!(
    /// One session. The same grammar as every id of ours (lowercase letters, digits, `.`, `_`,
    /// `-`; at most 64 bytes), so it is safe in a file name, an audit line and a D-Bus string.
    /// A session id that is not in it (an ACP session id with capitals) is mapped by whoever
    /// hosts the session, for instance to its lowercase hex digest; the service never sees the
    /// original.
    SessionToken,
    "session token"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_is_the_id_grammar() {
        for good in ["sess-1", "0b1c2d3e-4f50", "a.b_c"] {
            assert_eq!(SessionToken::parse(good).expect(good).as_str(), good);
        }
        for bad in ["", "Sess", "a b", "../x", "-x", &"a".repeat(65)] {
            assert!(SessionToken::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn it_reads_and_writes_as_its_text_and_a_malformed_one_does_not_read() {
        let id = SessionToken::parse("sess-7").expect("id");
        assert_eq!(serde_json::to_string(&id).expect("json"), r#""sess-7""#);
        assert_eq!(
            serde_json::from_str::<SessionToken>(r#""sess-7""#).expect("id"),
            id
        );
        assert!(serde_json::from_str::<SessionToken>(r#""Sess 7""#).is_err());
    }
}
