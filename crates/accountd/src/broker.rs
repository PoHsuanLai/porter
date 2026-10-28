//! `org.quire.Accounts1.Broker` at `/org/quire/Accounts1`: what the AI broker (inferd) asks of
//! the daemon that keeps the AI keys. inferd is the one that sends a key to an AI company, so it
//! asks which accounts an app holds a permit on, and for the key behind a permit.
//!
//! Only a connection the caller table knows as the AI broker may call; every other sender is
//! `AccessDenied`. The app is named by the broker from its own connection, never by the app. A key
//! leaves on a sealed memfd, once per call, and `ReadKey` audits it (`KeyResolved`, never the
//! key). `Changed` is sent to the broker alone.

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::state::{Core, Standing};
use porter_callers::CallerRole;
use porter_capability::Claim;
use porter_core::consent::Usage;
use porter_core::{AccountId, AccountState, AppId, AppName, DataClass, GrantId, Isolation};
use porter_core::{AiAccountRow, AuthKind, ProviderId};
use porter_dbus::Call;
use porter_dbus::accounts::{BrokerServed, BrokerServer, RowArg};
use porter_dbus::accounts::{ai_account_to_dbus, own_server_to_dbus, verdict_to_dbus};
use porter_ids::word::from_word;
use porter_service_accounts::LocalFault;
use porter_sheet::ProviderGroup;
use porter_wire::Refusal;
use std::sync::Arc;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

/// The Broker object.
#[derive(Debug)]
pub(crate) struct BrokerObject<H, C>(Arc<Core<H, C>>);

impl<H, C> BrokerObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// A key on a sealed memfd, or why it cannot be.
fn handed(key: &porter_core::SecretText) -> Result<OwnedFd, RefusedError> {
    porter_secrets::sealed_key(key.expose())
        .map(OwnedFd::from)
        .map_err(|why| {
            eprintln!("accountd: a key could not be sealed for the broker: {why}");
            RefusedError::of(Refusal::Unavailable)
        })
}

impl<H: Host, C: Callers> BrokerObject<H, C> {
    /// The caller, which must be the AI broker.
    async fn broker(&self, header: &zbus::message::Header<'_>) -> Result<(), RefusedError> {
        let caller = self.0.identify(header, Standing::Broker).await?;
        match caller.role {
            CallerRole::AiBroker => Ok(()),
            _ => Err(RefusedError::access_denied(
                "only the AI broker may ask for keys on behalf of an app",
            )),
        }
    }
}

fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RefusedError> {
    from_word(text)
        .ok_or_else(|| RefusedError::invalid(format!("`{text}` is not a word of its kind")))
}

impl<H: Host, C: Callers> BrokerServer for BrokerObject<H, C> {
    type Error = RefusedError;

    async fn verdicts(
        &self,
        Call { header, .. }: Call<'_>,
        app: (String, String),
        class: String,
        usage: String,
    ) -> Result<Vec<RowArg>, RefusedError> {
        self.broker(&header).await?;
        let (name, isolation) = app;
        let app = AppId {
            name: AppName::parse(&name).map_err(RefusedError::invalid)?,
            isolation: slug::<Isolation>(&isolation)?,
        };
        let (class, usage): (DataClass, Usage) = (slug(&class)?, slug(&usage)?);
        let names = &self.0.provider_names;
        let label = |provider: &ProviderId| names.label_of(provider).map(str::to_owned);
        Ok(self
            .0
            .host
            .llm_verdicts(&app, class, usage, &label)
            .iter()
            .map(verdict_to_dbus)
            .collect())
    }

    async fn read_key(
        &self,
        Call { header, .. }: Call<'_>,
        grant: String,
    ) -> Result<OwnedFd, RefusedError> {
        self.broker(&header).await?;
        let grant = GrantId::parse(&grant).map_err(RefusedError::invalid)?;
        let key = self
            .0
            .host
            .release_key(&grant)
            .await
            .map_err(RefusedError::of)?;
        // An allow-once permit is spent by the read: the clients hear of it.
        self.0.publish().await;
        handed(&key)
    }

    async fn accounts(&self, Call { header, .. }: Call<'_>) -> Result<Vec<RowArg>, RefusedError> {
        self.broker(&header).await?;
        let names = &self.0.provider_names;
        Ok(self
            .0
            .host
            .registry()
            .accounts
            .iter()
            .filter(|account| names.group_of(account) != ProviderGroup::Internet)
            .map(|account| {
                ai_account_to_dbus(&AiAccountRow::new(
                    account.id.clone(),
                    account.provider.as_str().to_owned(),
                    names.label_of(&account.provider).map(str::to_owned),
                    account.label.0.clone(),
                    account.auth != AuthKind::LocalRuntime,
                ))
            })
            .collect())
    }

    async fn servers(&self, Call { header, .. }: Call<'_>) -> Result<Vec<RowArg>, RefusedError> {
        self.broker(&header).await?;
        Ok(self
            .0
            .host
            .own_servers()
            .iter()
            .map(own_server_to_dbus)
            .collect())
    }

    async fn read_server_key(
        &self,
        Call { header, .. }: Call<'_>,
        account: String,
    ) -> Result<OwnedFd, RefusedError> {
        self.broker(&header).await?;
        let account = AccountId::parse(&account).map_err(RefusedError::invalid)?;
        let key = self
            .0
            .host
            .own_server_key(&account)
            .await
            .map_err(RefusedError::of)?;
        handed(&key)
    }

    async fn report_local(
        &self,
        Call { header, .. }: Call<'_>,
        provider: String,
        claims: String,
        state: String,
    ) -> Result<String, RefusedError> {
        self.broker(&header).await?;
        let provider = ProviderId::parse(&provider).map_err(RefusedError::invalid)?;
        let claims: Vec<Claim> = serde_json::from_str(&claims)
            .map_err(|why| RefusedError::invalid(format!("claims: {why}")))?;
        let state: AccountState = slug(&state)?;
        let id = self
            .0
            .host
            .report_local(&provider, claims, state)
            .await
            .map_err(|fault| match fault {
                LocalFault::Unavailable => RefusedError::of(Refusal::Unavailable),
                other => RefusedError::invalid(format!("report refused: {other:?}")),
            })?;
        self.0.publish().await;
        Ok(id.as_str().to_owned())
    }
}

/// Tells every AI broker connection that has called accountd that an AI account, a permit or a
/// switch changed.
pub(crate) async fn announce<H: Host, C: Callers>(core: &Arc<Core<H, C>>) {
    for name in core.known_where(|caller| caller.role == CallerRole::AiBroker) {
        let Ok(destination) = BusName::try_from(name) else {
            continue;
        };
        let Ok(emitter) =
            SignalEmitter::new(&core.connection, porter_dbus::accounts::ACCOUNTS_PATH)
        else {
            continue;
        };
        let emitter = emitter.set_destination(destination);
        let _ = BrokerServed::<BrokerObject<H, C>>::changed(&emitter).await;
    }
}
