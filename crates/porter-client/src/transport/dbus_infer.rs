//! inferd on the session bus (feature `infer`): `Inference1.Open` and `Prepare`.

use super::dbus::{DbusTransport, bus_error, slug};
use super::dbus_session::DbusSession;
use crate::error::TransportError;
use porter_core::{DataClass, Need, Permille, Tier};
use porter_dbus::zvariant::{OwnedValue, Value};
use porter_dbus::{Details, InferenceProxy, OPTION_TRACEPARENT, OPTION_USAGE, need_to_dbus};
use porter_infer::{OpenOptions, Readiness};

/// The `options` dictionary of `Open`: the reserved `traceparent` and `usage` when the caller has
/// them (`usage` as its slug).
fn details(options: &OpenOptions) -> Details {
    let trace = options
        .traceparent
        .iter()
        .map(|trace| (OPTION_TRACEPARENT, trace.as_str().to_owned()));
    let usage = options
        .usage
        .iter()
        .filter_map(|usage| Some((OPTION_USAGE, slug(usage).ok()?)));
    trace
        .chain(usage)
        .filter_map(|(key, text)| {
            let value = OwnedValue::try_from(Value::from(text)).ok()?;
            Some((key.to_owned(), value))
        })
        .collect()
}

impl DbusTransport {
    pub(super) async fn open_session(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<DbusSession, TransportError> {
        let proxy = InferenceProxy::new(self.connection())
            .await
            .map_err(|e| bus_error(&e))?;
        let fd = proxy
            .open(
                &need_to_dbus(need),
                &slug(&class)?,
                &slug(&tier)?,
                &details(options),
            )
            .await
            .map_err(|e| bus_error(&e))?;
        DbusSession::over(fd.into())
    }

    pub(super) async fn prepare_engine(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<Readiness, TransportError> {
        let proxy = InferenceProxy::new(self.connection())
            .await
            .map_err(|e| bus_error(&e))?;
        let text = proxy
            .prepare(
                &need_to_dbus(need),
                &slug(&class)?,
                &slug(&tier)?,
                &details(options),
            )
            .await
            .map_err(|e| bus_error(&e))?;
        readiness_of(&text)
    }
}

/// The readiness a `Prepare` slug names. `downloading` carries no progress on the bus (the slug
/// is all there is), so it reads as zero; `unavailable` is also the refusal of that name; every
/// other refusal slug is the daemon saying no.
fn readiness_of(text: &str) -> Result<Readiness, TransportError> {
    match text {
        "ready" => Ok(Readiness::Ready),
        "loading" => Ok(Readiness::Loading),
        "loadable" => Ok(Readiness::Loadable),
        "downloading" => Ok(Readiness::Downloading(Permille(0))),
        "downloadable" => Ok(Readiness::Downloadable),
        "unavailable" => Ok(Readiness::Unavailable),
        "" => Err(TransportError::Malformed("empty Prepare answer".to_owned())),
        refusal => Err(TransportError::Denied(format!("inferd refused: {refusal}"))),
    }
}
