//! A supervisor whose look could not be finished says so, so that its loop looks again in
//! seconds (`next_look`) and not at the next ten-minute rescan: a grant list that does not answer
//! yet on a loaded machine must not leave a granted account unmirrored for ten minutes.

use crate::common::bus::PrivateBus;
use crate::common::hosted::host;
use porter_core::Candidate;
use porter_fake::cloud_provider;
use porter_secrets::MemorySecrets;
use porter_service_core::registry::Registry;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use syncd::datasets::pim::{
    AccountdUnavailable, PimConfig, PimGrants, PimKind, PimSupervisor, Wiring,
};
use syncd::paths::Paths;
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access, Hub};
use tokio::sync::watch;

/// Grants that cannot be asked while `down` is set, and are none otherwise.
struct Flaky {
    down: Arc<AtomicBool>,
}

impl PimGrants for Flaky {
    async fn granted(&self, _kind: PimKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        match self.down.load(Ordering::SeqCst) {
            true => Err(AccountdUnavailable),
            false => Ok(Vec::new()),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_that_could_not_ask_for_grants_says_it_was_not_whole_and_the_next_one_is() {
    let bus = PrivateBus::start();
    let hosted = host(
        cloud_provider(),
        Registry::default(),
        MemorySecrets::default(),
    );
    let root = bus.scratch().join("syncd");
    let paths = Paths {
        journals: root.join("state/porter/sync"),
        mirrors: root.join("data/porter/vdir"),
        callers_system: root.join("none"),
        callers_user: root.join("none"),
        cloud: root.join("Cloud"),
        drive_cache: root.join("cache/porter/drives"),
        settings: root.join("config/porter/syncd/syncd.toml"),
    };
    let (_network_up, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: hosted.accounts,
        hub: Hub::default(),
        paths,
        settings: Settings::quick(),
        network,
        owners: Access::default(),
        clock: porter_sync::Clock::new(porter_ids::clock::SystemClock),
        nonce: "t1".to_owned(),
        millis: Arc::new(syncd::datasets::photos::ManualMillis::at(1_700_000_000_000)),
    };
    let down = Arc::new(AtomicBool::new(true));
    let mut supervisor = PimSupervisor::new(
        wiring,
        Flaky {
            down: Arc::clone(&down),
        },
        PimConfig::default(),
    );
    assert!(
        !supervisor.tick().await,
        "the grants could not be asked: the look is not whole"
    );
    down.store(false, Ordering::SeqCst);
    assert!(supervisor.tick().await, "asked and answered: whole");
}
