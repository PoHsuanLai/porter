//! Bus names and object paths.

use porter_core::{AccountId, object_segment};

/// accountd's bus name.
pub const ACCOUNTS_BUS: &str = "org.quire.Accounts1";
/// accountd's root object (Manager, Grants, Tokens).
pub const ACCOUNTS_PATH: &str = "/org/quire/Accounts1";
/// The desktop-wide Spaces (`org.quire.Spaces1`), served on accountd's bus name.
pub const SPACES_PATH: &str = "/org/quire/Spaces1";
/// `Spaces1.List`'s key of a Space's name (`s`).
pub const SPACE_KEY_NAME: &str = "name";
/// `Spaces1.List`'s key of a Space's look (`s`, opaque).
pub const SPACE_KEY_LOOK: &str = "look";
/// `Spaces1.List`'s key of when a Space was made (`x`, Unix seconds).
pub const SPACE_KEY_CREATED: &str = "created";
/// syncd's bus name.
pub const SYNC_BUS: &str = "org.quire.Sync1";
/// syncd's object.
pub const SYNC_PATH: &str = "/org/quire/Sync1";
/// The sheet host's bus name (`org.quire.AccountsSheet1`, served by sill or a standalone host).
pub const SHEET_BUS: &str = "org.quire.AccountsSheet1";
/// The sheet host's object.
pub const SHEET_PATH: &str = "/org/quire/AccountsSheet1";

/// The key of the quota in `Sync1.Status` (a vardict `a{sv}` holding `used` as a `t` and, when
/// the provider reports a limit, `total` as a `t`). syncd builds it from porter-sync's `Quota`.
pub const STATUS_KEY_QUOTA: &str = "quota";

/// The key of the conflict's number in the `Sync1.Conflict` signal's details (an `x`): the
/// number `Sync1.Resolve` takes as `conflict`.
pub const CONFLICT_KEY_NUMBER: &str = "number";

/// `Sync1.Resolve`'s `how`: upload the local content over the replica's current version.
pub const RESOLVE_KEEP_LOCAL: &str = "keep_local";
/// `Sync1.Resolve`'s `how`: take the replica's version and drop the local change.
pub const RESOLVE_KEEP_REMOTE: &str = "keep_remote";

/// The prefix of the errors `Sync1` names itself (a refusal of accountd's vocabulary keeps its
/// `org.quire.Accounts1.Error.` name).
pub const SYNC_ERROR_PREFIX: &str = "org.quire.Sync1.Error.";
/// `Sync1.Resolve` on a conflict that is not there: never stored, or already settled
/// (`org.quire.Sync1.Error.NoSuchConflict`).
pub const SYNC_ERROR_NO_SUCH_CONFLICT: &str = "org.quire.Sync1.Error.NoSuchConflict";
/// `Sync1.ConfirmDiscard` on a dataset that is not held for confirmation: it never was, or the
/// discard was already confirmed (`org.quire.Sync1.Error.NothingHeld`).
pub const SYNC_ERROR_NOTHING_HELD: &str = "org.quire.Sync1.Error.NothingHeld";

/// inferd's bus name.
pub const INFERENCE_BUS: &str = "org.quire.Inference1";
/// inferd's object.
pub const INFERENCE_PATH: &str = "/org/quire/Inference1";

/// accountd's settings module (`org.quire.SettingsModule1`): the account, grant and client-id
/// rows the Accounts pane reads.
pub const ACCOUNTS_SETTINGS_PATH: &str = "/org/quire/Accounts1/settings";

/// inferd's settings module (`org.quire.SettingsModule1`, declared in design/22 §9.4, not
/// here): the picker rows per kind and the `ai.model.<kind>.<tier>` map.
pub const INFERENCE_SETTINGS_PATH: &str = "/org/quire/Inference1/settings";

/// The reserved key of the W3C trace context in an `options` vardict (`Inference1.Open`,
/// `Prepare`, `Availability`, and the same dictionary on the companion and intents interfaces).
/// The value is a version 00 `traceparent` string (`porter_infer::Traceparent`); a daemon that
/// finds none starts its own root, and an unknown key is ignored.
pub const OPTION_TRACEPARENT: &str = "traceparent";

/// The reserved key of the session's usage in the same `options` vardict (`Inference1.Open`,
/// `Prepare`, `Availability`): whether a person is waiting on it or nobody is. The value is a
/// string, `porter_core::consent::Usage`'s slug (`interactive`, `background`); inferd asks
/// accountd's `Verdicts` with it. Absent means `interactive`; an unknown slug is refused as
/// invalid args.
pub const OPTION_USAGE: &str = "usage";

/// The object path of one account (`org.quire.Accounts1.Account`).
pub fn account_path(id: &AccountId) -> String {
    format!("{ACCOUNTS_PATH}/account/{}", object_segment(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_paths_are_valid_object_paths() {
        let id = AccountId::parse("67e55044-10b1.x").expect("id");
        let path = account_path(&id);
        assert_eq!(path, "/org/quire/Accounts1/account/67e55044_10b1_x");
        assert!(zbus::zvariant::ObjectPath::try_from(path.as_str()).is_ok());
    }
}
