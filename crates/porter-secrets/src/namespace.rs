//! The keyring namespace: the `service` attribute every item is filed under (and the keyring-core
//! service), chosen by the host, so two programs on one keyring never read each other's items.
//!
//! Every constructor that takes no namespace files under `porter` ([`Namespace::PORTER`]). That
//! namespace is kept for apps that migrate their old items out of it; new code names its own.
//! porter's daemons each name theirs (`porter.accountd`, `porter.syncd`), and a
//! host that keeps its own credentials names its own, such as mailo's `mailo/<profile>`
//! (`mailo/default`, `mailo/3fa9c01b`). Any one line of text is a valid name.

use std::borrow::Cow;
use std::fmt;

/// Why a namespace was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NamespaceError {
    /// There was no name.
    #[error("a keyring namespace needs a name")]
    Empty,
    /// The name has a line break or another control character in it.
    #[error("a keyring namespace is one line of plain text")]
    Control,
}

/// Which program's items a store reads and writes: the `service` attribute of every item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Namespace(Cow<'static, str>);

impl Namespace {
    /// `porter`: what every store uses unless its host names another, and `Namespace::default()`.
    /// Kept for apps that migrate their old items out of it; new code names its own namespace.
    pub const PORTER: Self = Self(Cow::Borrowed("porter"));

    /// The namespace `text` names (`mailo/work`, `porter.accountd`).
    pub fn parse(text: &str) -> Result<Self, NamespaceError> {
        match text {
            "" => Err(NamespaceError::Empty),
            _ if text.chars().any(char::is_control) => Err(NamespaceError::Control),
            _ => Ok(Self(Cow::Owned(text.to_owned()))),
        }
    }

    /// The `service` attribute, as the keyring stores it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Namespace {
    fn default() -> Self {
        Self::PORTER
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_namespace_is_one_line_of_text() {
        let cases = [
            ("porter", Ok("porter")),
            ("mailo/work", Ok("mailo/work")),
            ("mailo/default", Ok("mailo/default")),
            ("mailo/3fa9c01b", Ok("mailo/3fa9c01b")),
            ("porter.accountd", Ok("porter.accountd")),
            ("", Err(NamespaceError::Empty)),
            ("two\nlines", Err(NamespaceError::Control)),
        ];
        for (text, want) in cases {
            assert_eq!(
                Namespace::parse(text).as_ref().map(Namespace::as_str),
                want.as_ref().map(|s: &&str| *s),
                "{text:?}"
            );
        }
        assert_eq!(Namespace::default(), Namespace::PORTER);
    }
}
