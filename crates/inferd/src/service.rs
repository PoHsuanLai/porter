//! `org.quire.Inference1` at `/org/quire/Inference1`: the daemon's object. `Open` checks the
//! caller (connection to pid to executable to caller), makes a socketpair, hands one end back as
//! the reply descriptor and runs [`serve_session`] on the other, over the four seams: this
//! session's router (which decides once and pins the runner), the shared engines, the session's
//! turns and its audit sink. The introspection of this object is the checked-in
//! `dbus/org.quire.Inference1.xml` (`tests/introspection.rs`).

use self::error::InferError;
use crate::agent::Agents;
use crate::agent::service::AgentsService;
use crate::attached::{
    ComputerError, Computers, NewComputer, NewModel, NewReach, NewTailnetComputer,
};
use crate::audit::{AuditOut, SessionAudit};
use crate::clock::Clock;
use crate::engines::Engines;
use crate::peers::{Caller, Peers, Role};
use crate::pipeline::Hearing;
use crate::router::placed::{Allowed, Unplaced};
use crate::runner::{Pin, Turns};
use crate::serve::{Seams, serve_session};
use crate::session::SessionSpec;
use crate::settings::Reload;
use crate::structured::Limits;
use crate::tailnet::{Candidate, Tailnet};
use crate::watch::Probing;
use porter_core::{DataClass, ModelId, NodeId, SecretText, Tier};
use porter_dbus::{
    CANDIDATE_KEY_MODELS, CANDIDATE_KEY_NAME, CANDIDATE_KEY_NEEDS_APPROVAL, Details,
    GUEST_KEY_NAME, GUEST_KEY_SINCE, GUEST_KEY_STATE, INFERENCE_BUS, INFERENCE_PATH, NeedArg,
    PLACE_KEY_KIND, PLACE_KEY_MODELS, PLACE_KEY_NAME, PLACE_KEY_PROVIDER, PLACE_KEY_READY,
    need_from_dbus,
};
use porter_infer::{InferRefusal, PlaceId, PlaceRow, PlaceState};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::os::unix::net::UnixStream as StdStream;
use std::sync::Arc;
use tokio::net::UnixStream;
use zbus::fdo;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

/// The daemon's state, shared by every call.
#[derive(Debug)]
pub struct Inference<P, O, C> {
    engines: Engines,
    peers: Arc<P>,
    audit: Arc<O>,
    clock: C,
    limits: Limits,
    reload: Option<Reload>,
    probing: Option<Probing>,
    agents: Option<Agents>,
    computers: Option<Arc<Computers>>,
    tailnet: Option<Tailnet>,
}

impl<P, O, C> Inference<P, O, C> {
    /// The object over these engines, peers, audit destination and clock.
    pub fn new(engines: Engines, peers: P, audit: O, clock: C) -> Self {
        Self {
            engines,
            peers: Arc::new(peers),
            audit: Arc::new(audit),
            clock,
            limits: Limits::default(),
            reload: None,
            probing: None,
            agents: None,
            computers: None,
            tailnet: None,
        }
    }

    /// The same object, whose `Candidates`, `AddTailnetComputer` and guest methods work over the
    /// person's Tailscale network through `tailnet`. Without it the lists are empty and an
    /// addition answers that it is not available here.
    pub fn tailnet(self, tailnet: Tailnet) -> Self {
        Self {
            tailnet: Some(tailnet),
            ..self
        }
    }

    /// The same object, whose `AddComputer` and `RemoveComputer` change the computers Settings
    /// added. Without it both answer that adding computers is not available.
    pub fn computers(self, computers: Computers) -> Self {
        Self {
            computers: Some(Arc::new(computers)),
            ..self
        }
    }

    /// The same object, also serving `org.quire.Inference1.Agents` to the agent launcher over
    /// these endpoints (`serve_on` puts it on the bus).
    pub fn agents(self, agents: Agents) -> Self {
        Self {
            agents: Some(agents),
            ..self
        }
    }

    /// The same object, whose `Rescan` reloads the settings from the file through `reload`.
    pub fn reloading(self, reload: Reload) -> Self {
        Self {
            reload: Some(reload),
            ..self
        }
    }

