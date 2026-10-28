//! The small enums every data kind shares (design/31 §2.1). Their declaration order is their
//! strength order: a need names a minimum and an offer satisfies it when it is at least that.

use serde::{Deserialize, Serialize};

/// How much of the data an account can touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Nothing (the kind is present only for its other fields, such as upload).
    None,
    /// Reading only.
    Read,
    /// Reading and writing.
    ReadWrite,
}

/// How changes become known (R11): a need asks for a minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    /// Only a full listing shows what changed.
    None,
    /// A change token the client polls (Drive page token, Graph delta, sync-token).
    Poll,
    /// The server pushes (IMAP IDLE, JMAP EventSource, long-poll).
    Push,
}

/// A two-word flag of a capability field: a `bool` in memory whose stored and bus text is one of
/// two words, as the field has always been written (`"present"` / `"absent"`, `"reported"` /
/// `"unreported"`). A capability field names the fact (`send`, `ranges`) and carries one of
/// these through `#[serde(with = "..")]`.
macro_rules! word_flag {
    ($(#[$doc:meta])* $module:ident, $yes:literal, $no:literal) => {
        $(#[$doc])*
        pub mod $module {
            use serde::{Deserialize, Deserializer, Serialize, Serializer};

            #[derive(Serialize, Deserialize)]
            enum Word {
                #[serde(rename = $yes)]
                Yes,
                #[serde(rename = $no)]
                No,
            }

            /// Writes the flag as its word.
            pub fn serialize<S: Serializer>(flag: &bool, serializer: S) -> Result<S::Ok, S::Error> {
                if *flag { Word::Yes } else { Word::No }.serialize(serializer)
            }

            /// Reads the flag from its word.
            pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
                Ok(matches!(Word::deserialize(deserializer)?, Word::Yes))
            }
        }
    };
}

word_flag!(
    /// Whether one optional feature is there: `"present"` or `"absent"`.
    present, "present", "absent"
);
word_flag!(
    /// Whether the account reports its storage quota: `"reported"` or `"unreported"`.
    reported, "reported", "unreported"
);

/// When the person is asked for a capability's access: at sign-in with everything else, or only
/// when something first uses it (the whole of a drive, asked when the person turns on "Show in
/// Files"). A provider file's row says `ask = "on_use"` for the second; the first is the
/// default and is not written.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Ask {
    /// With the sign-in.
    #[default]
    SignIn,
    /// When it is first used, in a sign-in of its own that asks for it alone.
    OnUse,
}

impl Ask {
    /// Whether this is the default, asked with the sign-in.
    pub fn is_sign_in(&self) -> bool {
        *self == Ask::SignIn
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Access, Capability, Delta, HashKind, LabelModel, MailCap, MailTransport, StorageCap,
        StorageScope,
    };

    fn storage(quota: bool, ranges: bool) -> Capability {
        Capability::Storage(StorageCap {
            access: Access::Read,
            delta: Delta::None,
            quota,
            scope: StorageScope::Full,
            hashes: HashKind::None,
            ranges,
            chunked_upload: false,
        })
    }

    #[test]
    fn a_flag_is_written_and_read_as_the_word_it_always_was() {
        let mail = Capability::Mail(MailCap {
            access: Access::ReadWrite,
            send: true,
            delta: Delta::Push,
            transport: MailTransport::Imap,
            labels: LabelModel::Folders,
        });
        let text = r#"{"kind":"mail","v":{"access":"read_write","send":"present","delta":"push","transport":"imap","labels":"folders"}}"#;
        assert_eq!(serde_json::to_string(&mail).expect("json"), text);
        assert_eq!(
            serde_json::from_str::<Capability>(text).expect("back"),
            mail
        );

        let text = r#"{"kind":"storage","v":{"access":"read","delta":"none","quota":"reported","scope":"full","hashes":"none","ranges":"absent","chunked_upload":"absent"}}"#;
        assert_eq!(
            serde_json::to_string(&storage(true, false)).expect("json"),
            text
        );
        assert_eq!(
            serde_json::from_str::<Capability>(text).expect("back"),
            storage(true, false)
        );
        let off = text
            .replace("reported", "unreported")
            .replace("\"ranges\":\"absent\"", "\"ranges\":\"present\"");
        assert_eq!(
            serde_json::from_str::<Capability>(&off).expect("back"),
            storage(false, true)
        );
        // Not a word of the flag: refused, as the enum refused it.
        let bad = text.replace("\"reported\"", "\"true\"");
        assert!(serde_json::from_str::<Capability>(&bad).is_err());
    }
}
