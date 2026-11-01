//! What every object shares: the state of the service's bus front end ([`Core`]: the service, the
//! caller table, the roster of connections, the open sheets, the books), the checks every method
//! starts with, and the arguments every method parses. The objects take it; the code that serves
//! them and tells their clients (`core`, `publish`) is above them.

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use porter_callers::{Caller, CallerRole};
use porter_core::ProviderHint;
use porter_core::ProviderId;
use porter_families::set::ProviderNames;
use porter_ids::clock::Clock;
use porter_sheets::{AppNames, RequestObjects};
use porter_wire::{ParentWindow, Refusal};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tokio::sync::mpsc;
use zbus::Connection;
use zbus::message::Header;

/// How much of its role a caller must have for a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Any identified caller.
    Any,
    /// A caller that may act for the person: an `Agent` is refused (`Denied`).
    Acting,
    /// A caller reading the person's computers (`Tailnet1`): the only standing a `Terminal` has,
    /// and the method that asks for it checks the role itself.
    Machines,
    /// A caller listing or registering desktop-wide Spaces (`Spaces1.List`, `Register`): the only
    /// standing a `Compositor` has; anyone else is as with `Any`, and `Register` checks the role
    /// itself.
    Spaces,
    /// The agent launcher's calls (`Launcher`): the only standing an `AgentLauncher` has, and the
    /// object checks that the caller is one.
    Launcher,
    /// The AI broker's calls (`Broker`): the one standing an `AiBroker` has besides reading the
    /// person's computers, and the object checks that the caller is one.
    Broker,
}

/// Why a role is turned away from a standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turned {
    /// Refused as `Denied`: the role may not act for the person.
    Denied,
    /// Refused as `AccessDenied`, with the words it is told.
    AccessDenied(&'static str),
}

impl Standing {
    /// The standings a role confined to a few has, with what it is told otherwise. Every role not
    /// listed has them all.
    fn confinement(role: CallerRole) -> Option<(&'static [Standing], &'static str)> {
        match role {
            // The agent launcher is nothing else to accountd than the launcher of its programs.
            CallerRole::AgentLauncher => Some((
                &[Standing::Launcher],
                "the agent launcher may only call the launcher's interface",
            )),
            // The AI broker asks for keys and reads the person's computers, and is nothing else.
            CallerRole::AiBroker => Some((
                &[Standing::Broker, Standing::Machines],
                "the AI broker may only ask for keys and read the person's computers",
            )),
            // The terminal is narrower still: it reads the person's computers and is nothing else.
            CallerRole::Terminal => Some((
                &[Standing::Machines],
                "the terminal may only read the person's computers",
            )),
            // The compositor registers the Spaces it mints and is nothing else to accountd.
            CallerRole::Compositor => Some((
                &[Standing::Spaces],
                "the compositor may only list and register Spaces",
            )),
            _ => None,
        }
    }

    /// Whether a caller of `role` may make a call that needs this standing. The one table of the
    /// role rules every method is held to; a method's own check of the role comes after it.
    pub(crate) fn admits(self, role: CallerRole) -> Result<(), Turned> {
        if self == Standing::Acting && role == CallerRole::Agent {
            return Err(Turned::Denied);
        }
        match Self::confinement(role) {
            Some((allowed, why)) if !allowed.contains(&self) => Err(Turned::AccessDenied(why)),
            _ => Ok(()),
        }
    }
}

/// The shared state of every object accountd serves.
pub(crate) struct Core<H, C> {
    pub(crate) host: Arc<H>,
    pub(crate) callers: C,
    /// The connection the objects are served on, for the signals.
    pub(crate) connection: Connection,
    /// The connections that have called, by unique name: who a unicast signal can reach.
    pub(crate) roster: Mutex<BTreeMap<String, Caller>>,
    /// The user's `clients.toml`, which Settings writes.
    pub(crate) clients: Option<std::path::PathBuf>,
    /// Held while Settings reads, edits and writes the clients file, so two edits cannot lose one.
    pub(crate) clients_write: Arc<std::sync::Mutex<()>>,
    /// What the settings module calls an app (the "apps using this identity" rows).
    pub(crate) app_names: AppNames,
    /// What the settings module calls a provider and which part of the page it is listed under.
    pub(crate) provider_names: ProviderNames,
    /// The agent launchers registered and their pending requests (`Launcher`).
    pub(crate) launchers: crate::launcher::Launchers,
    /// The relays run for `Grants1.OpenAuthenticated` and `OpenLinked`.
    pub(crate) relays: crate::relay::Relays,
    /// The credentials handed to the processes the launcher spawns.
    pub(crate) credentials: crate::launcher::Credentials,
    /// The time a property that says when a sign-in ends is read on.
    pub(crate) clock: Arc<dyn Clock>,
    /// The served settings module, which announces an account's new state as `Changed`; set
    /// once the module is served.
    pub(crate) settings: OnceLock<ds_settings::live::Served>,
    /// The sheets open now, one per app and kind (rel-11).
    pub(crate) open_sheets: crate::open_sheets::OpenSheets,
    /// The Request objects of the sheet methods (porter-sheets), at `Accounts1`'s root.
    pub(crate) requests: RequestObjects,
    /// The desktop-wide Spaces (`org.quire.Spaces1`).
    pub(crate) spaces: tokio::sync::Mutex<porter_spaces::SpaceBook>,
    /// How Tailscale is followed (`org.quire.Tailnet1`); none lists no computers.
    pub(crate) tailnet: Option<crate::tailnet::TailnetWatch>,
    /// The identity picker and the passwords it may hand over (`org.quire.Identities1`); none
    /// refuses `Offer` and `TakePassword` as unavailable.
    pub(crate) identity_desk: Option<Arc<dyn porter_identities::IdentityDesk>>,
    /// The apps that use each identity (`Identities1.RegisterUse`).
    pub(crate) uses: tokio::sync::Mutex<porter_identities::UseBook>,
    /// What each connection just picked in `Identities1.Offer`.
    pub(crate) picks: porter_identities::Picks,
    /// Where the host's commits go to be told to the clients, in order (`publish`); a request
    /// to wait until they were told goes down it too.
    pub(crate) news: mpsc::UnboundedSender<crate::publish::Item>,
}