    /// The same object, whose `Rescan` also looks at once for the runtimes the person runs.
    pub fn probing(self, probing: Probing) -> Self {
        Self {
            probing: Some(probing),
            ..self
        }
    }

    /// The same object under the configured structured-output limits.
    pub fn limited(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }
}

/// The text a closed set is written as on the bus: its serde slug.
fn slug<T: Serialize>(value: &T, field: &str) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(serde_json::Value::Object(map)) => map
            .get(field)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

fn parse_slug<T: DeserializeOwned>(text: &str) -> fdo::Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))
}

/// What the bus says to a caller that is nobody.
fn unknown_caller() -> fdo::Error {
    fdo::Error::AccessDenied("inferd: the caller is not in its caller table".into())
}

/// The W3C trace context of a call, if it carries a valid one (`traceparent` in the options; an
/// unknown key is ignored, as the interface says). No spans are written yet.
fn trace_of(options: &Details) -> Option<porter_infer::Traceparent> {
    let value = options.get(porter_dbus::OPTION_TRACEPARENT)?;
    let text = String::try_from(value.try_clone().ok()?).ok()?;
    porter_infer::Traceparent::parse(&text).ok()
}

/// The usage a call names (`usage` in the options): `Interactive` when it names none, and
/// invalid args for a slug that is not one.
fn usage_of(options: &Details) -> fdo::Result<porter_core::consent::Usage> {
    let Some(value) = options.get(porter_dbus::OPTION_USAGE) else {
        return Ok(porter_core::consent::Usage::Interactive);
    };
    let text = value
        .try_clone()
        .ok()
        .and_then(|value| String::try_from(value).ok())
        .ok_or_else(|| fdo::Error::InvalidArgs("usage is not a string".into()))?;
    parse_slug(&text)
}

fn failed(why: impl ToString) -> fdo::Error {
    fdo::Error::Failed(why.to_string())
}

/// The places a call may run (`places` and `place_models` in the options), when it names them.
/// Only a caller that may choose places (`Role::Placer`) may send either key: from anyone else
/// the call is refused, not ignored. `place_models` without `places`, or naming a place outside
/// it, is invalid args, as is a value that is not what the interface says.
fn allowed_of(caller: &Caller, options: &Details) -> fdo::Result<Option<Allowed>> {
    let (places, pins) = (
        options.get(porter_dbus::OPTION_PLACES),
        options.get(porter_dbus::OPTION_PLACE_MODELS),
    );
    if places.is_none() && pins.is_none() {
        return Ok(None);
    }
    if !caller.may_choose_places() {
        return Err(fdo::Error::AccessDenied(
            "inferd: this caller may not choose where the assistant runs".into(),
        ));
    }
    let invalid = |what: &str| fdo::Error::InvalidArgs(what.to_owned());
    let places = places.ok_or_else(|| invalid("place_models needs places"))?;
    let texts = places
        .try_clone()
        .ok()
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .ok_or_else(|| invalid("places is not a list of text"))?;
    let order = texts
        .iter()
        .map(|text| PlaceId::parse(text).map_err(|e| fdo::Error::InvalidArgs(e.to_string())))
        .collect::<fdo::Result<Vec<_>>>()?;
    let mut pinned = std::collections::BTreeMap::new();
    if let Some(value) = pins {
        let rows = value
            .try_clone()
            .ok()
            .and_then(|value| std::collections::HashMap::<String, String>::try_from(value).ok())
            .ok_or_else(|| invalid("place_models is not a dictionary of text to text"))?;
        for (place, model) in rows {
            let place =
                PlaceId::parse(&place).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
            let model =
                ModelId::parse(&model).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
            if !order.contains(&place) {
                return Err(invalid("place_models names a place that is not in places"));
            }
            pinned.insert(place, model);
        }
    }
    Ok(Some(Allowed::new(order, pinned)))
}

impl<P: Peers, O: AuditOut + 'static, C: Clock + Clone + 'static> Inference<P, O, C> {
    async fn identified(&self, header: &Header<'_>) -> fdo::Result<Caller> {
        let sender = header.sender().ok_or_else(unknown_caller)?;
        self.peers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(unknown_caller)
    }

