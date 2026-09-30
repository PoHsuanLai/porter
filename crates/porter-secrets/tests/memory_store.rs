//! The in-memory store keeps the `Secrets` contract the daemon relies on.

use porter_core::{AccountId, Credential, SecretKey, SecretPurpose, SecretText};
use porter_secrets::{MemorySecrets, Secrets, SecretsError};

fn key(account: &str, purpose: SecretPurpose) -> SecretKey {
    SecretKey {
        account: AccountId::parse(account).expect("id"),
        purpose,
    }
}

#[tokio::test]
async fn a_filed_secret_comes_back_and_goes_away() {
    let store = MemorySecrets::default();
    let filed = key("cloud", SecretPurpose::Password);
    assert_eq!(store.get(&filed).await, Err(SecretsError::Missing));
    let value = Credential::Password(SecretText::new("hunter2"));
    store.put(&filed, &value).await.expect("put");
    assert_eq!(store.get(&filed).await, Ok(value));
    store.delete(&filed).await.expect("delete");
    assert_eq!(store.get(&filed).await, Err(SecretsError::Missing));
}

#[tokio::test]
async fn removing_an_account_removes_only_its_secrets() {
    let store = MemorySecrets::default();
    let value = Credential::ApiKey(SecretText::new("k"));
    let gone = [
        key("cloud", SecretPurpose::Password),
        key("cloud", SecretPurpose::OAuthRefresh),
    ];
    let kept = key("other", SecretPurpose::Password);
    for filed in gone.iter().chain([&kept]) {
        store.put(filed, &value).await.expect("put");
    }
    store
        .delete_account(&AccountId::parse("cloud").expect("id"))
        .await
        .expect("wipe");
    for filed in &gone {
        assert_eq!(store.get(filed).await, Err(SecretsError::Missing));
    }
    assert_eq!(store.get(&kept).await, Ok(value));
}
