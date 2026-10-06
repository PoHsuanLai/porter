//! Adopting an app's legacy account (`Manager.Adopt`): the daemon reads the app's old secret
//! items itself through a [`LegacyStore`], files them as porter secrets, and makes the account.
//! No credential crosses a transport. Which legacy service a caller may read is the host's
//! table (accountd's `[adopt]`), passed in as `service`; it is never taken from the request.

use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::audit::AuditEvent;
use porter_core::wire::{LegacyItem, LegacyRef, Refusal};
use porter_core::{
    Account, AccountId, AccountState, AccountsReply, AppId, CapabilityKind, Claim, Credential,
    Offer, Provenance, SecretKey, SecretPurpose, Subject,
};
use porter_provider::Provider;
use porter_secrets::Secrets;
use std::future::Future;
use std::pin::Pin;

/// Why a legacy item could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyFault {
    /// The legacy store cannot be reached or unlocked.
    Unavailable,
}

/// An app's old secret store, read-only. Object safe, so a host holds one as a trait object.
pub trait LegacyStore: Send + Sync {
    /// The credential filed under `entry` for the legacy `service`, read as porter's own form (the
    /// store's implementation converts what the old app wrote), or `None` when there is none.
    fn read<'a>(
        &'a self,
        service: &'a str,
        entry: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Credential>, LegacyFault>> + Send + 'a>>;
}

/// The entry name mailo filed `item` of `account` under: `<uuid>:incoming|outgoing|oauth|carddav`.
pub fn legacy_entry(account: &AccountId, item: LegacyItem) -> String {
    let word = match item {
        LegacyItem::Incoming => "incoming",
        LegacyItem::Outgoing => "outgoing",
        LegacyItem::OAuth => "oauth",
        LegacyItem::AddressBook => "carddav",
    };
    format!("{account}:{word}")
}

