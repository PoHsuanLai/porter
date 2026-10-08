//! Who is asking: the identity a grant binds to (design/31 §4.5, R12).

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A reverse-DNS application name (`org.quire.Photos`), as Flatpak and desktop files write it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppName(String);

impl AppName {
    /// The name written as `text`: two or more dot-separated elements of `[A-Za-z0-9_-]`, each
    /// starting with a letter or `_`, at most 255 bytes (the D-Bus and Flatpak rule).
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let element_ok = |element: &str| {
            let mut chars = element.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        let elements: Vec<&str> = text.split('.').collect();
        if text.len() <= 255 && elements.len() >= 2 && elements.iter().all(|e| element_ok(e)) {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "app name",
                text: text.to_owned(),
            })
        }
    }

    /// The name's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AppName {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<AppName> for String {
    fn from(name: AppName) -> String {
        name.0
    }
}

impl fmt::Display for AppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a person reads for an app ("Claude Code"), as accountd resolves it (its caller table, the
/// app's desktop entry, the agent's provider file): free text, never the bus id. A sheet that
/// carries an [`AppId`] carries this beside it when accountd has one, so the host does not
/// guess a name from the id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppLabel(pub String);

/// How far the daemon can trust the name, which Settings shows beside every grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// A Flatpak sandbox: the name comes from the sandbox's own metadata for the caller's pid.
    Flatpak,
    /// A native process, proven by a systemd scope our launcher made (`app-<name>-*.scope`, read
    /// by `identity_of`). Same-UID processes can impersonate each other, so consent here is
    /// advisory and the UI says "unsandboxed".
    Unsandboxed,
    /// The app hosts porter's core itself (the in-process link); it is the only caller.
    InProcess,
}

/// The caller a grant binds to. Never sent by the caller: each transport derives it from the
/// connection (D-Bus credentials, the socket's peer, the embedding app's configuration).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AppId {
    /// The application's name.
    pub name: AppName,
    /// How the name was established.
    pub isolation: Isolation,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &[(&str, &str, bool)] = &[
        ("flatpak style", "org.quire.Photos", true),
        ("dash and underscore", "com.example.my_app-2", true),
        ("one element", "photos", false),
        ("empty element", "org..Photos", false),
        ("digit first", "org.1photos", false),
        ("slash", "org/quire", false),
    ];

    #[test]
    fn app_names_follow_the_reverse_dns_rule() {
        for (name, text, ok) in CASES {
            assert_eq!(AppName::parse(text).is_ok(), *ok, "{name}");
        }
    }
}
