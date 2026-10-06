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
    let cases = [
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
    let fd =
        memfd_create("test-key", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).expect("memfd");
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
            Err(AccountdFault::Unreadable),
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
