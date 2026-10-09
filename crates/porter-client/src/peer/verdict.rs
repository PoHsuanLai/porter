//! One row of a `Peer.Verdicts` answer, and the API key `Peer.ResolveKey` hands over on a sealed
//! file.

use super::PeerError;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{AccountId, AccountState, GrantId, LauncherSession, SecretText};
use porter_dbus::Details;
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::Read;
use std::os::fd::OwnedFd;

/// What the consent store says for an app on one account that could serve a need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountVerdict {
    /// The account.
    pub account: AccountId,
    /// The provider file the account was made from, when accountd says (`provider` in the
    /// answer's details); otherwise the account id's own stem names it.
    pub provider: Option<String>,
    /// What a person reads for the account (`label` in the answer's details), when accountd says.
    pub label: Option<String>,
    /// What a person reads for its provider (`provider_label`), when accountd knows the file.
    pub provider_label: Option<String>,
    /// Whether it works now (`state`), when accountd says; an account that must be signed in
    /// again is not a place that can serve.
    pub state: Option<AccountState>,
    /// The app's grant on it.
    pub verdict: Verdict,
}

fn text_of(details: &Details, name: &str) -> Option<String> {
    String::try_from(details.get(name)?.try_clone().ok()?).ok()
}

/// One row of a `Verdicts` answer; `None` for a row that says `granted` without saying which
/// grant (it grants nothing).
pub(super) fn verdict_of(row: (String, String, Details)) -> Option<AccountVerdict> {
    let (account, word, details) = row;
    let verdict = match word.as_str() {
        "granted" => {
            let grant = GrantId::parse(&text_of(&details, "grant")?).ok()?;
            // The scope's word, and for a session grant the session beside it.
            let session = match text_of(&details, "session") {
                Some(text) => Some(LauncherSession::parse(&text).ok()?),
                None => None,
            };
            let scope = GrantScope::from_words(&text_of(&details, "scope")?, session.as_ref())?;
            Verdict::Granted { grant, scope }
        }
        "denied" => Verdict::Denied,
        "ask" => Verdict::Ask,
        _ => return None,
    };
    Some(AccountVerdict {
        account: AccountId::parse(&account).ok()?,
        provider: text_of(&details, "provider"),
        label: text_of(&details, "label"),
        provider_label: text_of(&details, "provider_label"),
        state: text_of(&details, "state")
            .and_then(|word| serde_json::from_value(serde_json::Value::String(word)).ok()),
        verdict,
    })
}

/// The key on a sealed descriptor: read from the start, and only when no one can still change it.
pub(super) fn read_key(fd: OwnedFd) -> Result<SecretText, PeerError> {
    let sealed = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK;
    let seals = fcntl_get_seals(&fd).map_err(|_| PeerError::Unreadable)?;
    if !seals.contains(sealed) {
        return Err(PeerError::Unreadable);
    }
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .map_err(|_| PeerError::Unreadable)?;
    Ok(SecretText::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{MemfdFlags, fcntl_add_seals, memfd_create};
    use std::io::{Seek, Write};
    use zbus::zvariant::{OwnedValue, Value};

    fn details(pairs: &[(&str, &str)]) -> Details {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    (*name).to_owned(),
                    OwnedValue::try_from(Value::from((*value).to_owned())).expect("text"),
                )
            })
            .collect()
    }

    fn row(account: &str, word: &str, pairs: &[(&str, &str)]) -> (String, String, Details) {
        (account.to_owned(), word.to_owned(), details(pairs))
    }

    #[test]
    fn a_row_is_a_verdict_and_a_malformed_one_grants_nothing() {
        let granted = verdict_of(row(
            "openrouter",
            "granted",
            &[
                ("grant", "g-1"),
                ("scope", "always"),
                ("provider", "openrouter"),
            ],
        ))
        .expect("granted");
        assert_eq!(granted.account.as_str(), "openrouter");
        assert_eq!(granted.provider.as_deref(), Some("openrouter"));
        assert_eq!(
            granted.verdict,
            Verdict::Granted {
                grant: GrantId::parse("g-1").expect("id"),
                scope: GrantScope::Always
            }
        );
        let once =
            verdict_of(row("a", "granted", &[("grant", "g-2"), ("scope", "once")])).expect("once");
        assert!(matches!(
            once.verdict,
            Verdict::Granted {
                scope: GrantScope::Once,
                ..
            }
        ));
        let session = verdict_of(row(
            "a",
            "granted",
            &[
                ("grant", "g-3"),
                ("scope", "session"),
                ("session", "sess-1"),
            ],
        ))
        .expect("session");
        assert_eq!(
            session.verdict,
            Verdict::Granted {
                grant: GrantId::parse("g-3").expect("id"),
                scope: GrantScope::Session(LauncherSession::parse("sess-1").expect("session"))
            }
        );
        let cases = [
            (
                "a session scope without its session",
                row("a", "granted", &[("grant", "g"), ("scope", "session")]),
                None,
            ),
            (
                "a session beside a scope that has none",
                row(
                    "a",
                    "granted",
                    &[("grant", "g"), ("scope", "always"), ("session", "sess-1")],
                ),
                None,
            ),
            (
                "a session that is no id",
                row(
                    "a",
                    "granted",
                    &[("grant", "g"), ("scope", "session"), ("session", "A B")],
                ),
                None,
            ),
            ("ask", row("a", "ask", &[]), Some(Verdict::Ask)),
            ("denied", row("a", "denied", &[]), Some(Verdict::Denied)),
            (
                "granted without a grant",
                row("a", "granted", &[("scope", "always")]),
                None,
            ),
            (
                "granted without a scope",
                row("a", "granted", &[("grant", "g")]),
                None,
            ),
            (
                "granted with an unknown scope",
                row("a", "granted", &[("grant", "g"), ("scope", "forever")]),
                None,
            ),
            ("an unknown word", row("a", "maybe", &[]), None),
            ("a bad account id", row("A B", "ask", &[]), None),
        ];
        for (name, input, want) in cases {
            assert_eq!(verdict_of(input).map(|v| v.verdict), want, "{name}");
        }
    }

    fn memfd_with(text: &str, seals: SealFlags) -> OwnedFd {
        let fd = memfd_create("test-key", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)
            .expect("memfd");
        let mut file = std::fs::File::from(fd);
        file.write_all(text.as_bytes()).expect("write");
        file.rewind().expect("rewind");
        let fd = OwnedFd::from(file);
        if !seals.is_empty() {
            fcntl_add_seals(&fd, seals).expect("seal");
        }
        fd
    }

    #[test]
    fn a_key_is_read_from_a_sealed_descriptor_and_from_nothing_else() {
        let sealed = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL;
        let key = read_key(memfd_with("sk-or-v1-abc", sealed)).expect("a sealed key");
        assert_eq!(key.expose(), "sk-or-v1-abc");
        for (name, seals) in [
            ("no seals", SealFlags::empty()),
            ("only the write seal", SealFlags::WRITE),
            ("no shrink seal", SealFlags::WRITE | SealFlags::GROW),
        ] {
            assert_eq!(
                read_key(memfd_with("sk-or-v1-abc", seals)).map(|k| k.expose().to_owned()),
                Err(PeerError::Unreadable),
                "{name}"
            );
        }
    }

    #[test]
    fn a_key_never_shows_in_a_debug_line() {
        let key = read_key(memfd_with(
            "sk-or-v1-abc",
            SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK,
        ))
        .expect("key");
        assert!(!format!("{key:?}").contains("sk-or"));
    }
}
