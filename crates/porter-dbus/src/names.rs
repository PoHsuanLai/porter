//! Bus names and object paths.

use porter_core::{AccountId, object_segment};

/// accountd's bus name.
pub const ACCOUNTS_BUS: &str = "org.quire.Accounts1";
/// accountd's root object (Manager, Grants, Tokens).
pub const ACCOUNTS_PATH: &str = "/org/quire/Accounts1";
/// syncd's bus name.
pub const SYNC_BUS: &str = "org.quire.Sync1";
/// syncd's object.
pub const SYNC_PATH: &str = "/org/quire/Sync1";
/// inferd's bus name.
pub const INFERENCE_BUS: &str = "org.quire.Inference1";
/// inferd's object.
pub const INFERENCE_PATH: &str = "/org/quire/Inference1";

/// inferd's settings module (`org.quire.SettingsModule1`, declared in design/22 §9.4, not
/// here): the picker rows per kind and the `ai.model.<kind>.<tier>` map.
pub const INFERENCE_SETTINGS_PATH: &str = "/org/quire/Inference1/settings";

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
