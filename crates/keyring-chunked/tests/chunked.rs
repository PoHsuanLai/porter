//! keyring-chunked over keyring-core's mock store: values in parts and whole, shrinking, a
//! write that fails part-way, damaged heads, and the entries porter and mailo wrote before this
//! crate. Nothing here touches a real keyring.

use keyring_chunked::{ChunkError, ChunkedStore, Error, Limit, MOST_PARTS};
use keyring_core::api::CredentialStoreApi;
use keyring_core::{CredentialStore, Entry, mock};
use std::collections::HashMap;
use std::sync::Arc;

const SERVICE: &str = "porter";
const MARK: &str = "porter-parts:";
/// Room for the head's marker and count, and small enough that a test value needs parts.
const SIXTEEN: Limit = Limit::Utf16Units(16);

fn mock() -> Arc<mock::Store> {
    mock::Store::new().expect("mock")
}

fn chunked(mock: &Arc<mock::Store>, limit: Limit) -> ChunkedStore<mock::Store> {
    ChunkedStore::new(mock.clone(), MARK, limit).expect("store")
}

fn entry(store: &ChunkedStore<mock::Store>, user: &str) -> Entry {
    store.build(SERVICE, user, None).expect("entry")
}

/// The users that hold a value in the mock, sorted.
fn names(mock: &mock::Store) -> Vec<String> {
    let creds = mock.inner.lock().expect("mock");
    let mut names: Vec<String> = creds
        .borrow()
        .iter()
        .filter(|cred| cred.inner.lock().expect("cred").borrow().secret.is_some())
        .map(|cred| cred.specifiers.1.clone())
        .collect();
    names.sort();
    names
}

/// What the mock holds under `user`, as the wrapped store would read it.
fn raw(mock: &mock::Store, user: &str) -> Option<String> {
    mock.build(SERVICE, user, None)
        .expect("entry")
        .get_password()
        .ok()
}

fn plant(mock: &mock::Store, user: &str, value: &str) {
    mock.build(SERVICE, user, None)
        .expect("entry")
        .set_password(value)
        .expect("planted");
}

/// The next call on the mock's entry `user` fails.
fn fail_next(mock: &mock::Store, user: &str) {
    let entry = mock.build(SERVICE, user, None).expect("entry");
    let cred: &mock::Cred = entry.as_any().downcast_ref().expect("a mock credential");
    cred.set_error(Error::PlatformFailure("the store refused".into()));
}

fn chunk_error(error: Error) -> ChunkError {
    match error {
        Error::BadDataFormat(_, source) => source
            .downcast_ref::<ChunkError>()
            .expect("a chunk error")
            .clone(),
        other => panic!("not a chunk error: {other:?}"),
    }
}

#[test]
fn a_value_too_long_for_one_entry_is_kept_in_parts_and_read_back_whole() {
    let mock = mock();
    let store = chunked(&mock, SIXTEEN);
    let token = "{\"access\":\"eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9-😀-déjà\"}";
    entry(&store, "acct:oauth")
        .set_password(token)
        .expect("set");
    let names = names(&mock);
    assert!(names.len() > 2, "{names:?}");
    for name in &names {
        let held = raw(&mock, name).expect("held");
        assert!(held.encode_utf16().count() <= 16, "{name}: {held:?}");
    }
    assert_eq!(
        raw(&mock, "acct:oauth").as_deref(),
        Some(format!("{MARK}{}", names.len() - 1).as_str())
    );
    assert_eq!(
        entry(&store, "acct:oauth").get_password().expect("get"),
        token
    );
}

#[test]
fn a_value_that_fits_is_one_entry_holding_exactly_the_value() {
    // What the Keychain and the Secret Service have always held: nothing about them moves.
    let mock = mock();
    for limit in [Limit::None, SIXTEEN] {
        entry(&chunked(&mock, limit), "acct:incoming")
            .set_password("\"hunter2\"")
            .expect("set");
        assert_eq!(names(&mock), ["acct:incoming"]);
        assert_eq!(raw(&mock, "acct:incoming").as_deref(), Some("\"hunter2\""));
    }
    let long = "x".repeat(5000);
    entry(&chunked(&mock, Limit::None), "k")
        .set_password(&long)
        .expect("set");
    assert_eq!(raw(&mock, "k"), Some(long));
}

#[test]
fn a_shorter_value_leaves_no_parts_of_the_longer_one_behind() {
    let mock = mock();
    let store = chunked(&mock, SIXTEEN);
    let k = entry(&store, "k");
    k.set_password(&"x".repeat(40)).expect("set");
    assert_eq!(names(&mock), ["k", "k#1", "k#2", "k#3"]);
    k.set_password(&"y".repeat(20)).expect("shrink");
    assert_eq!(names(&mock), ["k", "k#1", "k#2"]);
    assert_eq!(k.get_password().expect("get"), "y".repeat(20));
    k.set_password("\"z\"").expect("whole");
    assert_eq!(names(&mock), ["k"]);
    assert_eq!(k.get_password().expect("get"), "\"z\"");
}

