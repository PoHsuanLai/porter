//! Identifiers: opaque, stable, ASCII slugs, parsed once where they enter.
//!
//! One grammar for every id (`[a-z0-9]` then `[a-z0-9._-]`, at most 64 bytes) so an id is
//! always safe in a file name, a Secret Service attribute and, after [`crate::object_segment`],
//! a D-Bus object path. A UUID in its hyphenated lowercase form is a valid id.

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The longest id, in bytes.
const MAX_LEN: usize = 64;

/// Whether `text` is a well-formed id.
fn well_formed(text: &str) -> bool {
    let mut bytes = text.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    first_ok
        && text.len() <= MAX_LEN
        && bytes.all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

macro_rules! slug_id {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The id written as `text`, or why it is not one.
            pub fn parse(text: &str) -> Result<Self, CoreError> {
                if well_formed(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(CoreError::MalformedId { what: $what, text: text.to_owned() })
                }
            }

            /// The id's text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = CoreError;
            fn try_from(text: String) -> Result<Self, CoreError> {
                Self::parse(&text)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

slug_id!(
    /// One configured account (a row in accountd's registry, or a discovered local runtime).
    AccountId,
    "account id"
);
slug_id!(
    /// One provider declaration (`providers/<id>.toml`).
    ProviderId,
    "provider id"
);
slug_id!(
    /// One consent decision, named so an app can revoke it and Settings can list it.
    GrantId,
    "grant id"
);
slug_id!(
    /// One model an AI account serves (`llama3.2`, `claude-sonnet-4-5`).
    ModelId,
    "model id"
);

/// The id as one D-Bus object path segment (`[A-Za-z0-9_]`): `.` and `-` become `_`.
///
/// Two ids differing only in those characters share a segment; accountd refuses the second
/// (it never mints such ids itself).
pub fn object_segment(id: &AccountId) -> String {
    id.as_str()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &[(&str, &str, bool)] = &[
        ("plain", "nextcloud", true),
        ("uuid", "67e55044-10b1-426f-9247-bb680e5fe0c8", true),
        ("dots", "llama3.2", true),
        ("empty", "", false),
        ("upper", "Nextcloud", false),
        ("leading dash", "-x", false),
        ("slash", "a/b", false),
        ("space", "a b", false),
        (
            "too long",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            false,
        ),
    ];

    #[test]
    fn ids_parse_only_the_slug_grammar() {
        for (name, text, ok) in CASES {
            assert_eq!(AccountId::parse(text).is_ok(), *ok, "{name}");
        }
    }

    #[test]
    fn a_malformed_id_does_not_deserialize() {
        let parsed: Result<ProviderId, _> = serde_json::from_str("\"Not An Id\"");
        assert!(parsed.is_err());
    }

    #[test]
    fn object_segment_keeps_only_path_characters() {
        let id = AccountId::parse("67e55044-10b1.x").expect("valid id");
        assert_eq!(object_segment(&id), "67e55044_10b1_x");
    }
}