/// Which purposes the items file as: one password for both mail servers when they agree.
fn filed(items: Vec<(LegacyItem, Credential)>) -> Vec<(SecretPurpose, Credential)> {
    let find = |want: LegacyItem| items.iter().find(|(i, _)| *i == want).map(|(_, c)| c);
    let (incoming, outgoing) = (find(LegacyItem::Incoming), find(LegacyItem::Outgoing));
    let mut out = Vec::new();
    match (incoming, outgoing) {
        (Some(i), Some(o)) if i == o => out.push((SecretPurpose::Password, i.clone())),
        (Some(i), Some(o)) => {
            out.push((SecretPurpose::IncomingPassword, i.clone()));
            out.push((SecretPurpose::OutgoingPassword, o.clone()));
        }
        (Some(i), None) => out.push((SecretPurpose::Password, i.clone())),
        (None, Some(o)) => out.push((SecretPurpose::OutgoingPassword, o.clone())),
        (None, None) => {}
    }
    for (item, credential) in &items {
        match item {
            LegacyItem::OAuth => out.push((SecretPurpose::OAuthRefresh, credential.clone())),
            LegacyItem::AddressBook => out.push((
                SecretPurpose::ServicePassword(CapabilityKind::Contacts),
                credential.clone(),
            )),
            LegacyItem::Incoming | LegacyItem::Outgoing => {}
        }
    }
    out
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// What `handle` answers `Adopt` with: this service has no legacy store.
    pub(crate) async fn adopt(&self, _caller: &AppId, _legacy: LegacyRef) -> AccountsReply {
        AccountsReply::Refused(Refusal::Unavailable)
    }

    /// Reads `legacy`'s items from `store` under the legacy `service` the host's table gave the
    /// caller, files them, stores the account (its claims as its provider file declares them)
    /// and audits `Adopted`. An account already adopted answers again with its id.
    pub async fn adopt_from(
        &self,
        store: &dyn LegacyStore,
        service: &str,
        caller: &AppId,
        legacy: LegacyRef,
    ) -> AccountsReply {
        if self.lock().accounts.iter().any(|a| a.id == legacy.account) {
            return AccountsReply::Adopted(legacy.account);
        }
        let Some(spec) = self.catalog.get(&legacy.provider) else {
            return AccountsReply::Refused(Refusal::Unavailable);
        };
        let mut read = Vec::new();
        for item in &legacy.items {
            let entry = legacy_entry(&legacy.account, *item);
            match store.read(service, &entry).await {
                Ok(Some(credential)) => read.push((*item, credential)),
                // The app adopted this one itself (mailo E2): it is filed already, so there is
                // nothing left to read and nothing to file.
                Ok(None) if self.already_filed(&legacy.account, *item).await => {}
                Ok(None) | Err(LegacyFault::Unavailable) => {
                    return AccountsReply::Refused(Refusal::Unavailable);
                }
            }
        }
        let account = Account {
            id: legacy.account.clone(),
            provider: legacy.provider.clone(),
            label: legacy.label.clone(),
            state: AccountState::Ok,
            auth: spec.auth.kind,
            capabilities: spec
                .capabilities
                .iter()
                .map(|row| Claim {
                    subject: Subject::Account,
                    offer: Offer::Present(row.capability.clone()),
                    provenance: Provenance::Declared,
                })
                .collect(),
            restriction: porter_core::Restriction::none(),
            endpoints: legacy.endpoints.clone(),
        };
        let mut written = Vec::new();
        for (purpose, credential) in filed(read) {
            let key = SecretKey {
                account: account.id.clone(),
                purpose,
            };
            if self.secrets.put(&key, &credential).await.is_err() {
                // Undo only what this call wrote: what the app filed itself stays.
                for key in &written {
                    let _ = self.secrets.delete(key).await;
                }
                return AccountsReply::Refused(Refusal::Unavailable);
            }
            written.push(key);
        }
        let id = account.id.clone();
        self.lock().accounts.push(account);
        self.note(Some(caller.clone()), Some(id.clone()), AuditEvent::Adopted);
        match self.persist().await {
            Ok(()) => AccountsReply::Adopted(id),
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Whether the secret a legacy `item` held is filed under one of the purposes it becomes.
    async fn already_filed(&self, account: &AccountId, item: LegacyItem) -> bool {
        for purpose in filed_purposes(item) {
            let key = SecretKey {
                account: account.clone(),
                purpose,
            };
            if self.secrets.get(&key).await.is_ok() {
                return true;
            }
        }
        false
    }
}

/// The purposes a legacy item may be filed under: a password under its own purpose, or under
/// `Password` when the incoming and outgoing ones were the same.
fn filed_purposes(item: LegacyItem) -> Vec<SecretPurpose> {
    match item {
        LegacyItem::Incoming => vec![SecretPurpose::IncomingPassword, SecretPurpose::Password],
        LegacyItem::Outgoing => vec![SecretPurpose::OutgoingPassword, SecretPurpose::Password],
        LegacyItem::OAuth => vec![SecretPurpose::OAuthRefresh],
        LegacyItem::AddressBook => vec![SecretPurpose::ServicePassword(CapabilityKind::Contacts)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_named_as_mailo_named_them() {
        let id = AccountId::parse("67e55044-10b1-426f-9247-bb680e5fe0c8").expect("id");
        assert_eq!(
            legacy_entry(&id, LegacyItem::AddressBook),
            "67e55044-10b1-426f-9247-bb680e5fe0c8:carddav"
        );
    }

    #[test]
    fn equal_mail_passwords_file_as_one_and_different_ones_as_two() {
        let pw = |t: &str| Credential::Password(porter_core::SecretText::new(t));
        let same = filed(vec![
            (LegacyItem::Incoming, pw("a")),
            (LegacyItem::Outgoing, pw("a")),
        ]);
        assert_eq!(same, vec![(SecretPurpose::Password, pw("a"))]);
        let split = filed(vec![
            (LegacyItem::Incoming, pw("a")),
            (LegacyItem::Outgoing, pw("b")),
            (LegacyItem::AddressBook, pw("c")),
        ]);
        assert_eq!(split.len(), 3);
        assert_eq!(split[0].0, SecretPurpose::IncomingPassword);
        assert_eq!(
            split[2].0,
            SecretPurpose::ServicePassword(CapabilityKind::Contacts)
        );
    }
}
