//! Editing an existing account's servers through the sheet (`Purpose::EditServers`): the host
//! half. The sheet opens on the server form of the account's protocol, prefilled; the servers the
//! person sends go to the provider's sign-in, begun as `SignInMode::EditServers`, which logs in
//! at them; only when it has does the sheet end, and what it ended with is handed back here.
//! Nothing is written by this call: the caller stores what it gets (mailo keeps its accounts in
//! its own store), and keeps the account as it was when it gets a refusal.

use crate::add_flow::{Job, Stored};
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::sheet::{EditedServers, Manual, ServersFixed, servers_fixed};
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AccountLabel, AppId, ProviderId, SecretKey, SecretPurpose, ServiceEndpoint,
};
use porter_provider::Provider;
use porter_secrets::Secrets;

/// An account whose servers are to be edited, as the caller has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditServers {
    /// The account. Its stored secrets, if this service keeps any, are tried when the person
    /// leaves the password empty.
    pub account: AccountId,
    /// Its provider: a loaded provider file whose sign-in is a password or an app password.
    pub provider: ProviderId,
    /// Its address: the login name when the form leaves that empty.
    pub label: AccountLabel,
    /// Its servers as they are; the form is prefilled from them.
    pub current: Manual,
    /// The name shown in From, as it is.
    pub name: Option<String>,
}

/// What an edit of an account's servers came to: what the person typed, tried and passed, and
/// the servers as the sign-in built them. `Debug` redacts the password and any token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServersEdited {
    /// The servers, the name shown in From and a new password (`None` keeps the stored one).
    pub edited: EditedServers,
    /// The servers as endpoints, logged in as the account: what porter's own registry keeps.
    pub endpoints: Vec<ServiceEndpoint>,
}

/// Why an edit kept nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditRefused {
    /// The account's servers are not the person's to type: the host shows them read-only.
    Fixed(ServersFixed),
    /// The sheet ended without new servers (closed, failed), or no provider by that id is
    /// loaded.
    Refused(Refusal),
}

/// The secrets an edit may try in place of a password left empty.
const KEPT: [SecretPurpose; 3] = [
    SecretPurpose::Password,
    SecretPurpose::IncomingPassword,
    SecretPurpose::OutgoingPassword,
];

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Opens the sheet that edits `edit`'s servers and drives it to its end. An account whose
    /// provider signs in at its own page is refused before any sheet opens
    /// (`EditRefused::Fixed`). Nothing is stored: on `Ok` the caller stores what came back.
    pub async fn edit_servers(
        &self,
        caller: &AppId,
        edit: EditServers,
        window: ParentWindow,
    ) -> Result<ServersEdited, EditRefused> {
        let auth = self
            .providers
            .iter()
            .find(|p| p.spec().id == edit.provider)
            .map(|p| p.spec().auth.kind)
            .ok_or(EditRefused::Refused(Refusal::Unavailable))?;
        if let Some(fixed) = servers_fixed(auth) {
            return Err(EditRefused::Fixed(fixed));
        }
        let mut credentials = Vec::new();
        for purpose in KEPT {
            let key = SecretKey {
                account: edit.account.clone(),
                purpose,
            };
            if let Ok(credential) = self.secrets.get(&key).await {
                credentials.push((purpose, credential));
            }
        }
        let job = Job::EditServers {
            account: edit.account,
            provider: edit.provider,
            label: edit.label,
            current: edit.current,
            name: edit.name,
            credentials,
        };
        match self.drive(caller, window, &job).await {
            Ok(Stored::Edited(edited, endpoints)) => Ok(ServersEdited { edited, endpoints }),
            Ok(_) => Err(EditRefused::Refused(Refusal::Unavailable)),
            Err(refusal) => Err(EditRefused::Refused(refusal)),
        }
    }
}
