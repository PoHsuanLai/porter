//! One line of `GET /localapi/v0/watch-ipn-bus`: tailscaled's `ipn.Notify`, newline-delimited
//! JSON, as of v1.80.0 (`ipn/backend.go`). porter reads it only as a nudge that something
//! changed (the answer to "what is the state now" is `status`), so only the few fields that
//! say what changed are read, and the net map, which is large, is only noticed.

use crate::error::TailscaleError;
use crate::status::Backend;
use serde::Deserialize;

/// What one notice from tailscaled says changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Its new state, when the notice carries one.
    pub state: Option<Backend>,
    /// The page Tailscale wants the person to open, when it does.
    pub browse_to: Option<String>,
    /// Whether the computers on the network changed.
    pub net_map: bool,
    /// Whether a sign-in finished.
    pub login_finished: bool,
}

#[derive(Deserialize)]
struct RawNotice {
    #[serde(rename = "State", default)]
    state: Option<i64>,
    #[serde(rename = "BrowseToURL", default)]
    browse_to: Option<String>,
    #[serde(rename = "NetMap", default)]
    net_map: Option<serde::de::IgnoredAny>,
    #[serde(rename = "LoginFinished", default)]
    login_finished: Option<serde::de::IgnoredAny>,
}

impl Notice {
    /// Reads one line of the stream.
    pub fn parse(line: &[u8]) -> Result<Self, TailscaleError> {
        let raw: RawNotice = crate::object::object(line)?;
        Ok(Self {
            state: raw.state.map(Backend::from_number),
            browse_to: raw.browse_to.filter(|url| !url.is_empty()),
            net_map: raw.net_map.is_some(),
            login_finished: raw.login_finished.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_says_what_changed() {
        let cases = [
            (
                r#"{"Version":"1.80.0","State":6}"#,
                Some(Backend::Running),
                None,
                false,
                false,
            ),
            (
                r#"{"State":2}"#,
                Some(Backend::NeedsLogin),
                None,
                false,
                false,
            ),
            (
                r#"{"BrowseToURL":"https://login.tailscale.com/a/abc"}"#,
                None,
                Some("https://login.tailscale.com/a/abc"),
                false,
                false,
            ),
            (r#"{"NetMap":{"Peers":[1,2,3]}}"#, None, None, true, false),
            (r#"{"LoginFinished":{}}"#, None, None, false, true),
            (
                r#"{"State":null,"NetMap":null,"BrowseToURL":""}"#,
                None,
                None,
                false,
                false,
            ),
            ("{}", None, None, false, false),
        ];
        for (line, state, url, net_map, login_finished) in cases {
            let notice = Notice::parse(line.as_bytes()).expect(line);
            assert_eq!(notice.state, state, "{line}");
            assert_eq!(notice.browse_to.as_deref(), url, "{line}");
            assert_eq!(notice.net_map, net_map, "{line}");
            assert_eq!(notice.login_finished, login_finished, "{line}");
        }
    }

    #[test]
    fn a_line_that_is_not_a_notice_is_malformed() {
        for line in ["", "not json", "[1]", "3"] {
            assert_eq!(
                Notice::parse(line.as_bytes()),
                Err(TailscaleError::Malformed),
                "{line:?}"
            );
        }
    }
}
