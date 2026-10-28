//! The grammar of an id, and the macro that declares one: opaque, stable, ASCII slugs, parsed
//! once where they enter. The ids themselves are declared, with [`slug_id!`], by the crate that
//! owns the concept they name.
//!
//! One grammar for every id (`[a-z0-9]` then `[a-z0-9._-]`, at most 64 bytes) so an id is
//! always safe in a file name, a Secret Service attribute and, after [`object_segment`],
//! a D-Bus object path. A UUID in its hyphenated lowercase form is a valid id.

/// The longest id, in bytes.
const MAX_LEN: usize = 64;

/// Whether `text` is a well-formed id (the one grammar every id of ours shares; crates that
/// mint ids of their own, such as `prov`, check with this).
pub fn is_id(text: &str) -> bool {
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

/// The text-newtype items every validated string shares, for a tuple struct `Name(String)` (or
/// `Name(Box<str>)`) whose `parse(&str) -> Result<Self, $error>` holds the rule: `as_str`,
/// `TryFrom<String>` (so serde's `try_from = "String"` runs `parse`), `From<Name> for String`
/// and `Display`. The one place those four are written.
#[macro_export]
macro_rules! string_newtype {
    ($name:ident, $error:ty) => {
        impl $name {
            /// The text as written.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $error;
            fn try_from(text: String) -> Result<Self, $error> {
                Self::parse(&text)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0.into()
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

/// Declares an id type in the one slug grammar ([`is_id`]): a `String` newtype whose `parse` is
/// the only way in, whose serde form is its text, and whose `Display` writes the text. The
/// field is private to the module that declares it, and the caller depends on `serde`.
#[macro_export]
macro_rules! slug_id {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The id written as `text`, or why it is not one.
            pub fn parse(text: &str) -> Result<Self, $crate::IdError> {
                if $crate::is_id(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err($crate::IdError::malformed($what, text))
                }
            }
        }

        $crate::string_newtype!($name, $crate::IdError);
    };
}

/// The id as one D-Bus object path segment (`[A-Za-z0-9_]`): `.` and `-` become `_`.
///
/// Two ids differing only in those characters share a segment; accountd refuses the second
/// (it never mints such ids itself).
pub fn object_segment(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    slug_id!(
        /// An id of the tests.
        TestId,
        "test id"
    );

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
            assert_eq!(TestId::parse(text).is_ok(), *ok, "{name}");
        }
    }

    #[test]
    fn a_malformed_id_does_not_deserialize() {
        let parsed: Result<TestId, _> = serde_json::from_str("\"Not An Id\"");
        assert!(parsed.is_err());
    }

    #[test]
    fn object_segment_keeps_only_path_characters() {
        assert_eq!(object_segment("67e55044-10b1.x"), "67e55044_10b1_x");
    }
}
