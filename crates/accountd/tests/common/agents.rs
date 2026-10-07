//! Agent accounts for the rigs: the accounts a launcher signs in, made from the shipped provider
//! files, as the add flow would store them.
#![allow(dead_code)]

use super::{Rig, SheetHost, storage_account};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AuthKind, Capability, Claim, Offer, Provenance,
    ProviderId, Restriction, Subject,
};
use porter_provider::{ProviderSpec, shipped_specs};

pub fn shipped(id: &str) -> ProviderSpec {
    shipped_specs()
        .into_iter()
        .find(|spec| spec.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} ships"))
}

/// The claims a shipped provider file declares for an agent, as an account would hold them.
pub fn agent_claims(spec: &ProviderSpec) -> Vec<Claim> {
    spec.capabilities
        .iter()
        .filter_map(|row| match &row.capability {
            Capability::Agent(agent) => Some(Claim {
                subject: Subject::Agent(agent.program.clone()),
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            }),
            _ => None,
        })
        .collect()
}

pub fn agent_account(provider: &str, state: AccountState) -> Account {
    let spec = shipped(provider);
    Account {
        id: AccountId::parse(provider).expect("id"),
        provider: ProviderId::parse(provider).expect("provider"),
        label: AccountLabel(spec.label.clone()),
        state,
        auth: AuthKind::AgentLogin,
        capabilities: agent_claims(&spec),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

pub async fn rig_with_an_agent(state: AccountState) -> Rig {
    let accounts = vec![
        storage_account(),
        agent_account("claude-code", state),
        agent_account("codex", state),
    ];
    Rig::start_holding(Default::default(), SheetHost::quiet(), Vec::new(), accounts).await
}
