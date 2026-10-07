//! The local model an attached engine is: the catalogue's entry for the id (so its codec, tool
//! and reasoning parser, limits and sampling are the catalogue's, through the same bridge as any
//! engine), a card that says where the data goes, and a spec no supervisor is given.

use super::config::{Attached, AttachedError};
use super::target::Target;
use crate::catalog::claims_of;
use crate::local::{LOCAL_ACCOUNT, LocalModel, flavor_of};
use engine_supervisor::{EngineId, EnginePaths, EngineSpec, ProgramPath, SocketPath, command};
use model_catalog::{EngineProfile, MiB, ModelEntry, Serving, WeightFiles};
use model_provider::ModelName;
use porter_core::{AccountId, Billing, Capability, Offer};
use porter_infer::ModelCard;
use std::path::{Path, PathBuf};

/// The engine id of an attached model: it names no process.
pub fn engine_id(id: &str) -> EngineId {
    EngineId(format!("attached:{id}"))
}

/// The local model of `attached`, over `entries` (the catalogue). `sockets` is where an engine's
/// socket would go; it is used only when the attachment names no socket of its own.
pub fn local_model(
    attached: &Attached,
    entries: &[ModelEntry],
    sockets: &Path,
) -> Result<LocalModel, AttachedError> {
    let name = attached.id.as_str();
    let entry = entries
        .iter()
        .find(|entry| entry.id.0 == name)
        .ok_or_else(|| AttachedError::NotInCatalogue {
            id: name.to_owned(),
        })?;
    let capabilities: Vec<Capability> = claims_of(entry)
        .into_iter()
        .filter_map(|claim| match claim.offer {
            Offer::Present(capability) => Some(capability),
            Offer::Absent { .. } => None,
        })
        .collect();
    if capabilities.is_empty() {
        return Err(AttachedError::NoCapability {
            id: name.to_owned(),
        });
    }
    let no_chat = || AttachedError::NoChatEngine {
        id: name.to_owned(),
    };
    // Only an entry the catalogue says is served by an engine somebody else started can be
    // attached; one inferd launches itself cannot.
    let Serving::Attached(served) = &entry.serving else {
        return Err(AttachedError::NotAttachable {
            id: name.to_owned(),
        });
    };
    let flavor = flavor_of(served.engine).ok_or_else(no_chat)?;
    // An attached entry has no engine profile (nothing is launched): the one made here names the
    // wire and is never run.
    let profile = EngineProfile {
        kind: served.engine,
        args: Vec::new(),
        weights: WeightFiles::HfSnapshot,
        inputs: None,
        outputs: None,
    };
    let target = Target::of(attached);
    let socket = SocketPath(match target.socket() {
        Some(path) => path.clone(),
        None => sockets.join(format!("attached-{name}.sock")),
    });
    let nothing = EnginePaths {
        vllm_python: ProgramPath(PathBuf::new()),
        llama_server: ProgramPath(PathBuf::new()),
        speech_host: ProgramPath(PathBuf::new()),
        kokoro_python: ProgramPath(PathBuf::new()),
        hf_cache: PathBuf::new(),
    };
    Ok(LocalModel {
        card: ModelCard {
            account: AccountId::parse(LOCAL_ACCOUNT).map_err(|_| no_chat())?,
            model: attached.id.clone(),
            locality: attached.place.locality(),
            billing: Billing::Free,
            capabilities,
        },
        // The name the catalogue says the engine serves the model under, which a request names and
        // `/v1/models` must list.
        name: ModelName(served.served_name.0.clone()),
        spec: EngineSpec {
            id: engine_id(name),
            need: MiB(0),
            unit: command(entry, &profile, &nothing, &socket),
        },
        socket,
        flavor: Some(flavor),
        cassette: None,
        loopback: None,
        attached: Some(target),
        entry: entry.clone(),
        profile,
    })
}

#[cfg(test)]
mod tests;
