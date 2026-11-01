//! Editing an existing account's servers through the sheet: who may, what is asked, and what a
//! passed check hands the host.
//!
//! Only an account whose sign-in is a password the person typed (a password or an app password)
//! has servers a person may retype. An account that signs in at its provider's page holds a
//! token that must go to the provider's own servers and nowhere else, so its servers are shown
//! and not edited ([`servers_fixed`]).
//!
//! The servers typed are tried before anything is kept: the sheet ends only after the sign-in has
//! logged in at them, and the host's own store step hands back an [`EditedServers`]. Porter
//! stores nothing of an edit; the app that asked keeps what it carries.

use crate::fields::{FieldAnswer, FieldKind, FieldValue};
use crate::manual::{Manual, display_name, parse_manual};
use crate::view::FieldProblem;
use porter_core::{AccountId, AccountLabel, AuthKind, ProviderId, SecretText, ServiceEndpoint};

/// Why an account's servers cannot be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServersFixed {
    /// It signs in at its provider's page (OAuth, a browser sign-in): the token it holds goes
    /// to the provider's servers only.
    ProviderSignIn,
    /// It has no servers a person types (an API key, a program on this computer, an agent, a
    /// key pair).
    NotTyped,
}

/// Whether an account that signs in by `auth` may have its servers edited: a password or an app
/// password only. A host asks this before it offers to edit, and shows the servers read-only
/// otherwise.
pub fn servers_editable(auth: AuthKind) -> bool {
    servers_fixed(auth).is_none()
}

/// Why an account that signs in by `auth` may not have its servers edited, or `None` when it
/// may.
pub fn servers_fixed(auth: AuthKind) -> Option<ServersFixed> {
    match auth {
        AuthKind::Password | AuthKind::AppPassword => None,
        AuthKind::OAuthPkce
        | AuthKind::OAuthMintsKey
        | AuthKind::OAuthPlan
        | AuthKind::LoginFlowV2
        | AuthKind::CloudIdentity => Some(ServersFixed::ProviderSignIn),
        // None, a runtime or a bridge on this computer, a key pair, an API key, an agent, a
        // program that signs itself in; and a way a newer porter adds, which is not typed until
        // this says so.
        _ => Some(ServersFixed::NotTyped),
    }
}

/// What an app asks when it edits an account's servers: which account, what its servers are now
/// (the form's prefill), whether to ask for the name shown in From, and the password it holds,
/// when it wants an empty password field to mean "keep it".
///
/// The password is private and `Debug` redacts it; it leaves by [`EditServers::into_parts`],
/// once.
#[derive(Debug)]
#[non_exhaustive]
pub struct EditServers {
    account: AccountId,
    provider: ProviderId,
    label: AccountLabel,
    current: Manual,
    name: Option<String>,
    keep: Option<SecretText>,
}

impl EditServers {
    /// An edit of `account`, of `provider`, called `label` (the login name when the form leaves
    /// it empty), whose servers are `current`. `current` holds no token: the form never
    /// prefills a secret.
    pub fn new(
        account: AccountId,
        provider: ProviderId,
        label: AccountLabel,
        current: Manual,
    ) -> Self {
        Self {
            account,
            provider,
            label,
            current,
            name: None,
            keep: None,
        }
    }

    /// The same edit that also asks for the name shown in From, prefilled with `name` (empty to
    /// prefill nothing). Without it the form has no name field: the name is the app's to ask.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The same edit that tries `password`, the one the app holds, when the person types none.
    #[must_use]
    pub fn keeping(mut self, password: SecretText) -> Self {
        self.keep = Some(password);
        self
    }

    /// The account.
    pub fn account(&self) -> &AccountId {
        &self.account
    }

    /// Its provider.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Its name, the login name when the form leaves it empty.
    pub fn label(&self) -> &AccountLabel {
        &self.label
    }

    /// Its servers as they are.
    pub fn current(&self) -> &Manual {
        &self.current
    }

    /// The name shown in From to prefill, when the form asks for one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The edit taken apart: the account, its provider, its label, its servers, the name to ask
    /// for and the password held. The password is moved out, not copied.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        AccountId,
        ProviderId,
        AccountLabel,
        Manual,
        Option<String>,
        Option<SecretText>,
    ) {
        (
            self.account,
            self.provider,
            self.label,
            self.current,
            self.name,
            self.keep,
        )
    }
}

/// What an edit of an account's servers came to, once the sign-in logged in at them. There is
/// no `Clone` and no `PartialEq`, and the password has no accessor: it leaves by
/// [`EditedServers::into_parts`], once. `Debug` redacts it and any token.
#[derive(Debug)]
#[non_exhaustive]
pub struct EditedServers {
    servers: Manual,
    endpoints: Vec<ServiceEndpoint>,
    name: Option<String>,
    password: Option<SecretText>,
}

impl EditedServers {
    /// What `answers` to the edit form came to, the sign-in having logged in at `endpoints`.
    ///
    /// # Errors
    /// The field that does not read: the sheet's machine has checked the same before it sent
    /// the answers, so this is a form that was not the one asked.
    pub fn from_answers(
        answers: &[FieldAnswer],
        endpoints: Vec<ServiceEndpoint>,
    ) -> Result<Self, FieldProblem> {
        let servers = parse_manual(answers)?;
        let name = display_name(answers)?;
        let password = [FieldKind::AppPassword, FieldKind::Password]
            .into_iter()
            .find_map(|kind| typed_secret(answers, kind));
        Ok(Self {
            servers,
            endpoints,
            name,
            password,
        })
    }

    /// The servers, as typed and tried. A JMAP token typed in place of the password is in it.
    pub fn servers(&self) -> &Manual {
        &self.servers
    }

    /// The servers the sign-in logged in at, as the account will hold them.
    pub fn endpoints(&self) -> &[ServiceEndpoint] {
        &self.endpoints
    }

    /// The name shown in From; `None` when the field was left empty or not asked.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether a new password was typed: when not, the one the account has stays.
    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    /// The edit taken apart: the servers, the name and the new password. The password is moved
    /// out, not copied.
    #[must_use]
    pub fn into_parts(self) -> (Manual, Option<String>, Option<SecretText>) {
        (self.servers, self.name, self.password)
    }
}

/// The secret typed for `kind`, exactly as typed, when it is not empty.
fn typed_secret(answers: &[FieldAnswer], kind: FieldKind) -> Option<SecretText> {
    answers
        .iter()
        .find(|a| a.kind == kind)
        .map(|a| match &a.value {
            FieldValue::Secret(secret) => secret.clone(),
            FieldValue::Plain(text) => SecretText::new(text.clone()),
        })
        .filter(|secret| !secret.expose().is_empty())
}

#[cfg(test)]
mod tests;
