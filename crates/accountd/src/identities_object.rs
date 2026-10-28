//! `org.quire.Identities1` at `/org/quire/Identities1`: the identities signed in on this machine
//! (the registry's accounts of a sign-in kind: OAuth, a password, an app password), on accountd's
//! connection. Local runtimes, Tailscale, AI keys and agent logins are not identities, and
//! `registry.json` is read as it is.
//!
//! - Any identified app (and the porter daemons) may `List`, `Offer` and `RegisterUse`. `Uses`
//!   only the Settings and sheet-host roles (the Settings app and the shell). The agent, computer
//!   use, the agent launcher, the terminal and the compositor are `AccessDenied`, and so is a role
//!   porter does not know yet (the `_` arm fails closed), and a sender accountd does not know.
//! - `Offer` shows the identity picker through the sheet host (the sheet `Choose` draws, with
//!   `AskPurpose::ContinueAs`) and waits for the person: the answer is the identity's id, address
//!   and provider, or `Dismissed` (the person closed the sheet). It never returns a token, and it
//!   makes no grant. An app has one sheet of this kind open at a time, as for `Choose`; the call
//!   ends without an answer if the app leaves the bus, and the sheet is taken down.
//! - `TakePassword` hands over the password of a `password` identity, on a sealed descriptor, and
//!   only to the connection whose `Offer` the person just answered with that identity, once. It
//!   is audited (`PasswordHandedOver`). Every other call is `AccessDenied`. The password is in
//!   no string, no signal and no audit line.
//! - `RegisterUse` and `Uses` are the "apps using this identity" list (`identities`), kept
//!   beside `registry.json`. When an identity is removed (the registry loses its account) the list
//!   for it goes, and `IdentityRemoved(identity_id)` is sent to each allowed connection accountd
//!   knows. An app decides what to do with its own copies; accountd reaches into nothing.

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::request::departed;
use crate::state::{Core, Standing, hint};
use porter_callers::CallerRole;
use porter_capability::CapabilityKind;
use porter_core::AccountId;
use porter_core::ProviderHint;
use porter_core::consent::{AccountChoice, AskPurpose, ConsentAnswer, ConsentAsk, Usage};
use porter_core::{AccountLabel, DataClass, Identity, IdentityId, IdentityKind};
use porter_dbus::accounts::IDENTITIES_PATH;
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::accounts::{IdentitiesServed, IdentitiesServer, identity_to_dbus};
use porter_dbus::{Call, Details};
use porter_identities::{IdentitiesApi, Unsaved};
use porter_secrets::sealed_key;
use porter_service_core::registry::Registry;
use porter_wire::Refusal;
use std::sync::Arc;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

/// The Identities object.
#[derive(Debug)]
pub(crate) struct IdentitiesObject<H, C>(Arc<Core<H, C>>);

impl<H, C> IdentitiesObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// An identity and the account it is.
type Held = (Identity, AccountId);

/// The identities `registry` holds, in its order. `names` says what a provider is called.
fn identities_in<H, C>(core: &Core<H, C>, registry: &Registry) -> Vec<Held> {
    registry
        .accounts
        .iter()
        .filter_map(|account| {
            let name = core.provider_names.label_of(&account.provider);
            Some((Identity::of(account, name)?, account.id.clone()))
        })
        .collect()
}

/// Whether a connection of this role is told of identities and may ask about them.
fn may_use(role: CallerRole) -> bool {
    matches!(
        role,
        CallerRole::App | CallerRole::Settings | CallerRole::SheetHost | CallerRole::PorterDaemon
    )
}

fn identity_arg(text: &str) -> Result<IdentityId, RefusedError> {
    IdentityId::parse(text).map_err(|_| RefusedError::invalid("There is no such identity."))
}

/// What a use that could not be saved is told, in plain words.
fn unsaved(_: Unsaved) -> RefusedError {
    RefusedError::failed("The list of apps could not be saved.")
}

