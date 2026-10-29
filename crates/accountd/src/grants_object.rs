//! `org.quire.Grants1` at `/org/quire/Grants1`: who may use which account, and the relays a grant
//! opens, over the service's grants domain.
//!
//! - An app and the porter daemons use it, for their own grants: the caller is the connection's,
//!   and a grant is the caller's app's, never a name the request sends. Every other role (the
//!   shell, Settings, the agent, the agent launcher, the AI broker, the terminal, the compositor,
//!   and any role a newer porter adds) is refused `AccessDenied`, as a sender accountd does not
//!   know is.
//! - `RequestGrant` returns a Request object like `Keys.RequestKey` and answers on it with the
//!   candidate the person granted. The sheet is drawn every time it is asked for: an app that
//!   holds a grant already reads it with `Candidates` and asks only for another account, so the
//!   consent sheet shows once for an account and its answer is kept.
//! - `OpenAuthenticated` and `OpenLinked` check the grant and the endpoint (the service), then
//!   hand the app one end of a socket pair whose other end the relay holds (`relay`). The
//!   credential in the relay's plan is in no reply.
//! - `Accounts` lists every account to a porter daemon, Settings or the shell, with what accountd
//!   calls the provider; an app is refused. `ExtendAccess` and `AsksOnUse` are the sync daemon's
//!   "Show in Files": the first waits for the person, in a sign-in of its own.

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::request::departed;
use crate::state::{Core, Standing, window};
use porter_callers::{Caller, CallerRole};
use porter_capability::CapabilityKind;
use porter_core::consent::Usage;
use porter_core::{AccountId, DataClass, EndpointUrl, GrantId, ProviderId};
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::accounts::{
    ACCOUNT_OPTION, CandidateArg, GrantsServer, candidate_to_dbus, from_text, to_text,
};
use porter_dbus::{Call, Details};
use porter_ids::word::word;
use porter_match::Need;
use porter_wire::{AccountsRequest, Refusal};
use std::sync::Arc;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

/// The Grants object.
#[derive(Debug)]
pub(crate) struct GrantsObject<H, C>(Arc<Core<H, C>>);

impl<H, C> GrantsObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RefusedError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| RefusedError::invalid(format!("`{text}`: {e}")))
}

fn need_of(text: &str) -> Result<Need, RefusedError> {
    from_text(text).map_err(RefusedError::invalid)
}

fn grant_of(text: &str) -> Result<GrantId, RefusedError> {
    GrantId::parse(text).map_err(RefusedError::invalid)
}

fn endpoint_of(text: &str) -> Result<EndpointUrl, RefusedError> {
    EndpointUrl::parse(text).map_err(RefusedError::invalid)
}

impl<H: Host, C: Callers> GrantsObject<H, C> {
    /// The caller, which must be an app the person uses or a porter daemon.
    async fn user(&self, header: &Header<'_>) -> Result<Caller, RefusedError> {
        let caller = self.0.identify(header, Standing::Any).await?;
        match caller.role {
            CallerRole::App | CallerRole::PorterDaemon => Ok(caller),
            _ => Err(RefusedError::access_denied(
                "only an app the person uses may ask about its grants",
            )),
        }
    }

    /// The caller, which must be a porter daemon, Settings or the shell.
    async fn reader(&self, header: &Header<'_>) -> Result<Caller, RefusedError> {
        let caller = self.0.identify(header, Standing::Any).await?;
        match caller.role {
            CallerRole::PorterDaemon | CallerRole::Settings | CallerRole::SheetHost => Ok(caller),
            _ => Err(RefusedError::access_denied(
                "only a porter daemon, Settings or the shell may list the accounts",
            )),
        }
    }
}

impl<H: Host, C: Callers> GrantsServer for GrantsObject<H, C> {
    type Error = RefusedError;

    async fn candidates(
        &self,
        Call { header, .. }: Call<'_>,
        need: String,
        class: String,
        usage: String,
    ) -> Result<Vec<CandidateArg>, RefusedError> {
        let caller = self.user(&header).await?;
        let (need, class, usage) = (need_of(&need)?, slug::<DataClass>(&class)?, slug(&usage)?);
        Ok(self
            .0
            .host
            .candidates(&caller.app, &need, class, usage)
            .iter()
            .map(candidate_to_dbus)
            .collect())
    }

    async fn availability(
        &self,
        Call { header, .. }: Call<'_>,
        need: String,
        class: String,
        usage: String,
    ) -> Result<String, RefusedError> {
        let caller = self.user(&header).await?;
        let (need, class, usage) = (need_of(&need)?, slug::<DataClass>(&class)?, slug(&usage)?);
        let availability = self.0.host.availability(&caller.app, &need, class, usage);
        Ok(word(&availability).to_owned())
    }

