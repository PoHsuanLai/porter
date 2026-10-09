//! `org.quire.Inference1` at `/org/quire/Inference1`: the daemon's object. `Open` checks the
//! caller (connection to pid to executable to caller), makes a socketpair, hands one end back as
//! the reply descriptor and runs [`serve_session`] on the other, over the four seams: this
//! session's router (which decides once and pins the runner), the shared engines, the session's
//! turns and its audit sink. The introspection of this object is the checked-in
//! `dbus/org.quire.Inference1.xml` (`tests/introspection.rs`).

use crate::agent::Agents;
use crate::agent::service::AgentsService;
use crate::audit::{AuditOut, SessionAudit};
use crate::clock::Clock;
use crate::engines::Engines;
use crate::peers::{Caller, Peers, Role};
use crate::pipeline::Hearing;
use crate::runner::{Pin, Turns};
use crate::serve::{Seams, serve_session};
use crate::session::SessionSpec;
use crate::settings::Reload;
use crate::structured::Limits;
use crate::watch::Probing;
use porter_core::{DataClass, Tier};
use porter_dbus::{
    Details, INFERENCE_BUS, INFERENCE_PATH, NeedArg, PLACE_KEY_KIND, PLACE_KEY_MODELS,
    PLACE_KEY_NAME, PLACE_KEY_PROVIDER, PLACE_KEY_READY, need_from_dbus,
};
use porter_infer::{InferRefusal, PlaceRow, PlaceState};
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
    fn open_session(&self, caller: Caller, spec: SessionSpec) -> fdo::Result<OwnedFd> {
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
            });
        let seams = Seams {
            router: self.engines.router_for(pin, &caller),
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
    ) -> fdo::Result<String> {
        let caller = self.caller(&header).await?;
        let _trace = trace_of(&options);
        let spec = Self::spec(need, &class, "balanced", &options)?;
        let availability = self
            .engines
            .availability_for(&spec.need, spec.class, spec.usage, &caller)
            .await;
        Ok(slug(&availability, "kind"))
    }

    async fn open(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> fdo::Result<OwnedFd> {
        let caller = self.caller(&header).await?;
        let _trace = trace_of(&options);
        let spec = Self::spec(need, &class, &tier, &options)?;
        self.open_session(caller, spec)
    }

    async fn prepare(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> fdo::Result<String> {
        let caller = self.caller(&header).await?;
        let _trace = trace_of(&options);
        let spec = Self::spec(need, &class, &tier, &options)?;
        Ok(
            match self
                .engines
                .prepare_for(&spec.need, spec.class, spec.tier, spec.usage, &caller)
                .await
            {
                Ok(readiness) => readiness.slug().to_owned(),
                Err(refusal) => refusal_slug(&refusal),
            },
        )
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
    // member would change the introspection XML).
    async fn places(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<Vec<(String, Details)>> {
        let caller = self.place_lister(&header).await?;
        Ok(self
            .engines
            .places(&caller)
            .await
            .into_iter()
            .map(place_row)
            .collect())
    }

    #[zbus(signal)]
    async fn engines_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    async fn gpu(&self) -> String {
        self.engines.gpu().slug().to_owned()
    }
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

#[cfg(test)]
mod tests;
