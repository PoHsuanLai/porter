//! The portal shape of a sheet's answer: the response codes, the results vardict (through the
//! real `(u, a{sv})` wire encoding), the request path and the handle token.

use porter_core::capability::{Access, Capability, Delta, HashKind, Offered};
use porter_core::capability::{QuotaReport, StorageCap, StorageScope};
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountLabel, AccountsReply, Candidate, EndpointUrl, Family, GrantId, LoginName,
    ProviderId, Restriction, ServiceEndpoint, Subject, Tls,
};
use porter_dbus::{
    Details, ResponseCode, SheetKind, account_path, is_handle_token, reply_of, request_namespace,
    request_path, response_of, sender_segment,
};
use zbus::zvariant::{OwnedValue, Value};

fn candidate() -> Candidate {
    Candidate::new(
        AccountId::parse("67e55044-10b1.x").expect("id"),
        AccountLabel("ada@example.org".into()),
        ProviderId::parse("nextcloud").expect("provider"),
        Subject::Account,
        Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::QuickXor,
            ranges: Offered::Present,
            chunked_upload: Offered::Present,
        }),
        Restriction::none(),
        GrantId::parse("grant-1").expect("grant"),
    )
    .with_endpoints(vec![ServiceEndpoint {
        family: Family::WebDav,
        url: EndpointUrl::parse("https://cloud.example.org/dav/").expect("url"),
        tls: Tls::Implicit,
        login: LoginName("ada".into()),
    }])
}

const REFUSALS: [Refusal; 7] = [
    Refusal::Dismissed,
    Refusal::Denied,
    Refusal::NoFittingAccount,
    Refusal::UnknownGrant,
    Refusal::AudienceNotGranted,
    Refusal::NeedsReauth,
    Refusal::Unavailable,
];

/// What crosses the bus: the response and its results through `(u, a{sv})` and back.
fn over_the_wire(code: u32, results: &Details) -> (u32, Details) {
    let ctxt = zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0);
    let bytes = zbus::zvariant::to_bytes(ctxt, &(code, results)).expect("encodes");
    let (back, _): ((u32, Details), usize) = bytes.deserialize().expect("decodes");
    back
}

fn crossing(kind: SheetKind, reply: &AccountsReply) -> (u32, Result<AccountsReply, String>) {
    let response = response_of(kind, reply);
    let (code, results) = over_the_wire(response.code.to_wire(), &response.results);
    (
        code,
        reply_of(kind, code, results).map_err(|e| e.to_string()),
    )
}

#[test]
fn every_answer_survives_the_response_and_keeps_its_code() {
    let added = AccountId::parse("nextcloud-1").expect("id");
    let mut cases: Vec<(&str, SheetKind, AccountsReply, u32)> = vec![
        (
            "chosen",
            SheetKind::Choose,
            AccountsReply::Chosen(candidate()),
            0,
        ),
        (
            "added",
            SheetKind::AddAccount,
            AccountsReply::Added(added.clone()),
            0,
        ),
        (
            "already here",
            SheetKind::AddAccount,
            AccountsReply::AlreadyAdded(added),
            0,
        ),
        (
            "signed in again",
            SheetKind::Reauthenticate,
            AccountsReply::Reauthenticated,
            0,
        ),
    ];
    for kind in [
        SheetKind::Choose,
        SheetKind::AddAccount,
        SheetKind::Reauthenticate,
    ] {
        for refusal in REFUSALS {
            let code = if refusal == Refusal::Dismissed { 1 } else { 2 };
            cases.push(("refused", kind, AccountsReply::Refused(refusal), code));
        }
    }
    for (name, kind, reply, code) in cases {
        let (got_code, back) = crossing(kind, &reply);
        assert_eq!(got_code, code, "{name} {kind:?} {reply:?}");
        assert_eq!(back, Ok(reply.clone()), "{name} {kind:?}");
    }
}

#[test]
fn dont_allow_is_other_with_its_reason_and_closing_the_sheet_is_cancelled() {
    let denied = response_of(SheetKind::Choose, &AccountsReply::Refused(Refusal::Denied));
    assert_eq!(denied.code, ResponseCode::Other);
    assert_eq!(
        String::try_from(denied.results["refusal"].try_clone().expect("clone")).expect("text"),
        "denied"
    );
    let closed = response_of(
        SheetKind::Choose,
        &AccountsReply::Refused(Refusal::Dismissed),
    );
    assert_eq!(closed.code, ResponseCode::Cancelled);
    assert!(closed.results.is_empty());
}