impl<H: Host, C: Callers> IdentitiesObject<H, C> {
    /// The caller, which must be an app the person uses, Settings, the shell or a porter daemon.
    /// The agent, computer use, the agent launcher, the terminal and the compositor are refused,
    /// and so is a role porter does not know yet.
    async fn user(&self, header: &Header<'_>) -> Result<porter_callers::Caller, RefusedError> {
        let caller = self.0.identify(header, Standing::Any).await?;
        match caller.role {
            role if may_use(role) => Ok(caller),
            CallerRole::Agent
            | CallerRole::Cua
            | CallerRole::AgentLauncher
            | CallerRole::Terminal
            | CallerRole::Compositor => Err(RefusedError::access_denied(
                "only an app the person uses may ask about identities",
            )),
            // A role porter does not know yet asks nothing.
            _ => Err(RefusedError::access_denied(
                "only an app the person uses may ask about identities",
            )),
        }
    }

    /// The caller, which must be Settings or the shell.
    async fn manager(&self, header: &Header<'_>) -> Result<porter_callers::Caller, RefusedError> {
        let caller = self.user(header).await?;
        match caller.role {
            CallerRole::Settings | CallerRole::SheetHost => Ok(caller),
            _ => Err(RefusedError::access_denied(
                "only Settings and the shell may see which apps use an identity",
            )),
        }
    }

    /// The identities the registry holds now.
    fn held(&self) -> Vec<Held> {
        identities_in(self.0.as_ref(), &self.0.host.registry())
    }

    /// The identity `id` is, with its account.
    fn find(&self, id: &IdentityId) -> Result<Held, RefusedError> {
        self.held()
            .into_iter()
            .find(|(identity, _)| identity.id == *id)
            .ok_or_else(|| RefusedError::invalid("There is no such identity."))
    }
}

impl<H: Host, C: Callers> IdentitiesServer for IdentitiesObject<H, C> {
    type Error = RefusedError;

    async fn list(
        &self,
        Call { header, .. }: Call<'_>,
    ) -> Result<Vec<(String, Details)>, RefusedError> {
        self.user(&header).await?;
        Ok(self
            .held()
            .iter()
            .map(|(identity, _)| identity_to_dbus(identity))
            .collect())
    }

    async fn offer(
        &self,
        Call { header, .. }: Call<'_>,
        provider_hint: String,
        parent_window: String,
    ) -> Result<(String, String, String), RefusedError> {
        let caller = self.user(&header).await?;
        let hint = hint(&provider_hint)?;
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_owned();
        let desk = self
            .0
            .identity_desk
            .as_ref()
            .ok_or_else(|| RefusedError::of(Refusal::Unavailable))?;
        let fitting: Vec<Held> = self
            .held()
            .into_iter()
            .filter(|(identity, _)| match &hint {
                ProviderHint::Provider(provider) => identity.provider == *provider,
                _ => true,
            })
            .collect();
        if fitting.is_empty() {
            return Err(RefusedError::of(Refusal::NoFittingAccount));
        }
        // One sheet of this kind at a time, as for `Choose`; held until the person answers.
        let _claim = self.0.open_sheets.claim(&caller.app, SheetKind::Choose)?;
        // A new offer ends what an earlier one allowed.
        self.0.picks.clear(sender.as_str());
        let choices = fitting
            .iter()
            .map(|(identity, account)| {
                AccountChoice::new(
                    account.clone(),
                    AccountLabel(identity.address.0.clone()),
                    identity.provider.clone(),
                )
            })
            .collect();
        let ask = ConsentAsk::new(
            caller.app.clone(),
            CapabilityKind::Identity,
            DataClass::AppOwn,
            Usage::Interactive,
            choices,
        )
        .with_purpose(AskPurpose::ContinueAs);
        let leaving = DBusProxy::new(&self.0.connection)
            .await
            .map_err(RefusedError::failed)?
            .receive_name_owner_changed_with_args(&[(0, sender.as_str())])
            .await
            .map_err(RefusedError::failed)?;
        let window = crate::state::window(&parent_window);
        let answer = tokio::select! {
            answer = desk.pick(ask, &window) => answer,
            // The app left: the sheet is taken down with the dropped pick, and nobody is told.
            () = departed(leaving) => return Err(RefusedError::of(Refusal::Dismissed)),
        };
        let picked = match answer {
            ConsentAnswer::Allow { account, .. } => fitting
                .iter()
                .find(|(_, offered)| *offered == account)
                .map(|(identity, _)| identity.clone()),
            // Closed, turned down, or an answer for something this sheet did not offer.
            _ => None,
        };
        let identity = picked.ok_or_else(|| RefusedError::of(Refusal::Dismissed))?;
        self.0.picks.set(sender.as_str(), identity.id.clone());
        Ok((
            identity.id.as_str().to_owned(),
            identity.address.0,
            identity.provider.as_str().to_owned(),
        ))
    }

