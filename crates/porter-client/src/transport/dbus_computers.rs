//! The computers and guests of inferd on the session bus (`Inference1.Guests`, `AnswerGuest`,
//! `ForgetGuest`, `Candidates`, `AddTailnetComputer`, `AddComputer`, `RemoveComputer`): each
//! row decoded into a typed one. A row that does not say what the interface promises is left out
//! of the list and named in [`Rows::skipped`], never a panic and never the whole call failing.

use super::dbus::{DbusTransport, Rows, bus_error};
use crate::error::TransportError;
use porter_core::lending::{
    Approval, CandidateModel, ComputerCandidate, GuestAnswer, GuestRow, RowState,
};
use porter_core::{NodeId, UnixSeconds};
use porter_dbus::{
    CANDIDATE_KEY_MODELS, CANDIDATE_KEY_NAME, CANDIDATE_KEY_NEEDS_APPROVAL, Details,
    GUEST_KEY_NAME, GUEST_KEY_SINCE, GUEST_KEY_STATE, InferenceProxy,
};

impl DbusTransport {
    async fn inference(&self) -> Result<InferenceProxy<'static>, TransportError> {
        InferenceProxy::new(self.connection())
            .await
            .map_err(|e| bus_error(&e))
    }

    /// `Inference1.Guests` row by row: a row that is malformed is in `skipped`, the rest are
    /// `rows` (`Transport::guests` gives the rows alone).
    pub async fn list_guests(&self) -> Result<Rows<GuestRow>, TransportError> {
        let rows = self
            .inference()
            .await?
            .guests()
            .await
            .map_err(|e| bus_error(&e))?;
        Ok(Rows::decode(rows, guest_row))
    }

    pub(super) async fn send_guest_answer(
        &self,
        node: &NodeId,
        answer: GuestAnswer,
    ) -> Result<(), TransportError> {
        self.inference()
            .await?
            .answer_guest(node.as_str(), answer.slug())
            .await
            .map_err(|e| bus_error(&e))
    }

    pub(super) async fn send_forget_guest(&self, node: &NodeId) -> Result<(), TransportError> {
        self.inference()
            .await?
            .forget_guest(node.as_str())
            .await
            .map_err(|e| bus_error(&e))
    }

    /// `Inference1.Candidates` row by row: a row that is malformed is in `skipped`, the rest are
    /// `rows` (`Transport::candidates` gives the rows alone).
    pub async fn list_candidates(&self) -> Result<Rows<ComputerCandidate>, TransportError> {
        let rows = self
            .inference()
            .await?
            .candidates()
            .await
            .map_err(|e| bus_error(&e))?;
        Ok(Rows::decode(rows, candidate_row))
    }
}

#[cfg(feature = "infer")]
mod computers {
    use super::*;
    use crate::computers::{ComputerReach, NewComputer};
    use porter_dbus::zvariant::{OwnedValue, Value};
    use porter_dbus::{COMPUTER_KEY_KEY, COMPUTER_KEY_PORT, COMPUTER_KEY_SOCKET};
    use porter_infer::{ComputerName, PlaceId};

    impl DbusTransport {
        pub(in crate::transport) async fn send_add_tailnet_computer(
            &self,
            node: &NodeId,
        ) -> Result<PlaceId, TransportError> {
            let place = self
                .inference()
                .await?
                .add_tailnet_computer(node.as_str())
                .await
                .map_err(|e| bus_error(&e))?;
            place_of(&place)
        }

        pub(in crate::transport) async fn send_add_computer(
            &self,
            computer: &NewComputer,
        ) -> Result<PlaceId, TransportError> {
            let models: Vec<(String, Details)> = computer
                .models
                .iter()
                .map(|model| {
                    let reach = match &model.reach {
                        ComputerReach::Socket(path) => (
                            COMPUTER_KEY_SOCKET,
                            Value::from(path.to_string_lossy().into_owned()),
                        ),
                        ComputerReach::Port(port) => (COMPUTER_KEY_PORT, Value::U16(*port)),
                    };
                    let key = model
                        .key
                        .iter()
                        .map(|key| (COMPUTER_KEY_KEY, Value::from(key.expose().to_owned())));
                    let details = std::iter::once(reach)
                        .chain(key)
                        .filter_map(|(name, value)| {
                            Some((name.to_owned(), OwnedValue::try_from(value).ok()?))
                        })
                        .collect();
                    (model.model.to_string(), details)
                })
                .collect();
            let place = self
                .inference()
                .await?
                .add_computer(&computer.name, models)
                .await
                .map_err(|e| bus_error(&e))?;
            place_of(&place)
        }

        pub(in crate::transport) async fn send_remove_computer(
            &self,
            name: &ComputerName,
        ) -> Result<(), TransportError> {
            self.inference()
                .await?
                .remove_computer(name.as_str())
                .await
                .map_err(|e| bus_error(&e))
        }
    }

    fn place_of(text: &str) -> Result<PlaceId, TransportError> {
        PlaceId::parse(text)
            .map_err(|_| TransportError::Malformed(format!("a place id was expected: {text:?}")))
    }
}

fn text(details: &Details, key: &str) -> Option<String> {
    String::try_from(details.get(key)?.try_clone().ok()?).ok()
}

/// One row of `Guests` as a [`GuestRow`].
fn guest_row((id, details): (String, Details)) -> Result<GuestRow, TransportError> {
    let bad = |what: &str| TransportError::Malformed(format!("Guests row {id:?}: {what}"));
    Ok(GuestRow {
        node: NodeId::parse(&id).map_err(|_| bad("not a node id"))?,
        name: text(&details, GUEST_KEY_NAME).ok_or_else(|| bad("no name"))?,
        state: text(&details, GUEST_KEY_STATE)
            .and_then(|slug| RowState::from_slug(&slug))
            .ok_or_else(|| bad("no state"))?,
        since: details
            .get(GUEST_KEY_SINCE)
            .and_then(|value| i64::try_from(value).ok())
            .map(UnixSeconds)
            .ok_or_else(|| bad("no since"))?,
    })
}

/// One row of `Candidates` as a [`ComputerCandidate`].
fn candidate_row((id, details): (String, Details)) -> Result<ComputerCandidate, TransportError> {
    let bad = |what: &str| TransportError::Malformed(format!("Candidates row {id:?}: {what}"));
    let models: Vec<(String, String)> = details
        .get(CANDIDATE_KEY_MODELS)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, String)>::try_from(value).ok())
        .ok_or_else(|| bad("no models"))?;
    let needs_approval = details
        .get(CANDIDATE_KEY_NEEDS_APPROVAL)
        .and_then(|value| bool::try_from(value).ok())
        .ok_or_else(|| bad("no needs_approval"))?;
    Ok(ComputerCandidate::new(
        NodeId::parse(&id).map_err(|_| bad("not a node id"))?,
        text(&details, CANDIDATE_KEY_NAME).ok_or_else(|| bad("no name"))?,
        models
            .into_iter()
            .map(|(id, name)| CandidateModel::new(id, name))
            .collect(),
        if needs_approval {
            Approval::Needed
        } else {
            Approval::Given
        },
    ))
}