#[test]
fn parts_an_earlier_write_left_past_the_head_are_swept() {
    let mock = mock();
    plant(&mock, "k", "\"whole\"");
    plant(&mock, "k#3", "left");
    plant(&mock, "k#4", "behind");
    entry(&chunked(&mock, SIXTEEN), "k")
        .set_password(&"x".repeat(20))
        .expect("set");
    assert_eq!(names(&mock), ["k", "k#1", "k#2"]);
}

#[test]
fn deleting_removes_every_part_and_a_second_delete_finds_nothing() {
    let mock = mock();
    let store = chunked(&mock, SIXTEEN);
    entry(&store, "k")
        .set_password(&"x".repeat(40))
        .expect("set");
    entry(&store, "other")
        .set_password("\"kept\"")
        .expect("set");
    entry(&store, "k").delete_credential().expect("delete");
    assert_eq!(names(&mock), ["other"]);
    assert!(matches!(
        entry(&store, "k").delete_credential(),
        Err(Error::NoEntry)
    ));
    assert!(matches!(
        entry(&store, "k").get_password(),
        Err(Error::NoEntry)
    ));
    assert!(matches!(
        entry(&store, "k").get_credential(),
        Err(Error::NoEntry)
    ));
}

#[test]
fn a_missing_part_or_a_damaged_head_is_an_error_and_not_a_shorter_value() {
    let mock = mock();
    let store = chunked(&mock, SIXTEEN);
    entry(&store, "k")
        .set_password(&"x".repeat(40))
        .expect("set");
    mock.build(SERVICE, "k#3", None)
        .expect("entry")
        .delete_credential()
        .expect("gone");
    let error = entry(&store, "k").get_password().expect_err("incomplete");
    assert_eq!(
        chunk_error(error),
        ChunkError::MissingPart { index: 3, count: 3 }
    );

    for head in [
        "porter-parts:",
        "porter-parts:1",
        "porter-parts:65",
        "porter-parts:-2",
    ] {
        let mock = self::mock();
        let store = chunked(&mock, SIXTEEN);
        plant(&mock, "k", head);
        plant(&mock, "k#1", "a part");
        let error = entry(&store, "k").get_password().expect_err(head);
        assert_eq!(chunk_error(error), ChunkError::BadHead, "{head}");
        // And it can still be deleted, which is how the user gets out of it.
        entry(&store, "k").delete_credential().expect(head);
        assert!(names(&mock).is_empty(), "{head}");
    }
}

#[test]
fn a_write_that_fails_part_way_leaves_the_old_value_or_nothing_but_never_a_mix() {
    // The old value whole: it stays, and the new parts written so far go.
    let mock = mock();
    let store = chunked(&mock, SIXTEEN);
    entry(&store, "k").set_password("\"old\"").expect("set");
    fail_next(&mock, "k#2");
    entry(&store, "k")
        .set_password(&"n".repeat(40))
        .expect_err("refused");
    assert_eq!(names(&mock), ["k"]);
    assert_eq!(entry(&store, "k").get_password().expect("old"), "\"old\"");

    // The old value in parts, some overwritten: all of it goes, and the entry reads as missing.
    let mock = self::mock();
    let store = chunked(&mock, SIXTEEN);
    entry(&store, "k")
        .set_password(&"o".repeat(40))
        .expect("set");
    fail_next(&mock, "k#2");
    entry(&store, "k")
        .set_password(&"n".repeat(60))
        .expect_err("refused");
    assert_eq!(names(&mock), Vec::<String>::new());
    assert!(matches!(
        entry(&store, "k").get_password(),
        Err(Error::NoEntry)
    ));

    // The first part refused: nothing was overwritten, so the old value stays.
    let mock = self::mock();
    let store = chunked(&mock, SIXTEEN);
    entry(&store, "k")
        .set_password(&"o".repeat(40))
        .expect("set");
    fail_next(&mock, "k#1");
    entry(&store, "k")
        .set_password(&"n".repeat(60))
        .expect_err("refused");
    assert_eq!(
        entry(&store, "k").get_password().expect("old"),
        "o".repeat(40)
    );
}

/// What porter-secrets' own `chunks.rs` wrote before this crate, at a limit of 16 units: the
/// format must read unchanged, and writing the same value must give the same entries.
const PORTER_WROTE: &[(&str, &str)] = &[
    ("acct:oauth", "porter-parts:4"),
    ("acct:oauth#1", "{\"refresh\":\"1//0"),
    ("acct:oauth#2", "gYw-déjà-vu-😀-t"),
    ("acct:oauth#3", "oken\",\"expires\":"),
    ("acct:oauth#4", "1767225600}"),
];
const PORTER_VALUE: &str = "{\"refresh\":\"1//0gYw-déjà-vu-😀-token\",\"expires\":1767225600}";