    async fn caller(&self, header: &Header<'_>) -> fdo::Result<Caller> {
        let caller = self.identified(header).await?;
        match caller.role {
            // The agent launcher has the endpoint interface and nothing else here.
            Role::AgentLauncher => Err(fdo::Error::AccessDenied(
                "inferd: the agent launcher may only open agent endpoints".into(),
            )),
            // The shell lists the places and asks for no model.
            Role::Shell => Err(fdo::Error::AccessDenied(
                "inferd: the shell may only list the places".into(),
            )),
            _ => Ok(caller),
        }
    }

    /// The caller of `AddComputer`, `RemoveComputer`, `Candidates`, `AddTailnetComputer` and
    /// `ForgetGuest`: Settings, by its unit, and no one else.
    async fn settings_caller(&self, header: &Header<'_>) -> fdo::Result<Caller> {
        let caller = self.identified(header).await?;
        if caller.role == Role::Settings {
            Ok(caller)
        } else {
            Err(fdo::Error::AccessDenied(
                "inferd: only Settings may do this".into(),
            ))
        }
    }

    /// The caller of `Guests` and `AnswerGuest`: Settings or the shell.
    async fn guest_caller(&self, header: &Header<'_>) -> fdo::Result<Caller> {
        let caller = self.identified(header).await?;
        if caller.may_answer_guests() {
            Ok(caller)
        } else {
            Err(fdo::Error::AccessDenied(
                "inferd: only Settings and the shell may see or answer the computers that ask"
                    .into(),
            ))
        }
    }

    /// Makes the names of the computers added over Tailscale follow the names Tailscale knows
    /// them by.
    async fn follow_names(&self) {
        let (Some(computers), Some(tailnet)) = (&self.computers, &self.tailnet) else {
            return;
        };
        if computers.tailnet_nodes().is_empty() {
            return;
        }
        let names = tailnet
            .machines()
            .machines()
            .await
            .into_iter()
            .map(|machine| (machine.node, machine.name))
            .collect();
        computers.relabel(&names);
    }

    /// The caller of `Places`: Settings, the shell or the companion.
    async fn place_lister(&self, header: &Header<'_>) -> fdo::Result<Caller> {
        let caller = self.identified(header).await?;
        if caller.may_list_places() {
            Ok(caller)
        } else {
            Err(fdo::Error::AccessDenied(
                "inferd: this caller may not list the places".into(),
            ))
        }
    }

    fn spec(need: NeedArg, class: &str, tier: &str, options: &Details) -> fdo::Result<SessionSpec> {
        Ok(SessionSpec {
            need: need_from_dbus(need).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?,
            class: parse_slug::<DataClass>(class)?,
            tier: parse_slug::<Tier>(tier)?,
            usage: usage_of(options)?,
        })
    }

    /// Serves one session on a fresh socketpair and returns the client's end.
    fn open_session(
        &self,
        caller: Caller,
        spec: SessionSpec,
        allowed: Option<Allowed>,
    ) -> fdo::Result<OwnedFd> {
        let (ours, theirs) = StdStream::pair().map_err(failed)?;
        ours.set_nonblocking(true).map_err(failed)?;
        let stream = UnixStream::from_std(ours).map_err(failed)?;
        let pin = Pin::new();
        let turns = Turns::new(pin.clone(), self.engines.supervised().clone(), spec.tier)
            .limited(self.limits)
            .hearing(Hearing {
                engines: self.engines.clone(),
                app: Some(caller.app.clone()),
                spec: spec.clone(),
                allowed: allowed.clone(),
            });
        let seams = Seams {
            router: self.engines.router_for_in(pin, &caller, allowed),
            engines: self.engines.clone(),
            runner: match self.engines.cloud() {
                Some(cloud) => turns.hosted(cloud.clone()),
                None => turns,
            },
            audit: SessionAudit::new(caller.app, Arc::clone(&self.audit), self.clock.clone()),
        };
        tokio::spawn(async move { serve_session(stream, spec, &seams).await });
        Ok(OwnedFd::from(std::os::fd::OwnedFd::from(theirs)))
    }
}

