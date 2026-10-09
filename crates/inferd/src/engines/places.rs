//! The places the assistant could run, as `Inference1.Places` lists them: this computer, each of
//! the person's own computers (attached engines that share a `computer` name), and each cloud
//! AI account accountd holds. A place's models are the language models it can serve now.

use super::*;
use crate::cloud::models::{known_providers, provider_of, remote_models};
use crate::router::fits;
use porter_core::capability::LlmFeature;
use porter_core::need::LlmNeed;
use porter_core::{AccountState, ModelId, Tokens};
use porter_infer::{ComputerName, PlaceId, PlaceModel, PlaceRow, PlaceState, PlaceTarget};
use std::collections::BTreeMap;

/// Whether a model in this state can take a request now: it is up, coming up, or a request
/// starts it. A model still to be downloaded, or whose engine failed, cannot.
pub(crate) fn serves_now(readiness: Readiness) -> bool {
    matches!(
        readiness,
        Readiness::Ready | Readiness::Loading | Readiness::Loadable
    )
}

/// What the assistant asks of a place: to chat.
fn chat_need() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(0),
    })
}

fn state_of(serving: bool) -> PlaceState {
    if serving {
        PlaceState::Ready
    } else {
        PlaceState::NotReady
    }
}

/// An account in these states can be used; the rest are waiting for the person.
pub(crate) fn usable(state: Option<AccountState>) -> bool {
    matches!(state, None | Some(AccountState::Ok | AccountState::Limited))
}

impl Engines {
    /// The computer an attached engine runs on: the one its entry names, else `other-computer`.
    fn computer_of(&self, model: &ModelRef) -> ComputerName {
        self.book
            .attached
            .find(model)
            .and_then(|local| local.attached.as_ref()?.computer.clone())
            .unwrap_or_else(ComputerName::other)
    }

    /// The language models this computer lends to the person's other computers: the ones that
    /// run on this computer (an engine inferd runs, a runtime the person runs here, an engine
    /// they attached that is here) and can answer now. Never a cloud account's model, never a
    /// model on another computer, whether the person added it or attached it: a computer that
    /// asks is not sent onward, and this is not a setting. Engines attached here are looked at
    /// first (and only those: looking at the others would ask the computers that may be asking
    /// us). Model id and the name a person reads.
    pub async fn lendable(&self) -> Vec<(ModelId, String)> {
        for local in self.book.attached.models() {
            if local.card.locality == Locality::OnDevice {
                let _ = self.book.attached.reprobe(&local.model_ref()).await;
            }
        }
        let none = Offered::default();
        let chat = chat_need();
        self.listed_with(&none)
            .into_iter()
            .filter(|one| {
                one.card.locality == Locality::OnDevice
                    && fits(&chat, &one.card)
                    && serves_now(one.readiness)
            })
            .map(|one| {
                let model = ModelRef {
                    account: one.card.account.clone(),
                    model: one.card.model.clone(),
                };
                let name = self
                    .label_of(&model, &none)
                    .map_or_else(|| one.card.model.to_string(), |label| label.0);
                (one.card.model, name)
            })
            .collect()
    }

    /// The place a model is served from.
    pub(crate) fn place_of(&self, card: &ModelCard) -> PlaceId {
        self.place_for(&card.locality, &card.account, &card.model)
    }

    /// The place a model at `locality` under `account` is served from.
    pub(crate) fn place_for(
        &self,
        locality: &Locality,
        account: &porter_core::AccountId,
        model: &ModelId,
    ) -> PlaceId {
        match locality {
            Locality::OnDevice => PlaceId::this_computer(),
            Locality::LocalNetwork => PlaceId::computer(&self.computer_of(&ModelRef {
                account: account.clone(),
                model: model.clone(),
            })),
            Locality::Cloud { .. } => PlaceId::account(account),
        }
    }

    /// Every place known now, one row each: this computer first, then the person's own computers
    /// by name, then the cloud accounts in accountd's order. Attached engines are looked at first,
    /// so what a row says of them is current. The asking app is only what accountd is asked the
    /// accounts for; no grant is read from the answer.
    pub async fn places(&self, caller: &Caller) -> Vec<PlaceRow> {
        self.look_at_attached().await;
        let none = Offered::default();
        let chat = chat_need();
        let models_of = |listed: &[&Listed]| -> Vec<PlaceModel> {
            listed
                .iter()
                .filter(|one| fits(&chat, &one.card) && serves_now(one.readiness))
                .map(|one| {
                    let model = ModelRef {
                        account: one.card.account.clone(),
                        model: one.card.model.clone(),
                    };
                    PlaceModel {
                        id: one.card.model.clone(),
                        name: self
                            .label_of(&model, &none)
                            .map_or_else(|| one.card.model.to_string(), |label| label.0),
                    }
                })
                .collect()
        };
        let listed = self.listed_with(&none);
        let here: Vec<&Listed> = listed
            .iter()
            .filter(|one| one.card.locality == Locality::OnDevice)
            .collect();
        let mut rows = vec![PlaceRow {
            id: PlaceId::this_computer(),
            kind: porter_infer::PlaceKind::ThisComputer,
            name: "This computer".to_owned(),
            provider: None,
            models: models_of(&here),
            state: PlaceState::Ready,
        }];
        let mut computers: BTreeMap<PlaceId, Vec<&Listed>> = BTreeMap::new();
        for one in listed
            .iter()
            .filter(|one| one.card.locality == Locality::LocalNetwork)
        {
            computers
                .entry(self.place_of(&one.card))
                .or_default()
                .push(one);
        }
        for (id, members) in computers {
            let models = models_of(&members);
            rows.push(PlaceRow {
                name: match id.target() {
                    PlaceTarget::OwnComputer(name) => self
                        .book
                        .attached
                        .label_of(&name)
                        .unwrap_or_else(|| name.to_string()),
                    PlaceTarget::ThisComputer | PlaceTarget::CloudAccount(_) => id.to_string(),
                },
                id,
                kind: porter_infer::PlaceKind::OwnComputer,
                provider: None,
                state: state_of(!models.is_empty()),
                models,
            });
        }
        rows.extend(self.account_rows(caller, &chat).await);
        rows
    }

    /// The cloud AI accounts accountd holds, as places: the accounts that reach a hosted model of
    /// the catalogue, with the models each serves while it is in a state to be used.
    async fn account_rows(&self, caller: &Caller, chat: &Need) -> Vec<PlaceRow> {
        let Some(cloud) = &self.book.cloud else {
            return Vec::new();
        };
        let known = known_providers(cloud.entries());
        let accounts = cloud
            .accounts(&caller.app, DataClass::Prompt, Usage::Interactive)
            .await;
        accounts
            .into_iter()
            .filter_map(|account| {
                let provider = provider_of(&account, &known)?;
                let working = usable(account.state);
                let models: Vec<PlaceModel> = if working {
                    remote_models(cloud.entries(), std::slice::from_ref(&account))
                        .into_iter()
                        .filter(|one| fits(chat, &one.card))
                        .filter_map(|one| {
                            Some(PlaceModel {
                                id: ModelId::parse(&one.entry.id.0).ok()?,
                                name: one.entry.label.clone(),
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Some(PlaceRow {
                    id: PlaceId::account(&account.account),
                    kind: porter_infer::PlaceKind::CloudAccount,
                    name: account
                        .label
                        .clone()
                        .unwrap_or_else(|| account.account.to_string()),
                    provider: Some(
                        account
                            .provider_label
                            .clone()
                            .unwrap_or_else(|| provider.0.clone()),
                    ),
                    models,
                    state: state_of(working),
                })
            })
            .collect()
    }
}