pub(crate) fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Every critical section is a plain data update.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Who the sender of the call is, with its role; a caller that must `Act` is refused as an
    /// `Agent`. An unknown sender is `AccessDenied`. The caller joins the roster.
    pub(crate) async fn identify(
        &self,
        header: &Header<'_>,
        standing: Standing,
    ) -> Result<Caller, RefusedError> {
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        let caller = self
            .callers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(|| RefusedError::access_denied("accountd does not know this caller"))?;
        standing
            .admits(caller.role)
            .map_err(|turned| match turned {
                Turned::Denied => RefusedError::of(Refusal::Denied),
                Turned::AccessDenied(why) => RefusedError::access_denied(why),
            })?;
        held(&self.roster).insert(sender.to_string(), caller.clone());
        Ok(caller)
    }

    /// The unique names of the connections that have called and whose caller `wanted` admits:
    /// who a signal meant for some roles reaches.
    pub(crate) fn known_where(&self, wanted: impl Fn(&Caller) -> bool) -> Vec<String> {
        held(&self.roster)
            .iter()
            .filter(|(_, caller)| wanted(caller))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The unique names of the connections that have called, which a broadcast reaches.
    pub(crate) fn known(&self) -> Vec<String> {
        held(&self.roster).keys().cloned().collect()
    }

    /// Forgets a connection that left the bus.
    pub(crate) async fn left(self: &Arc<Self>, name: &str) {
        held(&self.roster).remove(name);
        self.picks.clear(name);
        // A launcher's sessions close first (their permits are removed and the credentials under
        // them end `session_closed`, with nobody left to tell), then what is left of its
        // credentials ends `launcher_gone`.
        for session in self.launchers.left(name) {
            self.close_session(&session, false).await;
        }
        self.credentials_left(name);
    }
}

/// The `parent_window` argument: empty is no parent.
pub(crate) fn window(text: &str) -> ParentWindow {
    match text.is_empty() {
        true => ParentWindow::Unparented,
        false => ParentWindow::Handle(text.to_owned()),
    }
}

/// The `provider_hint` argument: empty is the provider list.
pub(crate) fn hint(text: &str) -> Result<ProviderHint, RefusedError> {
    match text.is_empty() {
        true => Ok(ProviderHint::Any),
        false => ProviderId::parse(text)
            .map(ProviderHint::Provider)
            .map_err(RefusedError::invalid),
    }
}

impl<H, C> std::fmt::Debug for Core<H, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{Standing, Turned};
    use porter_callers::CallerRole;

    const ALL: [Standing; 6] = [
        Standing::Any,
        Standing::Acting,
        Standing::Machines,
        Standing::Spaces,
        Standing::Launcher,
        Standing::Broker,
    ];

    /// The standings each role is admitted to: the whole table, so a rule cannot change unseen.
    fn admitted(role: CallerRole) -> Vec<Standing> {
        ALL.into_iter()
            .filter(|standing| standing.admits(role).is_ok())
            .collect()
    }

    #[test]
    fn each_role_has_exactly_its_standings() {
        use Standing::*;
        let everywhere = ALL.to_vec();
        for role in [
            CallerRole::App,
            CallerRole::Settings,
            CallerRole::SheetHost,
            CallerRole::PorterDaemon,
            CallerRole::Cua,
        ] {
            assert_eq!(admitted(role), everywhere, "{role:?}");
        }
        assert_eq!(
            admitted(CallerRole::Agent),
            [Any, Machines, Spaces, Launcher, Broker]
        );
        assert_eq!(admitted(CallerRole::AgentLauncher), [Launcher]);
        assert_eq!(admitted(CallerRole::AiBroker), [Machines, Broker]);
        assert_eq!(admitted(CallerRole::Terminal), [Machines]);
        assert_eq!(admitted(CallerRole::Compositor), [Spaces]);
    }

    #[test]
    fn an_agent_is_denied_and_a_confined_role_is_told_why() {
        assert_eq!(
            Standing::Acting.admits(CallerRole::Agent),
            Err(Turned::Denied)
        );
        assert!(matches!(
            Standing::Any.admits(CallerRole::Terminal),
            Err(Turned::AccessDenied(_))
        ));
    }
}
