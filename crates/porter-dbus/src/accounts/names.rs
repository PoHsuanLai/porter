//! The keys of the rows accountd gives, and the object path of one account. The bus names and
//! paths are `porter_wire::names`.

use porter_core::AccountId;
use porter_ids::object_segment;
use porter_wire::names::ACCOUNTS_PATH;

/// `Spaces1.List`'s key of a Space's name (`s`).
pub const SPACE_KEY_NAME: &str = "name";
/// `Spaces1.List`'s key of a Space's look (`s`, opaque).
pub const SPACE_KEY_LOOK: &str = "look";
/// `Spaces1.List`'s key of when a Space was made (`x`, Unix seconds).
pub const SPACE_KEY_CREATED: &str = "created";

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
