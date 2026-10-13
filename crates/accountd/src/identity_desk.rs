//! `Identities1`'s two ways out of accountd, as a seam of its own: the sheet that lets the person
//! pick an identity ("Continue as"), and the read of an identity's password for the one that
//! picked it.
//!
//! The desk is not a method of the service because the service has neither: the picker is the
//! same sheet host that draws `Choose` (`Sheets::consent`, with `AskPurpose::ContinueAs`), and
//! the password is read with the same `Secrets` the service files it with. An entry of the audit
//! file says which app was handed which identity's password, never the password.

use porter_core::audit::{AuditEntry, AuditEvent};
use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{AccountId, AppId, Credential, SecretKey, SecretPurpose, SecretText};
use porter_secrets::{Secrets, SecretsError};
use porter_service::{AuditSink, Clock, Sheets};
use std::future::Future;
use std::pin::Pin;

/// A boxed future, so the desk can be used as a trait object.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Shows the identity picker and reads a picked identity's password.
pub trait IdentityDesk: std::fmt::Debug + Send + Sync + 'static {
    /// Shows `ask` (its accounts are the identities on offer) and answers what the person did.
    /// A host that is not there answers `Dismissed`.
    fn pick<'a>(&'a self, ask: ConsentAsk, window: &'a ParentWindow) -> Boxed<'a, ConsentAnswer>;

    /// The password or app password filed for `account`. A password that is gone or unreadable
    /// means signing in again.
    fn password<'a>(&'a self, account: &'a AccountId) -> Boxed<'a, Result<SecretText, Refusal>>;

    /// Records that the password of `account` was handed to `app`.
    fn note_password(&self, app: &AppId, account: &AccountId);
}

/// The desk over the sheet host, the secret store, an audit sink and a clock.
#[derive(Debug, Clone)]
pub struct SheetsDesk<U, S, A, K> {
    sheets: U,
    secrets: S,
    audit: A,
    clock: K,
}

impl<U, S, A, K> SheetsDesk<U, S, A, K> {
    /// A desk drawing on `sheets`, reading from `secrets` and recording to `audit`, dated by
    /// `clock`.
    pub fn new(sheets: U, secrets: S, audit: A, clock: K) -> Self {
        Self {
            sheets,
            secrets,
            audit,
            clock,
        }
    }
}

/// What the store's failure is to the app that asked.
fn refusal_of(error: SecretsError) -> Refusal {
    match error {
        SecretsError::Missing | SecretsError::Unreadable => Refusal::NeedsReauth,
        SecretsError::Locked | SecretsError::Unavailable => Refusal::Unavailable,
    }
}

impl<U, S, A, K> IdentityDesk for SheetsDesk<U, S, A, K>
where
    U: Sheets + std::fmt::Debug + 'static,
    S: Secrets + std::fmt::Debug + 'static,
    A: AuditSink + std::fmt::Debug + 'static,
    K: Clock + std::fmt::Debug + 'static,
{
    fn pick<'a>(&'a self, ask: ConsentAsk, window: &'a ParentWindow) -> Boxed<'a, ConsentAnswer> {
        Box::pin(async move { self.sheets.consent(ask, window).await })
    }

    fn password<'a>(&'a self, account: &'a AccountId) -> Boxed<'a, Result<SecretText, Refusal>> {
        Box::pin(async move {
            let key = SecretKey {
                account: account.clone(),
                purpose: SecretPurpose::Password,
            };
            match self.secrets.get(&key).await.map_err(refusal_of)? {
                Credential::Password(password) => Ok(password),
                _ => Err(Refusal::NeedsReauth),
            }
        })
    }

    fn note_password(&self, app: &AppId, account: &AccountId) {
        self.audit.record(AuditEntry::new(
            self.clock.now(),
            Some(app.clone()),
            Some(account.clone()),
            AuditEvent::PasswordHandedOver,
        ));
    }
}
