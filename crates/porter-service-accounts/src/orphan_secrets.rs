//! The startup sweep of orphan secrets: credentials filed for an account the registry does not
//! name. An add files its secrets before it adds the row (the id is reserved meanwhile), so a
//! crash between the two leaves secrets no row owns. The daemon removes them once, at start,
//! before it serves, and audits each removal.

use porter_core::AccountId;
use porter_ids::clock::Clock;
use porter_secrets::Secrets;
use porter_service_core::audit::{AuditEntry, AuditEvent, AuditSink};
use porter_service_core::registry::Registry;

/// The accounts `filed` holds secrets for that `registry` does not name. Nothing is an orphan
/// when the registry names no account at all: an empty registry is as likely a lost file as a
/// true absence, and a credential removed wrongly cannot be brought back.
pub fn orphans(registry: &Registry, filed: &[AccountId]) -> Vec<AccountId> {
    if registry.accounts.is_empty() {
        return Vec::new();
    }
    filed
        .iter()
        .filter(|id| !registry.accounts.iter().any(|a| a.id == **id))
        .cloned()
        .collect()
}

/// Removes the secrets of each orphan in `secrets` and records one `SecretsSwept` line for it.
/// Returns the accounts swept. A store that cannot list, or a removal that fails, is said and
/// left for the next start; nothing here stops the daemon.
pub async fn sweep_orphan_secrets<S: Secrets, A: AuditSink>(
    secrets: &S,
    registry: &Registry,
    audit: &A,
    clock: &dyn Clock,
) -> Vec<AccountId> {
    let filed = match secrets.accounts_filed().await {
        Ok(filed) => filed,
        Err(why) => {
            eprintln!("accountd: the secrets could not be listed for the orphan sweep: {why}");
            return Vec::new();
        }
    };
    let mut swept = Vec::new();
    for id in orphans(registry, &filed) {
        match secrets.delete_account(&id).await {
            Ok(()) => {
                audit.record(AuditEntry::new(
                    clock.now(),
                    None,
                    Some(id.clone()),
                    AuditEvent::SecretsSwept,
                ));
                swept.push(id);
            }
            Err(why) => eprintln!("accountd: orphan secrets of {id} were not removed: {why}"),
        }
    }
    swept
}
