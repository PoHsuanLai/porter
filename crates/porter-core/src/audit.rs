//! What accountd records about its own decisions (design/31 §4.5 "Audit", porter PLAN §2.3):
//! one line per event, never a value. An entry names ids, kinds, audiences and endpoints; there
//! is no field that could hold a secret, so a log of them cannot leak one.

use crate::app_id::AppId;
use crate::capability::CapabilityKind;
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
    /// An app's legacy account was adopted.
    Adopted,
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
}
