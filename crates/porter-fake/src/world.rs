//! A service over the three fake accounts, as a daemon or an in-process host would build it.

use crate::accounts::{llm_account, mail_account, storage_account};
use crate::clock::FixedClock;
use crate::prompter::ScriptedPrompter;
use crate::provider::{FakeProvider, cloud_provider, llm_provider, mail_provider};
use porter_core::{Credential, SecretKey, SecretPurpose, SecretText, UnixSeconds};
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};

/// The instant the fake clock reads.
pub const NOW: UnixSeconds = UnixSeconds(1_790_000_000);

/// The service type the fakes make.
pub type FakeService = AccountService<FakeProvider, MemorySecrets, ScriptedPrompter, FixedClock>;

/// A service with the Storage, Mail and Llm accounts, their secrets filed, no grants, and
/// `prompter` drawing the consent sheet.
pub async fn fake_service(prompter: ScriptedPrompter) -> FakeService {
    let secrets = MemorySecrets::default();
    let filed = [
        (
            storage_account().id,
            SecretPurpose::Password,
            Credential::Password(SecretText::new("app-pw")),
        ),
        (
            mail_account().id,
            SecretPurpose::OAuthRefresh,
            Credential::OAuth {
                access: SecretText::new("access"),
                refresh: SecretText::new("refresh"),
                expires_at: NOW,
            },
        ),
    ];
    for (account, purpose, credential) in filed {
        let key = SecretKey { account, purpose };
        // The in-memory store cannot fail.
        let _ = secrets.put(&key, &credential).await;
    }
    let registry = Registry {
        accounts: vec![storage_account(), mail_account(), llm_account()],
        grants: vec![],
    };
    let providers = vec![cloud_provider(), mail_provider(), llm_provider()];
    AccountService::new(providers, registry, secrets, prompter, FixedClock(NOW))
}
