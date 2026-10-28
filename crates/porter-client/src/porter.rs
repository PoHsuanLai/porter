//! `Porter`: the app's handle on the account service, over a transport.

use crate::accounts::Accounts;
use crate::ai_accounts::AiAccounts;
use crate::ai_keys::AiKeys;
use crate::grants::Grants;
use crate::identities::Identities;
use crate::link::{
    AccountsLink, AiAccountsLink, AiKeysLink, GrantsLink, IdentitiesLink, SpacesLink, TailnetLink,
};
use crate::spaces::Spaces;
use crate::tailnet::Tailnet;
use std::sync::Arc;

/// The app's handle. `T` is the transport: the bus (`porter-client-dbus`) or the service hosted in
/// process (`porter-client-inproc`). A domain accessor exists only over a transport that carries
/// the domain, so a call a transport cannot serve does not compile.
#[derive(Debug)]
pub struct Porter<T> {
    link: Arc<T>,
}

impl<T> Clone for Porter<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T> Porter<T> {
    /// Over a transport the app built.
    pub fn over(link: T) -> Self {
        Self {
            link: Arc::new(link),
        }
    }
}

impl<T: AccountsLink> Porter<T> {
    /// Signing an account in.
    pub fn accounts(&self) -> Accounts<T> {
        Accounts {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: GrantsLink> Porter<T> {
    /// Who may use which account, and the relays a grant opens.
    pub fn grants(&self) -> Grants<T> {
        Grants {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: IdentitiesLink> Porter<T> {
    /// The identities signed in on this machine ("Continue as").
    pub fn identities(&self) -> Identities<T> {
        Identities {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: SpacesLink> Porter<T> {
    /// The desktop-wide Spaces.
    pub fn spaces(&self) -> Spaces<T> {
        Spaces {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: TailnetLink> Porter<T> {
    /// The person's computers on their Tailscale network.
    pub fn tailnet(&self) -> Tailnet<T> {
        Tailnet {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: AiKeysLink> Porter<T> {
    /// An app's permit to use the person's AI keys.
    pub fn ai_keys(&self) -> AiKeys<T> {
        AiKeys {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: AiAccountsLink> Porter<T> {
    /// What the AI broker asks of the daemon that keeps the AI keys.
    pub fn ai_accounts(&self) -> AiAccounts<T> {
        AiAccounts {
            link: Arc::clone(&self.link),
        }
    }
}
