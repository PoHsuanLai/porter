//! A local runtime as an account (PLAN G3, design/31 §3.3): inferd probes Ollama, llama.cpp and
//! LM Studio and reports each through `Peer.ReportLocal`. There is no sign-in and no secret: the
//! account is made from the runtime's provider file the first time it is reported, takes its
//! models as `Discovered` claims, and goes `Offline` when the runtime stops. It is never deleted
//! for that (removing it is the person's, in Settings).

use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AuthKind, Claim, KindToggle, Offer, Provenance,
    ProviderId, Restriction, Subject, effective,
};
use porter_provider::Provider;
use porter_secrets::Secrets;

/// The most claims one report may carry (one runtime lists at most this many models).
pub const MAX_LOCAL_CLAIMS: usize = 256;

/// Why a report was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalFault {
    /// No provider file has this id.
    UnknownProvider,
    /// The provider is not a local runtime: it signs in, and only a sign-in makes its account.
    NotLocal,
    /// A runtime is `ok` or `offline`; nothing else is reported.
    State,
    /// A claim is not one a runtime may make: a model's, present, from a probe.
    Claim,
    /// The registry could not be saved; the change stays in memory.
    Unavailable,
}

/// Whether a reported claim is one a runtime may make: about a model, present, `Discovered` (a
/// probe cannot declare, curate or settle by a real call what a provider file or a person did).
fn claimable(claim: &Claim) -> bool {
    matches!(claim.subject, Subject::Model(_))
        && matches!(claim.offer, Offer::Present(_))
        && claim.provenance == Provenance::Discovered
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Takes what a probe found: the account of `provider` (its id is the provider's, one per
    /// runtime) is made if it is not there, holds `models` as its claims, and is in `state`.
    ///
    /// An `offline` report with no models keeps the models the account had, so a stopped runtime
    /// still lists what it ran and comes back as it was. An `ok` report replaces them.
    pub async fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> Result<AccountId, LocalFault> {
        if !matches!(state, AccountState::Ok | AccountState::Offline) {
            return Err(LocalFault::State);
        }
        if models.len() > MAX_LOCAL_CLAIMS || !models.iter().all(claimable) {
            return Err(LocalFault::Claim);
        }
        let spec = self
            .catalog
            .get(provider)
            .ok_or(LocalFault::UnknownProvider)?;
        if spec.auth.kind != AuthKind::LocalRuntime {
            return Err(LocalFault::NotLocal);
        }
        let id = AccountId::parse(provider.as_str()).map_err(|_| LocalFault::UnknownProvider)?;
        {
            let mut registry = self.lock();
            let off: Vec<KindToggle> = registry
                .toggles
                .iter()
                .filter(|t| t.account == id)
                .map(|t| KindToggle {
                    kind: t.kind,
                    toggle: t.toggle,
                })
                .collect();
            let held = registry.accounts.iter_mut().find(|a| a.id == id);
            match held {
                Some(account) => {
                    account.state = state;
                    if !(models.is_empty() && state == AccountState::Offline) {
                        account.capabilities = effective(&models, &off);
                    }
                }
                None => registry.accounts.push(Account {
                    id: id.clone(),
                    provider: spec.id.clone(),
                    label: AccountLabel(spec.label.clone()),
                    state,
                    auth: spec.auth.kind,
                    capabilities: effective(&models, &off),
                    restriction: Restriction::none(),
                    endpoints: Vec::new(),
                }),
            }
        }
        self.persist().await.map_err(|_| LocalFault::Unavailable)?;
        Ok(id)
    }
}
