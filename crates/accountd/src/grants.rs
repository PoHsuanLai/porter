//! `Grants` and `Tokens`: the methods that answer at once.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing};
use crate::errors::RefusedError;
use porter_core::{AccountsReply, AccountsRequest, Audience, GrantId};
use porter_dbus::{Details, TokenArg, grant_to_dbus, token_to_dbus};
use std::sync::Arc;
use zbus::message::Header;

/// `org.quire.Accounts1.Grants`.
#[derive(Debug)]
pub(crate) struct Grants<H, C>(Arc<Core<H, C>>);

impl<H, C> Grants<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// `org.quire.Accounts1.Tokens`.
#[derive(Debug)]
pub(crate) struct Tokens<H, C>(Arc<Core<H, C>>);

impl<H, C> Tokens<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

fn mismatched(reply: AccountsReply) -> RefusedError {
    RefusedError::failed(format!("the service answered another request: {reply:?}"))
}

#[zbus::interface(name = "org.quire.Accounts1.Grants")]
impl<H: Host, C: Callers> Grants<H, C> {
    async fn list(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<(String, Details)>, RefusedError> {
        match self
            .0
            .answer(&header, Standing::Any, AccountsRequest::ListGrants)
            .await?
        {
            AccountsReply::Grants(list) => Ok(list.iter().map(grant_to_dbus).collect()),
            other => Err(mismatched(other)),
        }
    }

    async fn revoke(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
    ) -> Result<(), RefusedError> {
        let grant = GrantId::parse(&grant).map_err(RefusedError::invalid)?;
        match self
            .0
            .answer(&header, Standing::Any, AccountsRequest::Revoke { grant })
            .await?
        {
            AccountsReply::Revoked => Ok(()),
            other => Err(mismatched(other)),
        }
    }
}

#[zbus::interface(name = "org.quire.Accounts1.Tokens")]
impl<H: Host, C: Callers> Tokens<H, C> {
    async fn issue_token(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
        audience: String,
    ) -> Result<TokenArg, RefusedError> {
        let request = AccountsRequest::IssueToken {
            grant: GrantId::parse(&grant).map_err(RefusedError::invalid)?,
            audience: Audience(audience),
        };
        match self.0.answer(&header, Standing::Acting, request).await? {
            AccountsReply::Token(token) => Ok(token_to_dbus(&token)),
            other => Err(mismatched(other)),
        }
    }

    /// A socket to a daemon-side authenticated relay (`OpenAuthenticated`).
    ///
    /// Role check done here: an `Agent` is refused `Denied`, an unknown sender `AccessDenied`.
    ///
    /// W3G SEAM: the serving path is W3g's. It checks the grant and the endpoint
    /// (`AccountService::open_authenticated`), makes the descriptor pair, runs the relay
    /// (`porter_proxy::relay`) and returns the client's end. Until then this answers
    /// `Unavailable` without touching the service (`relay_plan` is still a stub there).
    async fn open_authenticated(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
        endpoint: String,
    ) -> Result<zbus::zvariant::OwnedFd, RefusedError> {
        let _app = self.0.acting(&header).await?;
        let _ = (grant, endpoint);
        Err(RefusedError::of(porter_core::wire::Refusal::Unavailable))
    }
}
