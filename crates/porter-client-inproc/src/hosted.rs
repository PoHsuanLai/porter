//! [`Hosted`]: `porter-client`'s accounts and grants answered by a service this process runs.

use crate::relays::{NoRelays, RelayHost};
use porter_capability::CapabilityKind;
use porter_client::{
    AccountsError, AccountsLink, AuthenticatedStream, ConsentOffer, EditError, GrantsError,
    GrantsLink, ListedAccount, Porter, ServerEdits,
};
use porter_core::RelayPlan;
use porter_core::consent::{Availability, Grant, Usage};
use porter_core::store::{NoStore, Persisted, RegistryStore};
use porter_core::stream::duplex;
use porter_core::{
    AccountId, AppId, Candidate, DataClass, EndpointUrl, GrantId, ProviderHint, ProviderId,
};
use porter_ids::clock::{Clock, SystemClock};
use porter_match::Need;
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, Service};
use porter_service_accounts::AccountsApi;
use porter_service_core::registry::Registry;
use porter_service_grants::GrantsApi;
use porter_sheet::host::Sheets;
use porter_sheet::{EditServers, EditedServers};
use porter_wire::{AccountsReply, ParentWindow, Refusal};
use std::sync::Arc;

/// How much of a relay's traffic an in-memory stream holds before the writer waits.
const RELAY_BUFFER: usize = 64 * 1024;

/// The service, hosted in this process, answering for one app. `C` is the service (porter's
/// `AccountService<..>`, a [`Service`]); the relay host defaults to none.
#[derive(Debug)]
pub struct Hosted<C, H = NoRelays> {
    service: Arc<C>,
    app: AppId,
    relays: H,
}

impl<C> Hosted<C, NoRelays> {
    /// `app`'s link to a service this process hosts, with no relays.
    pub fn new(service: Arc<C>, app: AppId) -> Self {
        Self {
            service,
            app,
            relays: NoRelays,
        }
    }
}

impl<C, H> Hosted<C, H> {
    /// The same link with `relays` running its authenticated relays (`PorterRelays`, feature
    /// `relays`, or a host the app writes). Without one, opening an authenticated stream says
    /// nobody is reachable.
    #[must_use]
    pub fn with_relays<N: RelayHost>(self, relays: N) -> Hosted<C, N> {
        Hosted {
            service: self.service,
            app: self.app,
            relays,
        }
    }
}