#[test]
fn a_chosen_account_carries_its_path_and_label_beside_its_fields() {
    let response = response_of(SheetKind::Choose, &AccountsReply::Chosen(candidate()));
    let mut keys: Vec<&str> = response.results.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "account",
            "capability",
            "endpoints",
            "grant",
            "label",
            "path",
            "provider",
            "restriction",
            "subject"
        ]
    );
    let path: &Value<'_> = &response.results["path"];
    assert_eq!(
        *path,
        Value::from(
            zbus::zvariant::ObjectPath::try_from(account_path(&candidate().account)).expect("path")
        )
    );
}

#[test]
fn a_reply_that_does_not_answer_the_sheet_is_other_unavailable() {
    let response = response_of(SheetKind::AddAccount, &AccountsReply::Revoked);
    assert_eq!(response.code, ResponseCode::Other);
    let (_, back) = crossing(SheetKind::AddAccount, &AccountsReply::Revoked);
    assert_eq!(back, Ok(AccountsReply::Refused(Refusal::Unavailable)));
    let chosen_for_add = crossing(SheetKind::AddAccount, &AccountsReply::Chosen(candidate()));
    assert_eq!(chosen_for_add.0, 2);
}

fn text(value: &str) -> OwnedValue {
    OwnedValue::try_from(Value::from(value.to_owned())).expect("value")
}

#[test]
fn a_response_that_breaks_its_promise_is_refused_not_guessed() {
    let with = |entries: &[(&str, OwnedValue)]| -> Details {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.try_clone().expect("clone")))
            .collect()
    };
    let cases: Vec<(&str, SheetKind, u32, Details)> = vec![
        ("code 3", SheetKind::Choose, 3, Details::new()),
        ("code far out", SheetKind::Choose, u32::MAX, Details::new()),
        (
            "done with no candidate",
            SheetKind::Choose,
            0,
            Details::new(),
        ),
        ("other with no reason", SheetKind::Choose, 2, Details::new()),
        (
            "other with an unknown reason",
            SheetKind::Choose,
            2,
            with(&[("refusal", text("sulking"))]),
        ),
        (
            "other with a reason that is not text",
            SheetKind::Choose,
            2,
            with(&[(
                "refusal",
                OwnedValue::try_from(Value::U32(1)).expect("value"),
            )]),
        ),
        (
            "added with nothing",
            SheetKind::AddAccount,
            0,
            Details::new(),
        ),
        (
            "added with a path that is not the account's",
            SheetKind::AddAccount,
            0,
            with(&[
                ("account", text("nextcloud-1")),
                (
                    "path",
                    OwnedValue::try_from(Value::from(
                        zbus::zvariant::ObjectPath::try_from("/org/quire/Accounts1/account/other")
                            .expect("path"),
                    ))
                    .expect("value"),
                ),
            ]),
        ),
        (
            "added with a path that is text",
            SheetKind::AddAccount,
            0,
            with(&[
                ("account", text("nextcloud-1")),
                ("path", text("/org/quire/Accounts1/account/nextcloud_1")),
            ]),
        ),
    ];
    for (name, kind, code, results) in cases {
        assert!(reply_of(kind, code, results).is_err(), "{name}");
    }
}

#[test]
fn sender_names_become_path_segments_as_portals_write_them() {
    let cases = [
        (":1.42", "1_42"),
        (":1.7", "1_7"),
        (":1.4294967295", "1_4294967295"),
        ("1.2", "1_2"),
    ];
    for (name, segment) in cases {
        assert_eq!(sender_segment(name), segment, "{name}");
    }
    assert_eq!(
        request_namespace(":1.42"),
        "/org/quire/Accounts1/request/1_42"
    );
}

#[test]
fn a_handle_token_is_a_path_segment_or_it_is_not_one() {
    let long = "x".repeat(64);
    let too_long = "x".repeat(65);
    let cases: [(&str, bool); 9] = [
        ("porter_1_0", true),
        ("A", true),
        (long.as_str(), true),
        (too_long.as_str(), false),
        ("", false),
        ("a/b", false),
        ("a.b", false),
        ("a b", false),
        ("naïve", false),
    ];
    for (token, ok) in cases {
        assert_eq!(is_handle_token(token), ok, "{token:?}");
        let path = request_path(":1.42", token);
        assert_eq!(path.is_some(), ok, "{token:?}");
        if let Some(path) = path {
            assert!(zbus::zvariant::ObjectPath::try_from(path.as_str()).is_ok());
            assert_eq!(path, format!("/org/quire/Accounts1/request/1_42/{token}"));
        }
    }
}

#[test]
fn every_refusal_has_a_distinct_response() {
    let mut seen = std::collections::BTreeSet::new();
    for refusal in REFUSALS {
        let response = response_of(SheetKind::Choose, &AccountsReply::Refused(refusal));
        let reason = response
            .results
            .get("refusal")
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        assert!(
            seen.insert((response.code.to_wire(), reason)),
            "{refusal:?}"
        );
    }
    assert_eq!(seen.len(), REFUSALS.len());
}