#[zbus::interface(name = "org.quire.Inference1")]
impl<P: Peers, O: AuditOut + 'static, C: Clock + Clone + 'static> Inference<P, O, C> {
    async fn availability(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        options: Details,
    ) -> Result<String, InferError> {
        let caller = self.caller(&header).await?;
        let _trace = trace_of(&options);
        let spec = Self::spec(need, &class, "balanced", &options)?;
        let allowed = allowed_of(&caller, &options)?;
        let availability = self
            .engines
            .availability_in(
                &spec.need,
                spec.class,
                spec.usage,
                &caller,
                allowed.as_ref(),
            )
            .await
            .map_err(InferError::no_place)?;
        Ok(slug(&availability, "kind"))
    }

    async fn open(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> Result<OwnedFd, InferError> {
        let caller = self.caller(&header).await?;
        let _trace = trace_of(&options);
        let spec = Self::spec(need, &class, &tier, &options)?;
        let allowed = allowed_of(&caller, &options)?;
        // A call that named its places and has none to run in is told so here, as an error with
        // its reason, and no session is opened. (Any other refusal is the session's to tell.)
        if let Some(allowed) = &allowed {
            self.engines
                .placement(&spec, &caller, allowed)
                .await
                .map_err(InferError::no_place)?;
        }
        Ok(self.open_session(caller, spec, allowed)?)
    }

    async fn prepare(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> Result<String, InferError> {
        let caller = self.caller(&header).await?;
        let _trace = trace_of(&options);
        let spec = Self::spec(need, &class, &tier, &options)?;
        let allowed = allowed_of(&caller, &options)?;
        let prepared = self
            .engines
            .prepare_in(
                &spec.need,
                spec.class,
                spec.tier,
                spec.usage,
                &caller,
                allowed.as_ref(),
            )
            .await;
        match prepared {
            Ok(readiness) => Ok(readiness.slug().to_owned()),
            Err(Unplaced::NoPlace(refusal)) => Err(InferError::no_place(refusal)),
            Err(Unplaced::Other(refusal)) => Ok(refusal_slug(&refusal.refusal)),
        }
    }

    // What the caller used this period, by name: tokens and micro-dollars of today, micro-dollars of
    // the month, and each spend cap that is set; empty on a daemon that serves no hosted model.
    // (Plain comments: a doc comment on a member would change the introspection XML.)
    async fn usage(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<Details> {
        let caller = self.caller(&header).await?;
        Ok(self
            .engines
            .usage_of(&caller.app)
            .into_iter()
            .filter_map(|(name, value)| {
                let value = zbus::zvariant::OwnedValue::try_from(zbus::zvariant::Value::U64(value));
                Some((name, value.ok()?))
            })
            .collect())
    }

    // Looks again: weights that arrived, engines that stopped, and the settings file (a file that
    // does not read keeps the settings in force and is the error). Readiness is read live, so
    // for it this only tells listeners to re-read.
    async fn rescan(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        self.caller(&header).await?;
        if let Some(reload) = &self.reload {
            reload.now().map_err(failed)?;
        }
        if let Some(probing) = &self.probing {
            probing.now().await;
        }
        Self::engines_changed(&emitter).await.map_err(failed)
    }

    // The places the assistant could run, one row each (a plain comment: a doc comment on a
    // member would change the introspection XML). The members follow the order of the checked-in
    // XML: Places, then AddComputer and RemoveComputer.
    async fn places(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<Vec<(String, Details)>> {
        let caller = self.place_lister(&header).await?;
        self.follow_names().await;
        Ok(self
            .engines
            .places(&caller)
            .await
            .into_iter()
            .map(place_row)
            .collect())
    }

    // Adds a computer of the person's own (Settings only) and answers its place id; the models
    // arrive with how each is reached and, if it has one, its key, which is kept in a file only
    // the owner reads and never comes back.
    async fn add_computer(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        name: String,
        models: Vec<(String, Details)>,
    ) -> Result<String, InferError> {
        self.settings_caller(&header).await?;
        let computers = self.computers.as_ref().ok_or(ComputerError::Unavailable)?;
        let placed = computers.add(new_computer(name, models)?)?;
        Self::engines_changed(&emitter).await.map_err(failed)?;
        Ok(placed.to_string())
    }

    // Removes a computer Settings added (Settings only).
    async fn remove_computer(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        name: String,
    ) -> Result<(), InferError> {
        self.settings_caller(&header).await?;
        let computers = self.computers.as_ref().ok_or(ComputerError::Unavailable)?;
        if let (Some(node), Some(tailnet)) = (computers.remove(&name)?, &self.tailnet) {
            tailnet.drop_relay(&node);
        }
        Ok(Self::engines_changed(&emitter).await.map_err(failed)?)
    }

    // The person's own computers on their Tailscale network that lend their models and are not
    // added yet (Settings only). Looking is done now for the computers whose look is due, and
    // never while Tailscale is not an account (accountd lists none then).
    async fn candidates(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<Vec<(String, Details)>> {
        self.settings_caller(&header).await?;
        let Some(tailnet) = &self.tailnet else {
            return Ok(Vec::new());
        };
        let added = self
            .computers
            .as_ref()
            .map(|computers| computers.tailnet_nodes())
            .unwrap_or_default();
        Ok(tailnet
            .candidates()
            .await
            .into_iter()
            .filter(|candidate| !added.contains_key(&candidate.node))
            .map(candidate_row)
            .collect())
    }

    // Adds one of those computers (Settings only): the models it lends that the assistant knows
    // and this computer does not run itself, reached through a relay that asks Tailscale who is
    // there on every connection. Answers its place id.
    async fn add_tailnet_computer(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        node: String,
    ) -> Result<String, InferError> {
        self.settings_caller(&header).await?;
        let node = NodeId::parse(&node).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        let (Some(computers), Some(tailnet)) = (&self.computers, &self.tailnet) else {
            return Err(ComputerError::Unavailable.into());
        };
        let Some(found) = tailnet
            .candidates()
            .await
            .into_iter()
            .find(|candidate| candidate.node == node)
        else {
            let mine = tailnet
                .machines()
                .machines()
                .await
                .into_iter()
                .any(|machine| {
                    machine.node == node && machine.owner == porter_core::MachineOwner::Mine
                });
            return Err(if mine {
                ComputerError::NotAnswering
            } else {
                ComputerError::NotOnTailscale
            }
            .into());
        };
        // A model this computer runs itself stays what it is; one the assistant has no entry
        // for cannot be used.
        let mine: std::collections::BTreeSet<String> = self
            .engines
            .listed()
            .into_iter()
            .filter(|one| one.card.locality == porter_core::Locality::OnDevice)
            .map(|one| one.card.model.to_string())
            .collect();
        let catalogue = self.engines.attached().catalogue();
        let models: Vec<String> = found
            .models
            .iter()
            .map(|model| model.id.clone())
            .filter(|id| !mine.contains(id) && catalogue.iter().any(|entry| entry.id.0 == *id))
            .collect();
        let placed = computers.add_tailnet(NewTailnetComputer {
            label: found.name,
            node: node.clone(),
            models,
        })?;
        if tailnet.ensure_relay(&node).is_err() {
            let _ = computers.remove(placed.as_str());
            return Err(ComputerError::Unavailable.into());
        }
        Self::engines_changed(&emitter).await.map_err(failed)?;
        Ok(placed.to_string())
    }

    // The computers that asked to use this one and were answered, and those asking now (Settings
    // and the shell).
    async fn guests(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<Vec<(String, Details)>> {
        self.guest_caller(&header).await?;
        Ok(self
            .tailnet
            .as_ref()
            .map(|tailnet| tailnet.guests().rows())
            .unwrap_or_default()
            .into_iter()
            .map(guest_row)
            .collect())
    }

    // The person's answer about a computer: the shell answers one that is asking, Settings any
    // computer of the person's network (also one that is never asked about, a server or another
    // person's), and a yes can be taken back by a no.
    async fn answer_guest(
        &self,
        #[zbus(header)] header: Header<'_>,
        node: String,
        answer: String,
    ) -> Result<(), InferError> {
        let caller = self.guest_caller(&header).await?;
        let answer = porter_tailnet::GuestAnswer::from_slug(&answer).ok_or_else(|| {
            fdo::Error::InvalidArgs(format!(
                "the answer is {} or {}",
                porter_dbus::GUEST_ANSWER_ALLOW,
                porter_dbus::GUEST_ANSWER_DENY
            ))
        })?;
        let node = NodeId::parse(&node).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        let tailnet = self.tailnet.as_ref().ok_or(ComputerError::Unavailable)?;
        let now = self.clock.now();
        match tailnet.guests().answer(&node, answer, now) {
            Err(porter_tailnet::GuestError::NotAsking) if caller.role == Role::Settings => {
                let machines = tailnet.machines().machines().await;
                let machine = machines
                    .iter()
                    .find(|machine| machine.node == node)
                    .ok_or(ComputerError::NotOnTailscale)?;
                tailnet
                    .guests()
                    .set(&node, &machine.name, answer, now)
                    .map_err(ComputerError::from)?;
            }
            other => other.map_err(ComputerError::from)?,
        }
        Ok(())
    }

    // Forgets the answer about a computer, and any question it has waiting (Settings only): it
    // is asked about again the next time it wants a model.
    async fn forget_guest(
        &self,
        #[zbus(header)] header: Header<'_>,
        node: String,
    ) -> Result<(), InferError> {
        self.settings_caller(&header).await?;
        let node = NodeId::parse(&node).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        let tailnet = self.tailnet.as_ref().ok_or(ComputerError::Unavailable)?;
        tailnet
            .guests()
            .forget(&node)
            .map_err(ComputerError::from)?;
        Ok(())
    }

    #[zbus(signal)]
    async fn guest_asks(
        emitter: &SignalEmitter<'_>,
        node: &str,
        details: Details,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn guests_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn engines_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    async fn gpu(&self) -> String {
        self.engines.gpu().slug().to_owned()
    }
}

/// The computer `AddComputer` was asked for: each model is its id and a vardict naming how it is
/// reached (`socket` or `port`, one of them) and its `key`, if any. A value of another type is
/// invalid args; two ways to reach it, or none, is a refusal in plain words.
fn new_computer(name: String, models: Vec<(String, Details)>) -> Result<NewComputer, InferError> {
    let invalid = |what: &str| InferError::Bus(fdo::Error::InvalidArgs(what.to_owned()));
    let mut parsed = Vec::new();
    for (id, details) in models {
        let text = |key: &str| -> Result<Option<String>, InferError> {
            details
                .get(key)
                .map(|value| {
                    value
                        .try_clone()
                        .ok()
                        .and_then(|value| String::try_from(value).ok())
                        .ok_or_else(|| invalid("a model's socket and key are text"))
                })
                .transpose()
        };
        let socket = text(porter_dbus::COMPUTER_KEY_SOCKET)?;
        let key = text(porter_dbus::COMPUTER_KEY_KEY)?.map(SecretText::new);
        let port = details
            .get(porter_dbus::COMPUTER_KEY_PORT)
            .map(|value| {
                value
                    .try_clone()
                    .ok()
                    .and_then(|value| u16::try_from(value).ok())
                    .ok_or_else(|| invalid("a model's port is a number from 0 to 65535"))
            })
            .transpose()?;
        let reach = match (socket, port) {
            (Some(path), None) => NewReach::Socket(path.into()),
            (None, Some(port)) => NewReach::Port(port),
            (Some(_), Some(_)) | (None, None) => return Err(ComputerError::BadAddress(id).into()),
        };
        parsed.push(NewModel { id, reach, key });
    }
    Ok(NewComputer {
        label: name,
        models: parsed,
    })
}

fn text_value(text: String) -> Option<porter_dbus::zvariant::OwnedValue> {
    use porter_dbus::zvariant::{OwnedValue, Value};
    OwnedValue::try_from(Value::from(text)).ok()
}

/// A computer that could be added, as a row of `Candidates`: its node id and the vardict of
/// `CANDIDATE_KEY_*`.
fn candidate_row(candidate: Candidate) -> (String, Details) {
    use porter_dbus::zvariant::{OwnedValue, Value};
    let models: Vec<(String, String)> = candidate
        .models
        .into_iter()
        .map(|model| (model.id, model.name))
        .collect();
    let entries = [
        (CANDIDATE_KEY_NAME, text_value(candidate.name)),
        (
            CANDIDATE_KEY_MODELS,
            OwnedValue::try_from(Value::new(models)).ok(),
        ),
        (
            CANDIDATE_KEY_NEEDS_APPROVAL,
            OwnedValue::try_from(Value::Bool(candidate.needs_approval)).ok(),
        ),
    ];
    let details = entries
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect();
    (candidate.node.to_string(), details)
}

/// A computer that asked, as a row of `Guests`: its node id and the vardict of `GUEST_KEY_*`.
fn guest_row(row: porter_tailnet::GuestRow) -> (String, Details) {
    use porter_dbus::zvariant::{OwnedValue, Value};
    let entries = [
        (GUEST_KEY_NAME, text_value(row.name)),
        (GUEST_KEY_STATE, text_value(row.state.slug().to_owned())),
        (
            GUEST_KEY_SINCE,
            OwnedValue::try_from(Value::I64(row.since.0)).ok(),
        ),
    ];
    let details = entries
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect();
    (row.node.to_string(), details)
}

/// What the `GuestAsks` signal carries of a question: the name and when it began.
fn ask_details(ask: &porter_tailnet::Ask) -> Details {
    use porter_dbus::zvariant::{OwnedValue, Value};
    [
        (GUEST_KEY_NAME, text_value(ask.name.clone())),
        (
            GUEST_KEY_SINCE,
            OwnedValue::try_from(Value::I64(ask.since.0)).ok(),
        ),
    ]
    .into_iter()
    .filter_map(|(key, value)| Some((key.to_owned(), value?)))
    .collect()
}

/// A place as a row of `Places`: its id and the vardict of `PLACE_KEY_*`.
fn place_row(row: PlaceRow) -> (String, Details) {
    use porter_dbus::zvariant::{OwnedValue, Value};
    let text = |text: String| OwnedValue::try_from(Value::from(text)).ok();
    let models: Vec<(String, String)> = row
        .models
        .into_iter()
        .map(|model| (model.id.to_string(), model.name))
        .collect();
    let ready = row.state == PlaceState::Ready;
    let entries = [
        (PLACE_KEY_KIND, text(row.kind.slug().to_owned())),
        (PLACE_KEY_NAME, text(row.name)),
        (PLACE_KEY_PROVIDER, row.provider.and_then(text)),
        (
            PLACE_KEY_MODELS,
            OwnedValue::try_from(Value::new(models)).ok(),
        ),
        (
            PLACE_KEY_READY,
            OwnedValue::try_from(Value::Bool(ready)).ok(),
        ),
    ];
    let details = entries
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect();
    (row.id.to_string(), details)
}

/// The slug of a refusal (`requires_cloud`, `unavailable`, ...).
pub fn refusal_slug(refusal: &InferRefusal) -> String {
    slug(refusal, "kind")
}

/// Serves `daemon` on `connection` under the bus name and path, and announces engine changes. The
/// name is claimed once the connection takes calls ([`porter_dbus::serve_ready`]). The daemon
/// itself serves its settings module too: [`serve_with_settings`].
pub async fn serve_on<P, O, C>(
    connection: &zbus::Connection,
    daemon: Inference<P, O, C>,
) -> zbus::Result<()>
where
    P: Peers,
    O: AuditOut + 'static,
    C: Clock + Clone + 'static,
{
    serve_parts(connection, daemon, std::future::ready(Ok(()))).await
}

/// As [`serve_on`], with `settings` served at [`porter_dbus::INFERENCE_SETTINGS_PATH`] before the
/// name is claimed: whoever sees `org.quire.Inference1` owned finds every object of it, the
/// settings module among them.
pub async fn serve_with_settings<P, O, C, S>(
    connection: &zbus::Connection,
    daemon: Inference<P, O, C>,
    settings: crate::settings::InferdSettings<S>,
) -> zbus::Result<()>
where
    P: Peers,
    O: AuditOut + 'static,
    C: Clock + Clone + 'static,
    S: Peers,
{
    let module = crate::settings::serve_settings(connection, settings);
    serve_parts(connection, daemon, module).await
}

/// Registers `daemon`'s objects, then runs `also` (more objects), then claims the name once the
/// connection takes calls, then announces engine changes.
async fn serve_parts<P, O, C>(
    connection: &zbus::Connection,
    daemon: Inference<P, O, C>,
    also: impl std::future::Future<Output = zbus::Result<()>>,
) -> zbus::Result<()>
where
    P: Peers,
    O: AuditOut + 'static,
    C: Clock + Clone + 'static,
{
    let supervised = daemon.engines.supervised().clone();
    let probed = daemon.engines.probed().clone();
    let hosts_cloud = daemon.engines.cloud().is_some();
    let asking = daemon.tailnet.as_ref().map(Tailnet::subscribe);
    let agents = daemon.agents.clone().map(|agents| AgentsService {
        peers: Arc::clone(&daemon.peers),
        agents,
    });
    connection
        .object_server()
        .at(INFERENCE_PATH, daemon)
        .await?;
    if let Some(agents) = agents {
        connection
            .object_server()
            .at(INFERENCE_PATH, agents)
            .await?;
    }
    also.await?;
    // Every object is registered; the name is the promise that calls are taken: claim it only
    // once they are.
    porter_dbus::serve_ready(connection).await?;
    connection.request_name(INFERENCE_BUS).await?;
    let iface: zbus::object_server::InterfaceRef<Inference<P, O, C>> =
        connection.object_server().interface(INFERENCE_PATH).await?;
    tokio::spawn(async move {
        while supervised.changed().await.is_ok() {
            let emitter = iface.signal_emitter();
            let _ = Inference::<P, O, C>::engines_changed(emitter).await;
            let guard = iface.get().await;
            let _ = guard.gpu_changed(emitter).await;
        }
    });
    // A computer of the person's asks to use this one, or an answer changed: the shell and
    // Settings are told.
    if let Some(mut asking) = asking {
        let iface: zbus::object_server::InterfaceRef<Inference<P, O, C>> =
            connection.object_server().interface(INFERENCE_PATH).await?;
        tokio::spawn(async move {
            loop {
                match asking.recv().await {
                    Ok(porter_tailnet::GuestEvent::Asked(ask)) => {
                        let _ = Inference::<P, O, C>::guest_asks(
                            iface.signal_emitter(),
                            ask.node.as_str(),
                            ask_details(&ask),
                        )
                        .await;
                    }
                    // Missed some news: say the answers changed so a listener reads them all.
                    Ok(porter_tailnet::GuestEvent::Changed)
                    | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = Inference::<P, O, C>::guests_changed(iface.signal_emitter()).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }
    // A runtime the person runs came up, went away or changed its models.
    let probed = probed.changed();
    let iface: zbus::object_server::InterfaceRef<Inference<P, O, C>> =
        connection.object_server().interface(INFERENCE_PATH).await?;
    tokio::spawn(async move {
        loop {
            probed.notified().await;
            let _ = Inference::<P, O, C>::engines_changed(iface.signal_emitter()).await;
        }
    });
    // A cloud AI account appeared, went or changed state (accountd tells a joined porter daemon):
    // a listener re-reads `Places`. Only a daemon that serves hosted models has accounts to hear.
    if hosts_cloud {
        let news = Arc::new(tokio::sync::Notify::new());
        crate::account_news::spawn(connection.clone(), Arc::clone(&news));
        let iface: zbus::object_server::InterfaceRef<Inference<P, O, C>> =
            connection.object_server().interface(INFERENCE_PATH).await?;
        tokio::spawn(async move {
            loop {
                news.notified().await;
                let _ = Inference::<P, O, C>::engines_changed(iface.signal_emitter()).await;
            }
        });
    }
    Ok(())
}

mod error;

#[cfg(test)]
mod tests;
