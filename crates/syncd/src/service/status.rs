//! The vardicts `Sync1` sends: `Status` (anchor age, pending, conflicts, paused, quota) and the
//! two signals' details.

use super::hub::{DatasetName, StatusSnapshot};
use crate::scheduler::Pausing;
use porter_dbus::{
    ACCOUNTS_PATH, CONFLICT_KEY_NUMBER, Details, STATUS_KEY_QUOTA, to_vardict, zvariant,
};
use porter_sync::{BaseVersion, MassDelete, Quota, RemoteSide, StoredConflict};
use serde_json::json;
use zvariant::{Dict, OwnedValue, Signature, Value};

/// `Status` keys.
pub const KEY_ANCHOR_AGE: &str = "anchor_age";
/// `Status` key: items with work left (`t`).
pub const KEY_PENDING: &str = "pending";
/// `Status` key: unsettled conflicts (`t`).
pub const KEY_CONFLICTS: &str = "conflicts";
/// `Status` key: whether the user paused it (`b`).
pub const KEY_PAUSED: &str = "paused";
/// `Status` key, present only while the dataset waits for the person (`a{sv}` `{discard: t,
/// held: t}`): the replica's listing lacks `discard` of the `held` items, and nothing has been
/// discarded.
pub const KEY_NEEDS_CONFIRMATION: &str = "needs_confirmation";
/// `NeedsConfirmation` key (`s`): the held dataset's account, as its object path.
pub const KEY_ACCOUNT: &str = "account";

fn owned(value: Value<'static>) -> Option<OwnedValue> {
    OwnedValue::try_from(value).ok()
}

/// A nested vardict of counts, `{key: t, ...}`.
fn counts_value(counts: &[(&str, u64)]) -> Option<OwnedValue> {
    let mut dict = Dict::new(&Signature::Str, &Signature::Variant);
    for (key, count) in counts {
        // The dict's own signatures, so this cannot fail.
        let _ = dict.append(
            Value::from((*key).to_owned()),
            Value::Value(Box::new(Value::U64(*count))),
        );
    }
    owned(Value::Dict(dict))
}

/// A quota as the nested vardict `{used: t, total: t}` (`total` only when there is a limit).
fn quota_value(quota: &Quota) -> Option<OwnedValue> {
    let mut counts = vec![("used", quota.used.0)];
    counts.extend(quota.total.map(|total| ("total", total.0)));
    counts_value(&counts)
}

/// What is held back as the nested vardict `{discard: t, held: t}`.
fn confirmation_value(held: &MassDelete) -> Option<OwnedValue> {
    counts_value(&[("discard", held.discard as u64), ("held", held.held as u64)])
}

/// What `Status` answers for a dataset.
pub fn status_details(status: &StatusSnapshot) -> Details {
    let mut details = Details::new();
    let mut put = |key: &str, value: Option<OwnedValue>| {
        details.extend(value.map(|v| (key.to_owned(), v)));
    };
    put(
        KEY_ANCHOR_AGE,
        status.anchor_age.and_then(|age| owned(Value::I64(age))),
    );
    put(KEY_PENDING, owned(Value::U64(status.pending)));
    put(KEY_CONFLICTS, owned(Value::U64(status.conflicts)));
    put(
        KEY_PAUSED,
        owned(Value::Bool(status.pausing == Pausing::Paused)),
    );
    put(
        STATUS_KEY_QUOTA,
        status.quota.as_ref().and_then(quota_value),
    );
    put(
        KEY_NEEDS_CONFIRMATION,
        status
            .needs_confirmation
            .as_ref()
            .and_then(confirmation_value),
    );
    details
}

/// What `NeedsConfirmation` carries for `dataset`: `{discard: t, held: t, account: s}` while
/// held (`account` is the account's object path, where Settings opens it), empty when the hold
/// ended (the signal's dataset argument says which).
pub fn held_details(dataset: &DatasetName, held: Option<&MassDelete>) -> Details {
    let mut details = Details::new();
    if let Some(held) = held {
        let mut put = |key: &str, count: usize| {
            details.extend(owned(Value::U64(count as u64)).map(|v| (key.to_owned(), v)));
        };
        put("discard", held.discard);
        put("held", held.held);
        let account = format!("{ACCOUNTS_PATH}/account/{}", dataset.account);
        details.extend(owned(Value::from(account)).map(|v| (KEY_ACCOUNT.to_owned(), v)));
    }
    details
}

/// What `Progress` carries for one finished cycle.
pub fn progress_details(fetched: u64, uploaded: u64) -> Details {
    let json = json!({ "fetched": fetched, "uploaded": uploaded });
    json.as_object().map(to_vardict).unwrap_or_default()
}

