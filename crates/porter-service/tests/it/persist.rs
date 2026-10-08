//! Saving the registry: saves do not overlap, and the last one to finish carries the last state.

use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::store::Persisted;
use porter_core::{
    AccountState, Claim, ModelId, Offer, Provenance, ProviderId, Subject, Tokens, UnixSeconds,
};
use porter_fake::{FixedClock, MemoryStore, ScriptedSheets, cloud_provider, llm_provider};
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, Registry, RegistryStore, StoreError};
use std::sync::Mutex;
use std::time::Duration;

/// A store whose first save is slow, as a busy disk is: a save that began later can finish first.
#[derive(Debug)]
struct SlowFirst {
    inner: MemoryStore,
    calls: Mutex<usize>,
}

impl RegistryStore for SlowFirst {
    async fn load(&self) -> Result<Persisted, StoreError> {
        self.inner.load().await
    }

    async fn save(&self, state: &Persisted) -> Result<(), StoreError> {
        let first = {
            let mut calls = self.calls.lock().expect("calls");
            *calls += 1;
            *calls == 1
        };
        if first {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.inner.save(state).await
    }
}

fn claim(model: &str) -> Claim {
    Claim {
        subject: Subject::Model(ModelId::parse(model).expect("id")),
        offer: Offer::Present(Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(4096),
            max_output: Tokens(4096),
            wire: LlmWire::ChatCompletions,
        })),
        provenance: Provenance::Discovered,
    }
}

/// rel-1: the first save is slow and the second change is saved while it waits. Without the gate
/// the second snapshot is written first and the first (older) one lands on top of it, so the
/// file ends one change behind the registry; with it, the second save waits and takes its
/// snapshot afterwards.
#[tokio::test]
async fn the_last_save_to_finish_carries_the_last_state() {
    let memory = MemoryStore::default();
    let service = AccountService::new(
        vec![llm_provider(), cloud_provider()],
        Registry::default(),
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(UnixSeconds(1_790_000_000)),
    )
    .with_store(SlowFirst {
        inner: memory.clone(),
        calls: Mutex::new(0),
    });
    let runtime = ProviderId::parse("fake-llm").expect("id");
    let (first, second) = tokio::join!(
        service.report_local(&runtime, vec![claim("first-model")], AccountState::Ok),
        service.report_local(&runtime, vec![claim("second-model")], AccountState::Ok),
    );
    first.expect("first");
    second.expect("second");
    assert_eq!(
        memory.stored().expect("saved"),
        service.registry().persisted(),
        "the file is the registry as it is now"
    );
    assert_eq!(memory.saves(), 2);
}
