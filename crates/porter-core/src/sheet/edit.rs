//! Editing an existing account's servers through the sheet: who may, and what a passed check
//! hands the host.
//!
//! Only an account whose sign-in is a password the person typed (a password, an app password,
//! or a JMAP token given in place of one) has servers a person may retype. An account that
//! signs in at its provider's page holds a token that must go to the provider's own servers
//! and nowhere else, so its servers are shown and not edited.
//!
//! The servers typed are tried before anything is kept: the sheet ends with [`EditedServers`]
//! only after the sign-in has logged in at them, and the host stores what it carries then.

use super::manual::Manual;
use crate::auth_kind::AuthKind;
use crate::secret::SecretText;

/// Why an account's servers cannot be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServersFixed {
    /// It signs in at its provider's page (OAuth, a browser sign-in): the token it holds goes
    /// to the provider's servers only.
    ProviderSignIn,
    /// It has no servers a person types (an API key, a program on this computer, an agent, a
    /// runtime, a key pair).
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
        AuthKind::None
        | AuthKind::LocalRuntime
        | AuthKind::LocalBridge
        | AuthKind::KeyPair
        | AuthKind::ApiKey
        | AuthKind::AgentLogin
        | AuthKind::OwnProgram => Some(ServersFixed::NotTyped),
    }
}

/// What an edit of an account's servers came to, once the sign-in logged in at them. `Debug`
/// redacts the password and any token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditedServers {
    /// The servers, as typed and tried. A JMAP token typed in place of the password is in it.
    pub servers: Manual,
    /// The name shown in From; `None` when the field was left empty.
    pub name: Option<String>,
    /// A new password, when one was typed; `None` keeps the stored one.
    pub password: Option<SecretText>,
}
