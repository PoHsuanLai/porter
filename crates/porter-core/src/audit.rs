//! What accountd records about its own decisions (design/31 §4.5 "Audit", porter PLAN §2.3):
//! one line per event, never a value. An entry names ids, kinds, audiences and endpoints; there
//! is no field that could hold a secret, so a log of them cannot leak one.

use crate::account::AgentState;
use crate::agent_login::LoginRequestId;
use crate::app_id::AppId;
use crate::capability::{AgentProgram, CapabilityKind};
use crate::endpoint::EndpointUrl;
use crate::id::{AccountId, GrantId};
use crate::token::Audience;
use crate::units::UnixSeconds;
use serde::{Deserialize, Serialize};

/// One audited event, as a line of `audit.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// When it happened.
    pub at: UnixSeconds,
    /// Who caused it; absent for what accountd does by itself (a renewal, a removal from
    /// Settings, which names no app).
    pub app: Option<AppId>,
    /// The account it concerns, if any.
    pub account: Option<AccountId>,
    /// What happened.
    pub event: AuditEvent,
}

impl AuditEntry {
    /// The entries of an `audit.jsonl` text. The file has no version of its own: a variant is
    /// added when it is needed and an old line keeps reading. A line this build cannot read (a
    /// kind a newer daemon wrote, or a damaged line) is skipped, so the lines around it are kept;
    /// nothing is rewritten, the file stays as it was.
    pub fn read_all(text: &str) -> Vec<AuditEntry> {
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum AuditEvent {
    /// A grant was given.
    Granted {
        /// The grant.
        grant: GrantId,
        /// What it covers.
        kind: CapabilityKind,
    },
    /// "Don't Allow" was stored.
    Denied {
        /// What was refused.
        kind: CapabilityKind,
    },
    /// A grant was withdrawn.
    Revoked {
        /// The grant.
        grant: GrantId,
    },
    /// A short-lived token was issued.
    TokenIssued {
        /// The grant it was issued under.
        grant: GrantId,
        /// The service it is for.
        audience: Audience,
    },
    /// An authenticated relay was opened.
    ProxyOpened {
        /// The grant it was opened under.
        grant: GrantId,
        /// The only server it talks to.
        endpoint: EndpointUrl,
    },
    /// An account was added.
    SignedIn,
    /// An account was signed in again.
    Reauthed,
    /// An account and everything filed for it was removed.
    Removed,
    /// An app's legacy account was adopted. No longer written (the `Adopt` request is gone); kept
    /// so an `audit.jsonl` written by an earlier daemon still reads.
    Adopted,
    /// `Peer.ResolveKey` released an account's API key (on a sealed memfd, to a porter daemon).
    /// The entry's `app` is the app the grant is for, which for an agent route is
    /// `org.quire.Agent.<program>`; `audience` repeats its name as the service the key was
    /// resolved for. Never the key.
    KeyResolved {
        /// The grant the key was released under.
        grant: GrantId,
        /// Who the key was resolved for.
        audience: Audience,
    },
    /// An agent account's state was set: by the launcher (`Peer.SetAgentState`), by the outcome of
    /// a login or logout request it carried out, or by Settings' sign out. Written only when the
    /// state changed.
    AgentStateSet {
        /// The state it is now.
        state: AgentState,
    },
    /// accountd sent the launcher of `program` a request to sign the agent in. Nothing of the
    /// login is ever recorded, only that it was asked.
    AgentLoginAsked {
        /// The request's id (`login-<n>`).
        request: LoginRequestId,
        /// The agent program asked to sign in.
        program: AgentProgram,
    },
    /// accountd sent the launcher of `program` a request to sign the agent out.
    AgentLogoutAsked {
        /// The request's id (`login-<n>`).
        request: LoginRequestId,
        /// The agent program asked to sign out.
        program: AgentProgram,
    },
    /// `Tokens.IssueProcessCredential` (P2, lane p2-handoff): an API key was handed to a process
    /// the launcher spawns for `audience` (`org.quire.Agent.<program>`), by `handoff`. The entry's
    /// `app` is the app the grant is for and `account` the account. Never the credential.
    ProcessCredentialIssued {
        /// Who it was handed to.
        audience: Audience,
        /// How it travelled.
        handoff: Handoff,
    },
    /// A credential handed to a process is no longer valid (`reason` says why).
    ProcessCredentialRevoked {
        /// Who held it.
        audience: Audience,
        /// Why it ended.
        reason: CredentialEnd,
    },
}

/// How a process credential travels (P2). A closed set: a path or fd number is not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Handoff {
    /// A sealed memfd passed to the child.
    Memfd,
    /// A file on a tmpfs, mode 0600, removed with the credential.
    TmpfsFile,
}

/// Why a process credential ended (P2). A closed set of words, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub const ALL: [CredentialEnd; 5] = [
        CredentialEnd::ProcessExited,
        CredentialEnd::LauncherGone,
        CredentialEnd::GrantRevoked,
        CredentialEnd::AccountRemoved,
        CredentialEnd::Expired,
    ];

    /// The word on the bus (`ProcessCredentialRevoked`'s `reason`) and in `audit.jsonl`.
    pub fn word(self) -> &'static str {
        match self {
            CredentialEnd::ProcessExited => "process_exited",
            CredentialEnd::LauncherGone => "launcher_gone",
            CredentialEnd::GrantRevoked => "grant_revoked",
            CredentialEnd::AccountRemoved => "account_removed",
            CredentialEnd::Expired => "expired",
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
    use crate::app_id::{AppName, Isolation};

    #[test]
    fn entries_round_trip_and_pin_their_form() {
        let entry = AuditEntry {
            at: UnixSeconds(1_790_000_000),
            app: Some(AppId {
                name: AppName::parse("org.quire.Mail").expect("name"),
                isolation: Isolation::Flatpak,
            }),
            account: Some(AccountId::parse("cloud").expect("id")),
            event: AuditEvent::TokenIssued {
                grant: GrantId::parse("g1").expect("id"),
                audience: Audience("imap".into()),
            },
        };
        let json = serde_json::to_string(&entry).expect("json");
        assert_eq!(
            json,
            concat!(
                r#"{"at":1790000000,"app":{"name":"org.quire.Mail","isolation":"flatpak"},"#,
                r#""account":"cloud","event":{"kind":"token_issued","v":{"grant":"g1","audience":"imap"}}}"#
            )
        );
        assert_eq!(
            serde_json::from_str::<AuditEntry>(&json).expect("entry"),
            entry
        );
    }

    #[test]
    fn every_event_round_trips() {
        let grant = GrantId::parse("g1").expect("id");
        let events = [
            AuditEvent::Granted {
                grant: grant.clone(),
                kind: CapabilityKind::Mail,
            },
            AuditEvent::Denied {
                kind: CapabilityKind::Storage,
            },
            AuditEvent::Revoked {
                grant: grant.clone(),
            },
            AuditEvent::ProxyOpened {
                grant,
                endpoint: EndpointUrl::parse("imaps://mail.example.org").expect("url"),
            },
            AuditEvent::SignedIn,
            AuditEvent::Reauthed,
            AuditEvent::Removed,
            AuditEvent::Adopted,
            AuditEvent::KeyResolved {
                grant: GrantId::parse("g1").expect("id"),
                audience: Audience("org.quire.Agent.claude-code".into()),
            },
            AuditEvent::AgentStateSet {
                state: AgentState::NeedsLogin,
            },
            AuditEvent::AgentLoginAsked {
                request: LoginRequestId::parse("login-1").expect("id"),
                program: AgentProgram::parse("claude-code").expect("program"),
            },
            AuditEvent::AgentLogoutAsked {
                request: LoginRequestId::parse("login-2").expect("id"),
                program: AgentProgram::parse("claude-code").expect("program"),
            },
            AuditEvent::ProcessCredentialIssued {
                audience: Audience("org.quire.Agent.codex".into()),
                handoff: Handoff::TmpfsFile,
            },
            AuditEvent::ProcessCredentialRevoked {
                audience: Audience("org.quire.Agent.codex".into()),
                reason: CredentialEnd::ProcessExited,
            },
        ];
        for event in events {
            let entry = AuditEntry {
                at: UnixSeconds(1),
                app: None,
                account: None,
                event,
            };
            let json = serde_json::to_string(&entry).expect("json");
            assert_eq!(
                serde_json::from_str::<AuditEntry>(&json).expect("entry"),
                entry
            );
        }
    }

    #[test]
    fn a_line_an_earlier_daemon_wrote_for_an_adoption_still_reads() {
        let line = r#"{"at":1790000000,"app":null,"account":"cloud","event":{"kind":"adopted"}}"#;
        let entry: AuditEntry = serde_json::from_str(line).expect("old line");
        assert_eq!(entry.event, AuditEvent::Adopted);
    }

    #[test]
    fn the_new_events_pin_their_form() {
        let at = |event| {
            serde_json::to_string(&AuditEntry {
                at: UnixSeconds(1),
                app: None,
                account: None,
                event,
            })
            .expect("json")
        };
        assert_eq!(
            at(AuditEvent::AgentStateSet {
                state: AgentState::Ready
            }),
            r#"{"at":1,"app":null,"account":null,"event":{"kind":"agent_state_set","v":{"state":"ready"}}}"#
        );
        assert_eq!(
            at(AuditEvent::ProcessCredentialIssued {
                audience: Audience("a".into()),
                handoff: Handoff::Memfd
            }),
            r#"{"at":1,"app":null,"account":null,"event":{"kind":"process_credential_issued","v":{"audience":"a","handoff":"memfd"}}}"#
        );
        assert_eq!(
            at(AuditEvent::ProcessCredentialRevoked {
                audience: Audience("a".into()),
                reason: CredentialEnd::LauncherGone
            }),
            r#"{"at":1,"app":null,"account":null,"event":{"kind":"process_credential_revoked","v":{"audience":"a","reason":"launcher_gone"}}}"#
        );
    }

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

    #[test]
    fn a_reader_skips_a_line_of_a_kind_from_a_newer_daemon_and_keeps_the_rest() {
        let text = concat!(
            r#"{"at":4,"app":null,"account":"cloud","event":{"kind":"signed_in"}}"#,
            "\n",
            r#"{"at":5,"app":null,"account":"cloud","event":{"kind":"from_the_future","v":{"x":1}}}"#,
            "\n",
            r#"{"at":6,"app":null,"account":"cloud","event":{"kind":"removed"}}"#,
            "\n"
        );
        let entries = AuditEntry::read_all(text);
        assert_eq!(
            entries.iter().map(|e| e.at).collect::<Vec<_>>(),
            [UnixSeconds(4), UnixSeconds(6)]
        );
    }

    #[test]
    fn lines_written_before_the_new_events_still_read() {
        // The format of master cd48cce, one line per kind it wrote.
        let lines = [
            r#"{"at":1,"app":null,"account":"cloud","event":{"kind":"granted","v":{"grant":"g1","kind":"mail"}}}"#,
            r#"{"at":1,"app":null,"account":null,"event":{"kind":"denied","v":{"kind":"storage"}}}"#,
            r#"{"at":1,"app":null,"account":"cloud","event":{"kind":"revoked","v":{"grant":"g1"}}}"#,
            r#"{"at":1,"app":null,"account":"cloud","event":{"kind":"token_issued","v":{"grant":"g1","audience":"resolve_key"}}}"#,
            r#"{"at":1,"app":null,"account":"cloud","event":{"kind":"signed_in"}}"#,
            r#"{"at":1,"app":null,"account":"cloud","event":{"kind":"reauthed"}}"#,
            r#"{"at":1,"app":null,"account":"cloud","event":{"kind":"removed"}}"#,
        ];
        for line in lines {
            let entry: AuditEntry = serde_json::from_str(line).expect(line);
            assert_eq!(AuditEntry::read_all(line), [entry], "{line}");
        }
    }
}