impl<C: Service + 'static, H: RelayHost + 'static> AccountsLink for Hosted<C, H> {
    async fn add(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> Result<AccountId, AccountsError> {
        let accounts = self.service.accounts_api();
        match accounts
            .add_account(&self.app, hint.clone(), window.clone())
            .await
        {
            AccountsReply::Added(account) => Ok(account),
            AccountsReply::AlreadyAdded(account) => Err(AccountsError::AlreadyAdded(account)),
            AccountsReply::Refused(refusal) => Err(AccountsError::Refused(refusal)),
            // a reply a newer porter adds
            _ => Err(AccountsError::Refused(Refusal::Unavailable)),
        }
    }

    async fn reauthenticate(
        &self,
        account: &AccountId,
        window: &ParentWindow,
    ) -> Result<(), AccountsError> {
        let accounts = self.service.accounts_api();
        match accounts
            .reauthenticate(&self.app, account, window.clone())
            .await
        {
            AccountsReply::Reauthenticated => Ok(()),
            AccountsReply::Refused(refusal) => Err(AccountsError::Refused(refusal)),
            _ => Err(AccountsError::Refused(Refusal::Unavailable)),
        }
    }
}

impl<C: Service + 'static, H: RelayHost + 'static> ServerEdits for Hosted<C, H> {
    async fn edit_servers(
        &self,
        edit: EditServers,
        window: &ParentWindow,
    ) -> Result<EditedServers, EditError> {
        self.service
            .accounts_api()
            .edit_servers(&self.app, edit, window.clone())
            .await
            .map_err(|refused| match refused {
                porter_service_accounts::EditError::Fixed(fixed) => EditError::Fixed(fixed),
                porter_service_accounts::EditError::NotThatProvider => EditError::NotThatProvider,
                porter_service_accounts::EditError::Refused(refusal) => EditError::Refused(refusal),
                // a reason a newer porter adds
                _ => EditError::Refused(Refusal::Unavailable),
            })
    }
}

impl<C: Service + 'static, H: RelayHost + 'static> GrantsLink for Hosted<C, H> {
    async fn candidates(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Vec<Candidate>, GrantsError> {
        Ok(self
            .service
            .grants_api()
            .candidates(&self.app, need, class, usage))
    }

    async fn availability(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Availability, GrantsError> {
        Ok(self
            .service
            .grants_api()
            .availability(&self.app, need, class, usage))
    }

    async fn grants(&self) -> Result<Vec<Grant>, GrantsError> {
        Ok(self.service.grants_api().grants(&self.app))
    }

    async fn request_grant(
        &self,
        offer: &ConsentOffer,
        window: &ParentWindow,
    ) -> Result<Candidate, GrantsError> {
        let grants = self.service.grants_api();
        match grants
            .choose(
                &self.app,
                offer.need.clone(),
                (offer.class, offer.usage),
                offer.account.as_ref(),
                window,
                None,
            )
            .await
        {
            AccountsReply::Chosen(candidate) => Ok(candidate),
            AccountsReply::Refused(refusal) => Err(GrantsError::Refused(refusal)),
            _ => Err(GrantsError::Refused(Refusal::Unavailable)),
        }
    }

    async fn revoke(&self, grant: &GrantId) -> Result<(), GrantsError> {
        self.service
            .grants_api()
            .revoke(&self.app, grant)
            .await
            .map_err(GrantsError::Refused)
    }

    async fn accounts(&self) -> Result<Vec<ListedAccount>, GrantsError> {
        // No provider names are kept here: the labels are accountd's (`provider_names`).
        Ok(self
            .service
            .grants_api()
            .accounts()
            .into_iter()
            .map(|account| ListedAccount::new(account, None))
            .collect())
    }

    async fn extend_access(
        &self,
        account: &AccountId,
        kinds: &[CapabilityKind],
        window: &ParentWindow,
    ) -> Result<(), GrantsError> {
        self.service
            .accounts_api()
            .extend_access(&self.app, account, kinds.to_vec(), window.clone())
            .await
            .map_err(GrantsError::Refused)
    }

    async fn asks_on_use(
        &self,
        provider: &ProviderId,
        kind: CapabilityKind,
    ) -> Result<bool, GrantsError> {
        Ok(self.service.accounts_api().asks_on_use(provider, kind))
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<AuthenticatedStream, GrantsError> {
        let plan = self
            .service
            .grants_api()
            .open_authenticated(&self.app, grant, endpoint)
            .await
            .map_err(GrantsError::Refused)?;
        Ok(self.relayed(plan))
    }

    async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<AuthenticatedStream, GrantsError> {
        let plan = self
            .service
            .grants_api()
            .open_linked(&self.app, grant, origin)
            .await
            .map_err(GrantsError::Refused)?;
        Ok(self.relayed(plan))
    }
}

impl<C, H: RelayHost> Hosted<C, H> {
    /// The app's end of an in-memory duplex whose other end the relay host runs `plan` on.
    fn relayed(&self, plan: RelayPlan) -> AuthenticatedStream {
        let (app_end, relay_end) = duplex(RELAY_BUFFER);
        self.relays.run(plan, relay_end);
        AuthenticatedStream::Memory(app_end)
    }
}

/// Everything a hosted handle carries: what [`hosted`] promises of the type it hides.
pub trait Hosting: AccountsLink + ServerEdits + GrantsLink + std::fmt::Debug {}

impl<T: AccountsLink + ServerEdits + GrantsLink + std::fmt::Debug> Hosting for T {}

/// What an app that runs the service itself brings: who it is, its providers, its secret store
/// and the host that draws its sheets; the clock, the store of its registry and the relay host
/// default to the system clock, none and none.
#[derive(Debug)]
pub struct HostConfig<P, S, U, K = SystemClock, R = NoStore, H = NoRelays> {
    app: AppId,
    providers: Vec<P>,
    secrets: S,
    sheets: U,
    clock: K,
    store: R,
    relays: H,
}

impl<P: Provider, S: Secrets, U: Sheets> HostConfig<P, S, U> {
    /// `app`'s service over `providers`, filing credentials in `secrets` and drawing sheets through
    /// `sheets`, starting from an empty registry that lives for the process.
    pub fn new(app: AppId, providers: Vec<P>, secrets: S, sheets: U) -> Self {
        Self {
            app,
            providers,
            secrets,
            sheets,
            clock: SystemClock,
            store: NoStore,
            relays: NoRelays,
        }
    }
}

impl<P, S, U, K, R, H> HostConfig<P, S, U, K, R, H> {
    /// With this clock.
    #[must_use]
    pub fn with_clock<N: Clock>(self, clock: N) -> HostConfig<P, S, U, N, R, H> {
        HostConfig {
            app: self.app,
            providers: self.providers,
            secrets: self.secrets,
            sheets: self.sheets,
            clock,
            store: self.store,
            relays: self.relays,
        }
    }

    /// With the registry saved to `store` after every change.
    #[must_use]
    pub fn with_store<N: RegistryStore>(self, store: N) -> HostConfig<P, S, U, K, N, H> {
        HostConfig {
            app: self.app,
            providers: self.providers,
            secrets: self.secrets,
            sheets: self.sheets,
            clock: self.clock,
            store,
            relays: self.relays,
        }
    }

    /// With these relays running the authenticated streams.
    #[must_use]
    pub fn with_relays<N: RelayHost>(self, relays: N) -> HostConfig<P, S, U, K, R, N> {
        HostConfig {
            app: self.app,
            providers: self.providers,
            secrets: self.secrets,
            sheets: self.sheets,
            clock: self.clock,
            store: self.store,
            relays,
        }
    }
}

/// The handle over the service `config` describes, answering for the app it names.
pub fn hosted<P, S, U, K, R, H>(config: HostConfig<P, S, U, K, R, H>) -> Porter<impl Hosting>
where
    P: Provider + 'static,
    S: Secrets + 'static,
    U: Sheets + 'static,
    K: Clock + 'static,
    R: RegistryStore + 'static,
    H: RelayHost + std::fmt::Debug + 'static,
{
    let service = AccountService::new(
        config.providers,
        Registry::from_persisted(Persisted::empty()),
        config.secrets,
        config.sheets,
        config.clock,
    )
    .with_store(config.store);
    Porter::over(Hosted::new(Arc::new(service), config.app).with_relays(config.relays))
}
