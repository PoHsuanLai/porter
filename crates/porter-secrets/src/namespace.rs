//! The keyring namespace: the `service` attribute every item is filed under (and the keyring-core
//! service), chosen by the host, so two programs on one keyring never read each other's items.
//!
//! porter's own daemons have used `porter` from the start, and every constructor that takes no
//! namespace still files under it ([`Namespace::PORTER`]). A host that keeps its own credentials
//! names its own, such as mailo's `mailo/<profile>`.

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
    /// `porter`: the namespace porter's daemons have always used, and what every store uses
    /// unless its host names another.
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
