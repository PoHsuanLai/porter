//! inferd on the session bus (feature `infer`): `Inference1.Open` and `Prepare`.

use super::dbus::{DbusTransport, Rows, bus_error, slug};
use super::dbus_session::DbusSession;
use crate::error::TransportError;
use porter_capability::ModelId;
use porter_core::{DataClass, Need, Tier};
use porter_dbus::names::{
    OPTION_ASKER, OPTION_MODEL, OPTION_PLACE_MODELS, OPTION_PLACES, OPTION_TRACEPARENT,
    OPTION_USAGE, PLACE_KEY_ENGINES, PLACE_KEY_KIND, PLACE_KEY_MODELS, PLACE_KEY_NAME,
    PLACE_KEY_PROVIDER, PLACE_KEY_READY, PLACE_KEY_TAKES,
};
use porter_dbus::zvariant::{OwnedValue, Value};
use porter_dbus::{Details, InferenceProxy, need_to_dbus};
use porter_ids::Permille;
use porter_infer::{
    OpenOptions, PlaceEngine, PlaceId, PlaceKind, PlaceModel, PlaceRow, PlaceState, Readiness,
    Takes,
};

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
    let asker = options
        .asker
        .iter()
        .map(|asker| (OPTION_ASKER, asker.to_text()));
    let model = options
        .model
        .iter()
        .map(|model| (OPTION_MODEL, model.to_string()));
    let mut details: Details = trace
        .chain(usage)
        .chain(asker)
        .chain(model)
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
        let proxy = self
            .inference()
            .proxy::<InferenceProxy<'static>>(self.connection())
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
        let proxy = self
            .inference()
            .proxy::<InferenceProxy<'static>>(self.connection())
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
        let proxy = self
            .inference()
            .proxy::<InferenceProxy<'static>>(self.connection())
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
    // Absent from an older inferd and for a model whose engine is not known.
    let engines: Vec<(String, String, String)> = details
        .get(PLACE_KEY_ENGINES)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, String, String)>::try_from(value).ok())
        .unwrap_or_default();
    // Absent from an older inferd and for a model whose input is not known.
    let takes: Vec<(String, String)> = details
        .get(PLACE_KEY_TAKES)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, String)>::try_from(value).ok())
        .unwrap_or_default();
    let models = models
        .into_iter()
        .map(|(model, name)| {
            let takes = takes
                .iter()
                .find(|(id, _)| *id == model)
                .map_or(Takes::Unknown, |(_, slug)| Takes::from_slug(slug));
            let engine = engines
                .iter()
                .find(|(id, _, _)| *id == model)
                .map(|(_, id, name)| PlaceEngine {
                    id: id.clone(),
                    name: name.clone(),
                });
            let placed =
                PlaceModel::new(ModelId::parse(&model).map_err(|_| bad("a model id"))?, name)
                    .with_takes(takes);
            Ok(match engine {
                Some(engine) => placed.with_engine(engine),
                None => placed,
            })
        })
        .collect::<Result<Vec<_>, TransportError>>()?;
    let ready = details
        .get(PLACE_KEY_READY)
        .and_then(|value| bool::try_from(value).ok())
        .ok_or_else(|| bad("no ready"))?;
    let name = text(PLACE_KEY_NAME).ok_or_else(|| bad("no name"))?;
    let state = if ready {
        PlaceState::Ready
    } else {
        PlaceState::NotReady
    };
    let row = PlaceRow::new(place, kind, name, state).with_models(models);
    Ok(match text(PLACE_KEY_PROVIDER) {
        Some(provider) => row.with_provider(provider),
        None => row,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn value<T: Into<Value<'static>>>(inner: T) -> OwnedValue {
        OwnedValue::try_from(inner.into()).expect("value")
    }

    fn row_details(engines: Option<Vec<(String, String, String)>>) -> Details {
        let models = vec![
            ("qwen".to_owned(), "Qwen".to_owned()),
            ("gemma".to_owned(), "Gemma".to_owned()),
        ];
        let mut details = Details::new();
        details.insert(PLACE_KEY_KIND.to_owned(), value("this_computer"));
        details.insert(PLACE_KEY_NAME.to_owned(), value("This computer"));
        details.insert(PLACE_KEY_READY.to_owned(), value(true));
        details.insert(
            PLACE_KEY_MODELS.to_owned(),
            OwnedValue::try_from(Value::new(models)).expect("models"),
        );
        if let Some(engines) = engines {
            details.insert(
                PLACE_KEY_ENGINES.to_owned(),
                OwnedValue::try_from(Value::new(engines)).expect("engines"),
            );
        }
        details
    }

    #[test]
    fn a_row_without_engines_parses_with_no_engine_on_any_model() {
        let row = place_row(("this-computer".to_owned(), row_details(None))).expect("row");
        assert_eq!(row.models.len(), 2);
        assert!(row.models.iter().all(|model| model.engine.is_none()));
    }

    #[test]
    fn a_row_with_engines_names_the_engine_of_the_models_it_lists() {
        let engines = vec![(
            "qwen".to_owned(),
            "llama_cpp".to_owned(),
            "llama.cpp".to_owned(),
        )];
        let row = place_row(("this-computer".to_owned(), row_details(Some(engines)))).expect("row");
        let engine = row.models[0].engine.as_ref().expect("engine");
        assert_eq!(
            (engine.id.as_str(), engine.name.as_str()),
            ("llama_cpp", "llama.cpp")
        );
        assert_eq!(row.models[1].engine, None);
    }

    #[test]
    fn a_row_says_what_each_model_takes_in_and_an_older_row_says_nothing() {
        let mut details = row_details(None);
        let takes = vec![("qwen".to_owned(), "text_and_images".to_owned())];
        details.insert(
            PLACE_KEY_TAKES.to_owned(),
            OwnedValue::try_from(Value::new(takes)).expect("takes"),
        );
        let row = place_row(("this-computer".to_owned(), details)).expect("row");
        assert_eq!(row.models[0].takes, Takes::TextAndImages);
        assert_eq!(row.models[1].takes, Takes::Unknown);
    }
}