    async fn grants(&self, Call { header, .. }: Call<'_>) -> Result<String, RefusedError> {
        let caller = self.user(&header).await?;
        Ok(to_text(&self.0.host.grants_of(&caller.app)))
    }

    async fn request_grant(
        &self,
        Call { header, .. }: Call<'_>,
        need: String,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        self.user(&header).await?;
        let account = match options.get(ACCOUNT_OPTION) {
            None => None,
            Some(value) => {
                let text: &str = value.downcast_ref().map_err(RefusedError::invalid)?;
                Some(AccountId::parse(text).map_err(RefusedError::invalid)?)
            }
        };
        let request = AccountsRequest::Choose {
            need: need_of(&need)?,
            class: slug::<DataClass>(&class)?,
            usage: slug::<Usage>(&usage)?,
            window: window(&parent_window),
            account,
        };
        self.0
            .sheet_anew(&header, SheetKind::Choose, &options, request)
            .await
    }

    async fn revoke(
        &self,
        Call { header, .. }: Call<'_>,
        grant: String,
    ) -> Result<(), RefusedError> {
        let caller = self.user(&header).await?;
        self.0
            .host
            .revoke_own(&caller.app, &grant_of(&grant)?)
            .await
            .map_err(RefusedError::of)?;
        self.0.publish().await;
        Ok(())
    }

    async fn open_authenticated(
        &self,
        Call { header, .. }: Call<'_>,
        grant: String,
        endpoint: String,
    ) -> Result<OwnedFd, RefusedError> {
        let caller = self.user(&header).await?;
        let plan = self
            .0
            .host
            .open_authenticated(&caller.app, &grant_of(&grant)?, &endpoint_of(&endpoint)?)
            .await
            .map_err(RefusedError::of)?;
        let fd = self.0.relays.open(plan).map_err(RefusedError::failed)?;
        Ok(OwnedFd::from(fd))
    }

    async fn open_linked(
        &self,
        Call { header, .. }: Call<'_>,
        grant: String,
        origin: String,
    ) -> Result<OwnedFd, RefusedError> {
        let caller = self.user(&header).await?;
        let plan = self
            .0
            .host
            .open_linked(&caller.app, &grant_of(&grant)?, &endpoint_of(&origin)?)
            .await
            .map_err(RefusedError::of)?;
        let fd = self.0.relays.open(plan).map_err(RefusedError::failed)?;
        Ok(OwnedFd::from(fd))
    }

    async fn accounts(&self, Call { header, .. }: Call<'_>) -> Result<String, RefusedError> {
        self.reader(&header).await?;
        let rows: Vec<_> = self
            .0
            .host
            .registry()
            .accounts
            .into_iter()
            .map(|account| {
                let label = self
                    .0
                    .provider_names
                    .label_of(&account.provider)
                    .unwrap_or_default()
                    .to_owned();
                (account, label)
            })
            .collect();
        Ok(to_text(&rows))
    }

    async fn extend_access(
        &self,
        Call { header, .. }: Call<'_>,
        account: String,
        kinds: Vec<String>,
        parent_window: String,
    ) -> Result<(), RefusedError> {
        let caller = self.user(&header).await?;
        let account = AccountId::parse(&account).map_err(RefusedError::invalid)?;
        let kinds = kinds
            .iter()
            .map(|kind| slug::<CapabilityKind>(kind))
            .collect::<Result<Vec<_>, _>>()?;
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_owned();
        // One sign-in of this kind at a time, as for `Reauthenticate`; held until it ends.
        let _claim = self
            .0
            .open_sheets
            .claim(&caller.app, SheetKind::Reauthenticate)?;
        let leaving = DBusProxy::new(&self.0.connection)
            .await
            .map_err(RefusedError::failed)?
            .receive_name_owner_changed_with_args(&[(0, sender.as_str())])
            .await
            .map_err(RefusedError::failed)?;
        let window = window(&parent_window);
        let ended = tokio::select! {
            ended = self.0.host.extend_access(&caller.app, &account, kinds, window) => ended,
            // The app left: the sign-in is taken down with the dropped call, and nobody is told.
            () = departed(leaving) => Err(Refusal::Dismissed),
        };
        self.0.publish().await;
        ended.map_err(RefusedError::of)
    }

    async fn asks_on_use(
        &self,
        Call { header, .. }: Call<'_>,
        provider: String,
        kind: String,
    ) -> Result<bool, RefusedError> {
        self.user(&header).await?;
        let provider = ProviderId::parse(&provider).map_err(RefusedError::invalid)?;
        Ok(self.0.host.asks_on_use(&provider, slug(&kind)?))
    }
}
