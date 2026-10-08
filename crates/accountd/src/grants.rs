//! `Grants` and `Tokens`: the methods that answer at once.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing};
use crate::errors::RefusedError;
use porter_core::wire::Refusal;
use porter_core::{AccountsReply, AccountsRequest, Audience, EndpointUrl, GrantId};
use porter_dbus::{Details, TokenArg, grant_to_dbus, token_to_dbus};
use std::sync::Arc;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

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
    /// Role check first: an `Agent` is refused `Denied`, an unknown sender `AccessDenied`. The
    /// service checks the grant and that `endpoint` is one the account holds for it, and reads
    /// what the relay presents; that goes to the relay task and nowhere else. The descriptor is
    /// the app's end of a socketpair and is returned once the relay has authenticated.
    async fn open_authenticated(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
        endpoint: String,
    ) -> Result<zbus::zvariant::OwnedFd, RefusedError> {
        let app = self.0.acting(&header).await?;
        let grant = GrantId::parse(&grant).map_err(RefusedError::invalid)?;
        let endpoint = EndpointUrl::parse(&endpoint).map_err(RefusedError::invalid)?;
        // Read before the plan: a `Once` grant is spent by the open itself.
        let account = self.0.account_of_grant(&grant);
        let planned = self.0.host.open_relay(&app, &grant, &endpoint).await;
        // A refused refresh, or a spent `Once` grant, changed the registry: tell the clients.
        self.0.publish().await;
        let plan = planned.map_err(RefusedError::of)?;
        match self.0.relays.open(plan).await {
            Ok(fd) => Ok(fd),
            Err(refusal) => {
                // The server refused the credential the account holds: say so, as the signals
                // and the Settings module show it.
                if refusal == Refusal::NeedsReauth
                    && let Some(account) = account
                {
                    self.0.needs_reauth(&account).await;
                }
                Err(RefusedError::of(refusal))
            }
        }
    }

    /// A socket to a relay that adds no credential, to an origin the account's provider file
    /// declares for the grant's kind (`OpenLinked`: Graph's `uploadUrl` and `downloadUrl` hosts).
    ///
    /// The same role check as `OpenAuthenticated`. The service refuses (`EndpointNotGranted`)
    /// any origin the file does not declare, so the daemon is never an open proxy; the relay
    /// dials that origin only and drops any `Authorization` the app writes.
    async fn open_linked(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
        origin: String,
    ) -> Result<zbus::zvariant::OwnedFd, RefusedError> {
        let app = self.0.acting(&header).await?;
        let grant = GrantId::parse(&grant).map_err(RefusedError::invalid)?;
        let origin = EndpointUrl::parse(&origin).map_err(RefusedError::invalid)?;
        let planned = self.0.host.open_linked_relay(&app, &grant, &origin).await;
        self.0.publish().await;
        let plan = planned.map_err(RefusedError::of)?;
        self.0.relays.open(plan).await.map_err(RefusedError::of)
    }

    /// An API key for a process the agent launcher spawns (P2), on a sealed memfd or in a 0600
    /// file: the credential's id and the handle in a variant (`h` or `s`). `AgentLauncher` only,
    /// for a program it registered, under an `Always` grant of `org.quire.Agent.<program>`. See
    /// `handoff`.
    async fn issue_process_credential(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
        program: String,
        target: String,
    ) -> Result<(String, OwnedValue), RefusedError> {
        self.0
            .issue_process_credential(&header, &grant, &program, &target)
            .await
    }

    /// The launcher says the process is over; accountd unlinks the file and forgets the
    /// credential.
    async fn revoke_process_credential(
        &self,
        #[zbus(header)] header: Header<'_>,
        id: String,
    ) -> Result<(), RefusedError> {
        self.0.revoke_process_credential(&header, &id).await
    }

    /// Sent to the launcher that was issued the credential, alone, when accountd ended it.
    #[zbus(signal)]
    async fn process_credential_revoked(
        emitter: &SignalEmitter<'_>,
        id: &str,
        reason: &str,
    ) -> zbus::Result<()>;
}
