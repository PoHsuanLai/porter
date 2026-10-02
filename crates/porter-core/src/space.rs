//! Spaces as consent and labels see them: a stable id, and a scope that is one Space or any.

use crate::error::CoreError;
use crate::id::slug_id;
use serde::{Deserialize, Serialize};
use std::fmt;

slug_id!(
    /// A Space's stable id. Minted by sill's Space store when the Space is made (not the COSMIC
    /// workspace id, which may not survive a session); the slug grammar of every porter id.
    /// `desktop` is reserved for what lives outside any Space.
    SpaceId,
    "space id"
);

impl SpaceId {
    /// The reserved id of "outside any Space".
    pub const DESKTOP: &'static str = "desktop";

    /// The reserved `desktop` Space.
    pub fn desktop() -> Self {
        Self(Self::DESKTOP.to_owned())
    }
}

/// Which Spaces a decision covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum SpaceScope {
    /// Every Space (and outside them).
    Any,
    /// This Space only.
    Only(SpaceId),
}

impl fmt::Display for SpaceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpaceScope::Any => f.write_str("any"),
            SpaceScope::Only(space) => write!(f, "only {space}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_desktop_space_is_a_valid_id() {
        assert_eq!(SpaceId::parse("desktop"), Ok(SpaceId::desktop()));
        assert!(SpaceId::parse("Work Space").is_err());
    }

    #[test]
    fn scopes_pin_their_json() {
        let cases = [
            (SpaceScope::Any, r#"{"kind":"any"}"#),
            (
                SpaceScope::Only(SpaceId::parse("work").expect("id")),
                r#"{"kind":"only","v":"work"}"#,
            ),
        ];
        for (scope, json) in cases {
            assert_eq!(serde_json::to_string(&scope).expect("json"), json);
            assert_eq!(
                serde_json::from_str::<SpaceScope>(json).expect("scope"),
                scope
            );
        }
    }
}
