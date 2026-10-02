//! The ids agent-side crates share. Slug ids use porter's one id grammar; entity names carry
//! their own.

use porter_core::{AppName, CoreError, is_id};
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! text_id {
    ($(#[$doc:meta])* $name:ident, $what:literal, $ok:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The id written as `text`, or why it is not one.
            pub fn parse(text: &str) -> Result<Self, CoreError> {
                let ok: fn(&str) -> bool = $ok;
                if ok(text) {
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

/// Two or more dot-separated elements, each `[a-z][a-z0-9_]*`.
fn dotted(text: &str) -> bool {
    let element = |e: &str| {
        let mut bytes = e.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    let mut count = 0;
    let all = text.split('.').all(|e| {
        count += 1;
        element(e)
    });
    all && count >= 2
}

/// 1 to 128 bytes, no control characters: an id prov does not interpret.
fn opaque(text: &str) -> bool {
    !text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control)
}

text_id!(
    /// One companion session. A slug minted by the companion service.
    SessionId,
    "session id",
    is_id
);
text_id!(
    /// One computer-use run. A slug minted by cuad (`r-<n>`).
    RunId,
    "run id",
    is_id
);
text_id!(
    /// An external MCP client's name, as it introduced itself: 1 to 64 bytes, no control
    /// characters.
    ClientName,
    "client name",
    |t| !t.is_empty() && t.len() <= 64 && !t.chars().any(char::is_control)
);
text_id!(
    /// What kind of thing an app owns: two or more dotted `[a-z][a-z0-9_]*` elements
    /// (`mail.thread`).
    EntityKind,
    "entity kind",
    dotted
);
text_id!(
    /// An app-local key for one entity: opaque, 1 to 512 bytes, no control characters.
    EntityKey,
    "entity key",
    |t| !t.is_empty() && t.len() <= 512 && !t.chars().any(char::is_control)
);
text_id!(
    /// A typed action's name (`mail.thread.archive`), the same grammar as [`EntityKind`].
    ActionName,
    "action name",
    dotted
);

text_id!(
    /// One task: a docket `Session` for one prompt, worker or side conversation. A slug minted
    /// by the companion service. It moved here from docket-core so [`AgentRole::Worker`] and
    /// [`AgentRef::Worker`] can name it.
    ///
    /// [`AgentRole::Worker`]: crate::AgentRole::Worker
    /// [`AgentRef::Worker`]: crate::AgentRef::Worker
    TaskId,
    "task id",
    is_id
);
text_id!(
    /// One [`Message`](crate::Message). A slug minted by the router that stamps it.
    MessageId,
    "message id",
    is_id
);
text_id!(
    /// A conversation: the messages that answer one another share it. A slug; the first
    /// message's id is a fine choice.
    ThreadId,
    "thread id",
    is_id
);
text_id!(
    /// A reference to one action outcome, opaque to prov (docket owns the outcome and its
    /// handle): 1 to 128 bytes, no control characters.
    OutcomeRef,
    "outcome ref",
    opaque
);
text_id!(
    /// A reference to one undo journal entry, opaque to prov (docket owns `UndoId`): 1 to 128
    /// bytes, no control characters.
    UndoHandle,
    "undo handle",
    opaque
);

/// One thing an app owns, as the router, memory and the confirmation sheet name it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId {
    /// The app that owns it.
    pub app: AppName,
    /// What kind of thing it is.
    pub kind: EntityKind,
    /// Which one.
    pub key: EntityKey,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_their_grammars() {
        let long_key = "k".repeat(513);
        let cases: Vec<(&str, bool, bool)> = vec![
            ("mail.thread", true, true),
            ("mail.thread.archive", true, true),
            ("mail", false, false),
            ("Mail.thread", false, false),
            ("mail..thread", false, false),
            ("mail.1thread", false, false),
            ("mail.thread_x9", true, true),
            ("", false, false),
            (long_key.as_str(), false, false),
        ];
        for (text, kind_ok, action_ok) in cases {
            assert_eq!(EntityKind::parse(text).is_ok(), kind_ok, "kind {text:?}");
            assert_eq!(
                ActionName::parse(text).is_ok(),
                action_ok,
                "action {text:?}"
            );
        }
        assert!(EntityKey::parse("a/b c").is_ok());
        assert!(EntityKey::parse("").is_err());
        assert!(EntityKey::parse(&"k".repeat(513)).is_err());
        assert!(EntityKey::parse("a\nb").is_err());
        assert!(RunId::parse("r-17").is_ok());
        assert!(RunId::parse("R 17").is_err());
        assert!(ClientName::parse("Claude Desktop").is_ok());
        assert!(ClientName::parse("").is_err());
        assert!(TaskId::parse("t-3").is_ok() && TaskId::parse("T 3").is_err());
        assert!(MessageId::parse("m-1").is_ok() && MessageId::parse("").is_err());
        assert!(ThreadId::parse("m-1").is_ok() && ThreadId::parse("a b").is_err());
        assert!(OutcomeRef::parse("out:41/a").is_ok());
        assert!(OutcomeRef::parse("").is_err());
        assert!(OutcomeRef::parse(&"o".repeat(129)).is_err());
        assert!(UndoHandle::parse("u\n1").is_err());
    }
}
