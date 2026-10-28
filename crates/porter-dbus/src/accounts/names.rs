//! accountd's bus names and object paths, and the keys of the rows it gives.

use porter_core::AccountId;
use porter_ids::object_segment;

/// accountd's bus name.
pub const ACCOUNTS_BUS: &str = "org.quire.Accounts1";
/// accountd's root object (Manager, the add-account sheet).
pub const ACCOUNTS_PATH: &str = "/org/quire/Accounts1";
/// The desktop-wide Spaces (`org.quire.Spaces1`), served on accountd's bus name.
pub const SPACES_PATH: &str = "/org/quire/Spaces1";
/// `Spaces1.List`'s key of a Space's name (`s`).
pub const SPACE_KEY_NAME: &str = "name";
/// `Spaces1.List`'s key of a Space's look (`s`, opaque).
pub const SPACE_KEY_LOOK: &str = "look";
/// `Spaces1.List`'s key of when a Space was made (`x`, Unix seconds).
pub const SPACE_KEY_CREATED: &str = "created";
/// The identities signed in on this machine (`org.quire.Identities1`), served on accountd's bus
/// name.
pub const IDENTITIES_PATH: &str = "/org/quire/Identities1";
/// The person's computers on their Tailscale network (`org.quire.Tailnet1`), served on
/// accountd's bus name.
pub const TAILNET_PATH: &str = "/org/quire/Tailnet1";
/// The sheet host's bus name (`org.quire.AccountsSheet1`, served by sill or a standalone host).
pub const SHEET_BUS: &str = "org.quire.AccountsSheet1";
/// The sheet host's object.
pub const SHEET_PATH: &str = "/org/quire/AccountsSheet1";
/// accountd's settings module (`org.quire.SettingsModule1`): the account, grant and client-id
/// rows the Accounts pane reads.
pub const ACCOUNTS_SETTINGS_PATH: &str = "/org/quire/Accounts1/settings";

/// The object path of one account (`org.quire.Accounts1.Account`).
pub fn account_path(id: &AccountId) -> String {
    format!("{ACCOUNTS_PATH}/account/{}", object_segment(id.as_str()))
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