#[test]
fn entries_porter_wrote_before_this_crate_read_back_and_are_written_the_same_way() {
    let mock = mock();
    for (user, value) in PORTER_WROTE {
        plant(&mock, user, value);
    }
    // Read at any limit: the head says how the value was cut.
    for limit in [Limit::None, SIXTEEN, Limit::WINDOWS] {
        let got = entry(&chunked(&mock, limit), "acct:oauth").get_password();
        assert_eq!(got.expect("read"), PORTER_VALUE, "{limit:?}");
    }

    let again = self::mock();
    entry(&chunked(&again, SIXTEEN), "acct:oauth")
        .set_password(PORTER_VALUE)
        .expect("set");
    for (user, value) in PORTER_WROTE {
        assert_eq!(raw(&again, user).as_deref(), Some(*value), "{user}");
    }
    assert_eq!(names(&again).len(), PORTER_WROTE.len());
}

#[test]
fn mailos_entries_read_with_mailos_marker() {
    // mailo's copy (`signing_store/chunks.rs`) writes the same layout under `mailo-parts:`.
    let mock = mock();
    plant(&mock, "smime:ab", "mailo-parts:2");
    plant(&mock, "smime:ab#1", "{\"smime_key\":");
    plant(&mock, "smime:ab#2", "\"pem\"}");
    let mailo = ChunkedStore::new(mock.clone(), "mailo-parts:", SIXTEEN).expect("store");
    let got = mailo.build(SERVICE, "smime:ab", None).expect("entry");
    assert_eq!(got.get_password().expect("read"), "{\"smime_key\":\"pem\"}");
    // Under porter's marker the head is just a value: the marker must match the writer's.
    let porter = entry(&chunked(&mock, SIXTEEN), "smime:ab");
    assert_eq!(porter.get_password().expect("read"), "mailo-parts:2");
}

#[test]
fn a_value_that_begins_with_the_marker_is_kept_in_parts_and_reads_back() {
    let mock = mock();
    for limit in [Limit::None, SIXTEEN] {
        let k = entry(&chunked(&mock, limit), "k");
        k.set_password("porter-parts:7").expect("set");
        assert_eq!(
            k.get_password().expect("get"),
            "porter-parts:7",
            "{limit:?}"
        );
        assert_eq!(raw(&mock, "k").as_deref(), Some("porter-parts:2"));
        assert_eq!(raw(&mock, "k#1").as_deref(), Some("porter-parts:7"));
    }
}

#[test]
fn bytes_are_cut_at_two_to_a_unit_and_read_back_whole() {
    let mock = mock();
    let store = chunked(&mock, Limit::Utf16Units(4));
    let secret: Vec<u8> = (0..=255u8).rev().collect();
    entry(&store, "key").set_secret(&secret).expect("set");
    assert_eq!(entry(&store, "key").get_secret().expect("get"), secret);
    let part = mock.build(SERVICE, "key#1", None).expect("entry");
    assert_eq!(part.get_secret().expect("part").len(), 8);
    entry(&store, "key").delete_credential().expect("delete");
    assert!(names(&mock).is_empty());
}

#[test]
fn a_value_needing_more_than_the_most_parts_is_refused_and_nothing_is_written() {
    let mock = mock();
    let store = chunked(&mock, Limit::Utf16Units(2));
    let error = entry(&store, "k")
        .set_password(&"x".repeat(2 * MOST_PARTS + 1))
        .expect_err("too long");
    assert!(matches!(error, Error::TooLong(..)), "{error:?}");
    assert!(names(&mock).is_empty());
}

#[test]
fn the_store_wraps_a_store_chosen_at_run_time_and_searches_without_parts() {
    let mock = mock();
    let any: Arc<CredentialStore> = mock.clone();
    let store: ChunkedStore = ChunkedStore::new(any, MARK, SIXTEEN).expect("store");
    let k = store.build(SERVICE, "k", None).expect("entry");
    k.set_password(&"x".repeat(40)).expect("set");
    store
        .build(SERVICE, "other", None)
        .expect("entry")
        .set_password("\"o\"")
        .expect("set");
    let spec = HashMap::from([("service", SERVICE)]);
    let mut found: Vec<String> = store
        .search(&spec)
        .expect("search")
        .iter()
        .filter(|e| e.get_password().is_ok())
        .filter_map(|e| e.get_specifiers().map(|(_, user)| user))
        .collect();
    found.sort();
    assert_eq!(found, ["k", "other"]);
    assert_eq!(
        k.get_specifiers(),
        Some((SERVICE.to_owned(), "k".to_owned()))
    );
    assert!(k.get_credential().is_ok());
}

#[test]
fn an_empty_marker_is_refused() {
    let error = ChunkedStore::new(mock(), "", SIXTEEN).expect_err("empty");
    assert_eq!(error, ChunkError::EmptyMarker);
}
