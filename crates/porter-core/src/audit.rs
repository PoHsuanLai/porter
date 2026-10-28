//! The words for a credential handed to a spawned process (P2): how it travelled and why it
//! ended. The bus, the launcher and the audit log share them, so they stay here where a bus client
//! names them without the account service; the audit log itself, `AuditEntry` and `AuditEvent`, is
//! `porter_service_core::audit`.

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

#[cfg(test)]
mod tests {
    use super::*;
    use porter_ids::word::{from_word, word as word_of};

    #[test]
    fn the_bus_words_of_a_handoff_and_an_end_are_the_audit_words_as_written() {
        for (way, word) in [
            (Handoff::Memfd, "memfd"),
            (Handoff::TmpfsFile, "tmpfs_file"),
        ] {
            assert_eq!(word_of(&way), word);
            assert_eq!(from_word::<Handoff>(word), Some(way));
        }
        for (end, word) in [
            (CredentialEnd::ProcessExited, "process_exited"),
            (CredentialEnd::LauncherGone, "launcher_gone"),
            (CredentialEnd::GrantRevoked, "grant_revoked"),
            (CredentialEnd::AccountRemoved, "account_removed"),
            (CredentialEnd::Expired, "expired"),
            (CredentialEnd::SessionClosed, "session_closed"),
        ] {
            assert_eq!(word_of(&end), word);
            assert_eq!(from_word::<CredentialEnd>(word), Some(end));
        }
        assert_eq!(from_word::<Handoff>("env"), None);
        assert_eq!(from_word::<CredentialEnd>("Memfd"), None);
    }
}
