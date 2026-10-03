//! `org.quire.Inference1` at `/org/quire/Inference1`: the daemon's object. `Open` checks the
//! caller (connection to pid to executable to caller), makes a socketpair, hands one end back as
//! the reply descriptor and runs [`serve_session`] on the other, over the four seams: this
//! session's router (which decides once and pins the runner), the shared engines, the session's
//! turns and its audit sink. The introspection of this object is the checked-in
//! `dbus/org.quire.Inference1.xml` (`tests/introspection.rs`).

use crate::audit::{AuditOut, SessionAudit};
use crate::clock::Clock;
use crate::engines::Engines;
use crate::peers::{Caller, Peers};
use crate::runner::{Pin, Turns};
use crate::serve::{Seams, serve_session};
use crate::session::SessionSpec;
use porter_core::{DataClass, Tier};
use porter_dbus::{Details, INFERENCE_BUS, INFERENCE_PATH, NeedArg, need_from_dbus};
use porter_infer::InferRefusal;
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
    peers: P,
    audit: Arc<O>,
    clock: C,
}

impl<P, O, C> Inference<P, O, C> {
    /// The object over these engines, peers, audit destination and clock.
    pub fn new(engines: Engines, peers: P, audit: O, clock: C) -> Self {
        Self {
            engines,
            peers,
            audit: Arc::new(audit),
            clock,
        }
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

fn failed(why: impl ToString) -> fdo::Error {
    fdo::Error::Failed(why.to_string())
}

impl<P: Peers, O: AuditOut + 'static, C: Clock + Clone + 'static> Inference<P, O, C> {
    async fn caller(&self, header: &Header<'_>) -> fdo::Result<Caller> {
        let sender = header.sender().ok_or_else(unknown_caller)?;
        self.peers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(unknown_caller)
    }

    fn spec(need: NeedArg, class: &str, tier: &str) -> fdo::Result<SessionSpec> {
        Ok(SessionSpec {
            need: need_from_dbus(need).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?,
            class: parse_slug::<DataClass>(class)?,
            tier: parse_slug::<Tier>(tier)?,
        })
    }

    /// Serves one session on a fresh socketpair and returns the client's end.
    fn open_session(&self, caller: Caller, spec: SessionSpec) -> fdo::Result<OwnedFd> {
        let (ours, theirs) = StdStream::pair().map_err(failed)?;
        ours.set_nonblocking(true).map_err(failed)?;
        let stream = UnixStream::from_std(ours).map_err(failed)?;
        let pin = Pin::new();
        let seams = Seams {
            router: self.engines.router(pin.clone(), caller.role),
            engines: self.engines.clone(),
            runner: Turns::new(pin, self.engines.supervised().clone(), spec.tier),
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
        let spec = Self::spec(need, &class, "balanced")?;
        let availability = self
            .engines
            .availability(&spec.need, spec.class, caller.role);
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
        let spec = Self::spec(need, &class, &tier)?;
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
        let spec = Self::spec(need, &class, &tier)?;
        Ok(
            match self
                .engines
                .prepare_as(&spec.need, spec.class, spec.tier, caller.role)
                .await
            {
                Ok(readiness) => readiness.slug().to_owned(),
                Err(refusal) => refusal_slug(&refusal),
            },
        )
    }

    // What the caller used this period. Nothing meters yet, so the dictionary is empty.
    // (Plain comments: a doc comment on a member would change the introspection XML.)
    async fn usage(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<Details> {
        self.caller(&header).await?;
        Ok(Details::new())
    }

    // Looks again: weights that arrived, engines that stopped. Readiness is read live, so this
    // only tells listeners to re-read it.
    async fn rescan(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        self.caller(&header).await?;
        Self::engines_changed(&emitter).await.map_err(failed)
    }

    #[zbus(signal)]
    async fn engines_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    async fn gpu(&self) -> String {
        self.engines.gpu().slug().to_owned()
    }
}

/// The slug of a refusal (`requires_cloud`, `unavailable`, ...).
pub fn refusal_slug(refusal: &InferRefusal) -> String {
    slug(refusal, "kind")
}

/// Serves `daemon` on `connection` under the bus name and path, and announces engine changes.
pub async fn serve_on<P, O, C>(
    connection: &zbus::Connection,
    daemon: Inference<P, O, C>,
) -> zbus::Result<()>
where
    P: Peers,
    O: AuditOut + 'static,
    C: Clock + Clone + 'static,
{
    let supervised = daemon.engines.supervised().clone();
    connection
        .object_server()
        .at(INFERENCE_PATH, daemon)
        .await?;
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
    Ok(())
}

#[cfg(test)]
mod tests;