    async fn take_password(
        &self,
        Call { header, .. }: Call<'_>,
        identity_id: String,
    ) -> Result<OwnedFd, RefusedError> {
        let caller = self.user(&header).await?;
        let id = identity_arg(&identity_id)?;
        let (identity, account) = self.find(&id)?;
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        // Only a password goes over, and only to the app the person just picked it for.
        if identity.kind != IdentityKind::Password {
            return Err(RefusedError::access_denied(
                "this identity signs in in the browser; there is no password to hand over",
            ));
        }
        if !self.0.picks.take(sender.as_str(), &id) {
            return Err(RefusedError::access_denied(
                "a password goes only to the app the person has just picked this identity for, once",
            ));
        }
        let desk = self
            .0
            .identity_desk
            .as_ref()
            .ok_or_else(|| RefusedError::of(Refusal::Unavailable))?;
        let password = desk.password(&account).await.map_err(RefusedError::of)?;
        let fd = sealed_key(password.expose()).map_err(|why| {
            // The cause goes to standard error, never the password.
            eprintln!("accountd: identity password {id}: {why}");
            RefusedError::of(Refusal::Unavailable)
        })?;
        desk.note_password(&caller.app, &account);
        Ok(OwnedFd::from(fd))
    }

    async fn register_use(
        &self,
        Call { header, .. }: Call<'_>,
        identity_id: String,
    ) -> Result<(), RefusedError> {
        let caller = self.user(&header).await?;
        let (identity, _) = self.find(&identity_arg(&identity_id)?)?;
        self.0
            .uses
            .lock()
            .await
            .record(identity.id, caller.app.name)
            .await
            .map_err(unsaved)
    }

    async fn uses(
        &self,
        Call { header, .. }: Call<'_>,
        identity_id: String,
    ) -> Result<Vec<String>, RefusedError> {
        self.manager(&header).await?;
        let (identity, _) = self.find(&identity_arg(&identity_id)?)?;
        Ok(self
            .0
            .uses
            .lock()
            .await
            .apps(&identity.id)
            .iter()
            .map(ToString::to_string)
            .collect())
    }
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Tells what the removal of an identity leaves to tell: for each account in `before` that is
    /// an identity and is not in `after`, its list of apps goes and `IdentityRemoved` is sent to
    /// each connection accountd knows that may ask about identities.
    pub(crate) async fn announce_identities_removed(&self, before: &Registry, after: &Registry) {
        let gone: Vec<IdentityId> = identities_in(self, before)
            .into_iter()
            .filter(|(_, account)| !after.accounts.iter().any(|a| a.id == *account))
            .map(|(identity, _)| identity.id)
            .collect();
        for identity in gone {
            if let Err(fault) = self.uses.lock().await.forget(&identity).await {
                eprintln!("accountd: identity {identity}: the list of its apps: {fault:?}");
            }
            for name in self.known_where(|caller| may_use(caller.role)) {
                let Ok(destination) = BusName::try_from(name) else {
                    continue;
                };
                let Ok(emitter) = SignalEmitter::new(&self.connection, IDENTITIES_PATH) else {
                    continue;
                };
                let emitter = emitter.set_destination(destination);
                let _ = IdentitiesServed::<IdentitiesObject<H, C>>::identity_removed(
                    &emitter,
                    identity.as_str(),
                )
                .await;
            }
        }
    }
}
