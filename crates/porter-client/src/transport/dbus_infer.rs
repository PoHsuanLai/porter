//! inferd on the session bus (feature `infer`): `Inference1.Open` and `Prepare`.

use super::dbus::{DbusTransport, Rows, bus_error, slug};
use super::dbus_session::DbusSession;
use crate::error::TransportError;
use porter_core::{DataClass, ModelId, Need, Permille, Tier};
use porter_dbus::zvariant::{OwnedValue, Value};
use porter_dbus::{
    Details, InferenceProxy, OPTION_PLACE_MODELS, OPTION_PLACES, OPTION_TRACEPARENT, OPTION_USAGE,
    PLACE_KEY_KIND, PLACE_KEY_MODELS, PLACE_KEY_NAME, PLACE_KEY_PROVIDER, PLACE_KEY_READY,
    need_to_dbus,
};
use porter_infer::{OpenOptions, PlaceId, PlaceKind, PlaceModel, PlaceRow, PlaceState, Readiness};

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
    let mut details: Details = trace
        .chain(usage)
        .filter_map(|(key, text)| {
            let value = OwnedValue::try_from(Value::from(text)).ok()?;
            Some((key.to_owned(), value))
        })
        .collect();
    // The places the call may run, in order, and the model to use at some of them.
    if let Some(places) = &options.places {
        let texts: Vec<String> = places.iter().map(|place| place.to_string()).collect();
        details.extend(
            OwnedValue::try_from(Value::new(texts))
                .ok()
                .map(|value| (OPTION_PLACES.to_owned(), value)),
        );
    }
    if !options.place_models.is_empty() {
        let pins: std::collections::HashMap<String, String> = options
            .place_models
            .iter()
            .map(|(place, model)| (place.to_string(), model.to_string()))
            .collect();
        details.extend(
            OwnedValue::try_from(Value::new(pins))
                .ok()
                .map(|value| (OPTION_PLACE_MODELS.to_owned(), value)),
        );
    }
    details
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

impl DbusTransport {
    /// `Inference1.Places` row by row: a row that is malformed is in `skipped`, the rest are
    /// `rows` (`Transport::places` gives the rows alone).
    pub async fn list_places(&self) -> Result<Rows<PlaceRow>, TransportError> {
        let proxy = InferenceProxy::new(self.connection())
            .await
            .map_err(|e| bus_error(&e))?;
        let rows = proxy.places().await.map_err(|e| bus_error(&e))?;
        Ok(Rows::decode(rows, place_row))
    }
}

/// One row of `Places` as a [`PlaceRow`]; a row that does not say what the interface promises is
/// malformed.
fn place_row((id, details): (String, Details)) -> Result<PlaceRow, TransportError> {
    let bad = |what: &str| TransportError::Malformed(format!("Places row {id:?}: {what}"));
    let text = |key: &str| -> Option<String> {
        String::try_from(details.get(key)?.try_clone().ok()?).ok()
    };
    let place = PlaceId::parse(&id).map_err(|_| bad("not a place id"))?;
    let kind = text(PLACE_KEY_KIND)
        .and_then(|slug| PlaceKind::from_slug(&slug))
        .ok_or_else(|| bad("no kind"))?;
    let models: Vec<(String, String)> = details
        .get(PLACE_KEY_MODELS)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, String)>::try_from(value).ok())
        .ok_or_else(|| bad("no models"))?;
    let models = models
        .into_iter()
        .map(|(model, name)| {
            Ok(PlaceModel {
                id: ModelId::parse(&model).map_err(|_| bad("a model id"))?,
                name,
            })
        })
        .collect::<Result<Vec<_>, TransportError>>()?;
    let ready = details
        .get(PLACE_KEY_READY)
        .and_then(|value| bool::try_from(value).ok())
        .ok_or_else(|| bad("no ready"))?;
    Ok(PlaceRow {
        kind,
        name: text(PLACE_KEY_NAME).ok_or_else(|| bad("no name"))?,
        provider: text(PLACE_KEY_PROVIDER),
        models,
        state: if ready {
            PlaceState::Ready
        } else {
            PlaceState::NotReady
        },
        id: place,
    })
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
