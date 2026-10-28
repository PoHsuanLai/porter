//! The words for a credential handed to a spawned process (P2): how it travelled and why it
//! ended. The bus, the launcher and the audit log share them, so they stay here where a bus client
//! names them without the account service; the audit log itself, `AuditEntry` and `AuditEvent`, is
//! `porter_service::audit`.

use serde::{Deserialize, Serialize};

/// How a process credential travels (P2). A closed set: a path or fd number is not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Handoff {
    /// A sealed memfd passed to the child.
    Memfd,
    /// A file on a tmpfs, mode 0600, removed with the credential.
    TmpfsFile,
}

/// Why a process credential ended (P2). A closed set of words, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CredentialEnd {
    /// The launcher ended it: the process it was handed to exited or was killed
    /// (`Tokens.RevokeProcessCredential`).
    ProcessExited,
    /// The launcher's connection left the bus, so nobody is left to end the process.
    LauncherGone,
    /// The grant it was issued under was withdrawn.
    GrantRevoked,
    /// The account was removed.
    AccountRemoved,
    /// Its lifetime ran out. Not written yet: a credential lives until one of the ends above.
    Expired,
    /// The launcher session its grant was scoped to closed (`GrantScope::Session`).
    SessionClosed,
}

impl Handoff {
    /// Every way, so a table over them is total.
    pub const ALL: [Handoff; 2] = [Handoff::Memfd, Handoff::TmpfsFile];

    /// The word on the bus (`Tokens.IssueProcessCredential`'s `target`) and in `audit.jsonl`.
    pub fn word(self) -> &'static str {
        match self {
            Handoff::Memfd => "memfd",
            Handoff::TmpfsFile => "tmpfs_file",
        }
    }

    /// The way a word names, if it is one.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|way| way.word() == word)
    }
}

impl CredentialEnd {
    /// Every end, so a table over them is total.
    pub const ALL: [CredentialEnd; 6] = [
        CredentialEnd::ProcessExited,
        CredentialEnd::LauncherGone,
        CredentialEnd::GrantRevoked,
        CredentialEnd::AccountRemoved,
        CredentialEnd::Expired,
        CredentialEnd::SessionClosed,
    ];

    /// The word on the bus (`ProcessCredentialRevoked`'s `reason`) and in `audit.jsonl`.
    pub fn word(self) -> &'static str {
        match self {
            CredentialEnd::ProcessExited => "process_exited",
            CredentialEnd::LauncherGone => "launcher_gone",
            CredentialEnd::GrantRevoked => "grant_revoked",
            CredentialEnd::AccountRemoved => "account_removed",
            CredentialEnd::Expired => "expired",
            CredentialEnd::SessionClosed => "session_closed",
        }
    }

    /// The end a word names, if it is one.
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|end| end.word() == word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bus_words_of_a_handoff_and_an_end_are_their_audit_words() {
        for way in Handoff::ALL {
            let json = serde_json::to_string(&way).expect("json");
            assert_eq!(json, format!("\"{}\"", way.word()));
            assert_eq!(Handoff::from_word(way.word()), Some(way));
        }
        for end in CredentialEnd::ALL {
            let json = serde_json::to_string(&end).expect("json");
            assert_eq!(json, format!("\"{}\"", end.word()));
            assert_eq!(CredentialEnd::from_word(end.word()), Some(end));
        }
        assert_eq!(Handoff::from_word("env"), None);
        assert_eq!(CredentialEnd::from_word("Memfd"), None);
    }
}