/// What `Conflict` carries: the `number` (what `Resolve` takes; absent only for a conflict not
/// stored yet), the item, the base, the replica's side and the local item.
pub fn conflict_details(stored: &StoredConflict) -> Details {
    let (side, version, id) = match &stored.conflict.remote {
        RemoteSide::Changed(v) => ("changed", Some(&v.0), None),
        RemoteSide::Deleted(v) => ("deleted", Some(&v.0), None),
        RemoteSide::Exists(id, v) => ("exists", Some(&v.0), Some(&id.0)),
    };
    let base = match &stored.conflict.base {
        BaseVersion::Absent => None,
        BaseVersion::At(v) => Some(&v.0),
    };
    let json = json!({
        CONFLICT_KEY_NUMBER: stored.number,
        "item": stored.conflict.item.0,
        "local": stored.local.0,
        "remote": side,
        "remote_version": version,
        "remote_id": id,
        "base": base,
        "at": stored.at.0,
    });
    json.as_object().map(to_vardict).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{Bytes, UnixSeconds};
    use porter_sync::{Conflict, LocalId, RemoteId, RemoteVersion};

    fn text(details: &Details, key: &str) -> Option<String> {
        details
            .get(key)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    }

    #[test]
    fn status_carries_its_keys_with_their_types_and_the_quota_as_a_nested_vardict() {
        let status = StatusSnapshot {
            anchor_age: Some(90),
            pending: 4,
            conflicts: 1,
            pausing: Pausing::Paused,
            quota: Some(Quota {
                used: Bytes(10),
                total: Some(Bytes(100)),
            }),
            needs_confirmation: None,
        };
        let details = status_details(&status);
        let get = |key: &str| details.get(key).map(|v| (**v).try_clone().expect("clone"));
        assert_eq!(get(KEY_ANCHOR_AGE), Some(Value::I64(90)));
        assert_eq!(get(KEY_PENDING), Some(Value::U64(4)));
        assert_eq!(get(KEY_CONFLICTS), Some(Value::U64(1)));
        assert_eq!(get(KEY_PAUSED), Some(Value::Bool(true)));
        let quota = details.get(STATUS_KEY_QUOTA).expect("quota");
        assert_eq!(quota.value_signature().to_string(), "a{sv}");
        let nested: std::collections::HashMap<String, OwnedValue> =
            quota.try_clone().expect("clone").try_into().expect("a{sv}");
        assert_eq!(u64::try_from(&nested["used"]).ok(), Some(10));
        assert_eq!(u64::try_from(&nested["total"]).ok(), Some(100));
        assert!(
            !details.contains_key(KEY_NEEDS_CONFIRMATION),
            "only while held"
        );
    }

    #[test]
    fn a_dataset_held_for_confirmation_says_how_many_of_how_many_would_go() {
        let status = StatusSnapshot {
            needs_confirmation: Some(MassDelete {
                discard: 30,
                held: 40,
            }),
            ..StatusSnapshot::default()
        };
        let details = status_details(&status);
        let held = details.get(KEY_NEEDS_CONFIRMATION).expect("the key");
        assert_eq!(held.value_signature().to_string(), "a{sv}");
        let nested: std::collections::HashMap<String, OwnedValue> =
            held.try_clone().expect("clone").try_into().expect("a{sv}");
        assert_eq!(u64::try_from(&nested["discard"]).ok(), Some(30));
        assert_eq!(u64::try_from(&nested["held"]).ok(), Some(40));
    }

    #[test]
    fn the_hold_signal_carries_the_counts_and_the_account_while_held_and_nothing_when_it_ends() {
        let dataset = DatasetName::parse("a1/notes").expect("name");
        let held = held_details(
            &dataset,
            Some(&MassDelete {
                discard: 30,
                held: 40,
            }),
        );
        assert_eq!(u64::try_from(&held["discard"]).ok(), Some(30));
        assert_eq!(u64::try_from(&held["held"]).ok(), Some(40));
        assert_eq!(
            text(&held, KEY_ACCOUNT).as_deref(),
            Some("/org/quire/Accounts1/account/a1")
        );
        assert!(held_details(&dataset, None).is_empty());
    }

    #[test]
    fn a_dataset_that_never_ran_has_no_anchor_age_and_no_limit_has_no_total() {
        let status = StatusSnapshot {
            quota: Some(Quota {
                used: Bytes(10),
                total: None,
            }),
            ..StatusSnapshot::default()
        };
        let details = status_details(&status);
        assert!(!details.contains_key(KEY_ANCHOR_AGE));
        let nested: std::collections::HashMap<String, OwnedValue> = details[STATUS_KEY_QUOTA]
            .try_clone()
            .expect("clone")
            .try_into()
            .expect("a{sv}");
        assert!(nested.contains_key("used") && !nested.contains_key("total"));
        assert!(!status_details(&StatusSnapshot::default()).contains_key(STATUS_KEY_QUOTA));
    }

    #[test]
    fn a_conflict_names_the_item_the_side_and_leaves_out_what_is_absent() {
        let stored = StoredConflict {
            number: Some(1),
            conflict: Conflict {
                item: RemoteId("r1".into()),
                base: BaseVersion::Absent,
                remote: RemoteSide::Exists(RemoteId("r1".into()), RemoteVersion("v3".into())),
            },
            local: LocalId("a.txt".into()),
            local_hash: None,
            at: UnixSeconds(5),
        };
        let details = conflict_details(&stored);
        assert_eq!(text(&details, "item").as_deref(), Some("r1"));
        assert_eq!(text(&details, "remote").as_deref(), Some("exists"));
        assert_eq!(text(&details, "remote_version").as_deref(), Some("v3"));
        assert_eq!(text(&details, "local").as_deref(), Some("a.txt"));
        assert!(!details.contains_key("base"));
        assert_eq!(details["number"].value_signature().to_string(), "x");
        assert_eq!(i64::try_from(&details["number"]).ok(), Some(1));
        let progress = progress_details(2, 1);
        assert_eq!(progress.len(), 2);
    }
}
