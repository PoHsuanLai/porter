//! Registering the Photos dataset: two engines (originals, metadata) over two replicas, each in
//! the hub as `<account>/photos_originals` and `<account>/photos_metadata`, each with its own
//! journal under `<state>/porter/sync/<account>/`.
//!
//! **Off by default.** There is no Photos app yet and no flow that grants syncd a Storage
//! account for photos, so the daemon does not start it; [`start`] is the seam a test, and later
//! the supervisor, calls, and it does nothing unless the [`PhotosSwitch`] is on
//! (`SYNCD_PHOTOS=on` is how a host reads it, [`PhotosSwitch::from_var`]). The replicas are
//! handed in: the caller builds them over relays accountd opens, as `syncd::webdav` does.

use super::library::{PhotoLibrary, PhotosError};
use super::metadata::PhotoMetadata;
use super::originals::PhotoOriginals;
use crate::clock::SystemClock;
use crate::dataset::Dataset;
use crate::driver::Driver;
use crate::engine::Engine;
use crate::journal::Journal;
use crate::paths::{AccountDir, Paths};
use crate::scheduler::{Network, Settings};
use crate::service::{Access, DatasetName, Hub};
use porter_sync::Replica;
use std::sync::Arc;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

/// Whether the daemon runs the Photos dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhotosSwitch {
    /// It does not (the default).
    #[default]
    Off,
    /// It does.
    On,
}

impl PhotosSwitch {
    /// The switch a variable's value (`SYNCD_PHOTOS`) sets: `on`, `1` or `true` turn it on,
    /// anything else, or nothing, leaves it off.
    pub fn from_var(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("on" | "1" | "true") => Self::On,
            _ => Self::Off,
        }
    }
}

/// What a running dataset shares with the daemon.
#[derive(Debug)]
pub struct PhotosWiring {
    /// The running datasets.
    pub hub: Hub,
    /// Where journals and libraries live.
    pub paths: Paths,
    /// The scheduler's settings.
    pub settings: Settings,
    /// The network, as the daemon reads it.
    pub network: watch::Receiver<Network>,
    /// The apps that see the datasets in `Sync1` besides Settings and the porter daemons.
    pub owners: Access,
}

/// A running Photos dataset of one account. Dropping it stops both engines.
#[derive(Debug)]
pub struct PhotosRun {
    library: PhotoLibrary,
    wake: Arc<Notify>,
    tasks: Vec<JoinHandle<()>>,
}

impl PhotosRun {
    /// The library the Photos app imports into and reads from.
    pub fn library(&self) -> &PhotoLibrary {
        &self.library
    }

    /// Tells the engines the library changed (after an import or an edit), so a replica that
    /// declares push is not waited for.
    pub fn changed(&self) {
        self.wake.notify_waiters();
    }
}

impl Drop for PhotosRun {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Why Photos could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The library could not be opened.
    #[error(transparent)]
    Library(#[from] PhotosError),
    /// A journal could not be opened.
    #[error("journal: {0}")]
    Journal(String),
}

/// Registers `engine`'s dataset in the hub and runs its driver.
fn run_one<R: Replica + 'static, D: Dataset + 'static>(
    wiring: &PhotosWiring,
    account: &AccountDir,
    wake: &Arc<Notify>,
    seed: u64,
    engine: Engine<R, D, SystemClock>,
) -> JoinHandle<()> {
    let handle = wiring.hub.register(
        DatasetName {
            account: account.clone(),
            dataset: engine.dataset().id(),
        },
        wiring.owners.clone(),
    );
    let driver = Driver::new(
        engine,
        handle,
        wiring.settings,
        wiring.network.clone(),
        Arc::clone(wake),
        seed,
    );
    tokio::spawn(driver.run())
}

/// Starts the Photos dataset of `account` over `library` and the two replicas (originals,
/// then metadata), registering both in the hub. `None` when the switch is off: nothing is
/// opened, registered or run.
pub fn start<O, M>(
    wiring: &PhotosWiring,
    switch: PhotosSwitch,
    account: &AccountDir,
    library: PhotoLibrary,
    originals: O,
    metadata: M,
) -> Result<Option<PhotosRun>, StartError>
where
    O: Replica + 'static,
    M: Replica + 'static,
{
    if switch == PhotosSwitch::Off {
        return Ok(None);
    }
    let wake = Arc::new(Notify::new());
    let open = |slug: &str| {
        Journal::open(&wiring.paths.journal(account, slug))
            .map_err(|e| StartError::Journal(e.to_string()))
    };
    let originals_journal = open(PhotoOriginals::SLUG)?;
    let metadata_journal = open(PhotoMetadata::SLUG)?;
    let tasks = vec![
        run_one(
            wiring,
            account,
            &wake,
            1,
            Engine::new(
                originals,
                PhotoOriginals::new(library.clone()),
                originals_journal,
                SystemClock,
            ),
        ),
        run_one(
            wiring,
            account,
            &wake,
            2,
            Engine::new(
                metadata,
                PhotoMetadata::new(library.clone()),
                metadata_journal,
                SystemClock,
            ),
        ),
    ];
    Ok(Some(PhotosRun {
        library,
        wake,
        tasks,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_off_unless_a_variable_says_on() {
        const CASES: &[(Option<&str>, PhotosSwitch)] = &[
            (None, PhotosSwitch::Off),
            (Some(""), PhotosSwitch::Off),
            (Some("off"), PhotosSwitch::Off),
            (Some("no"), PhotosSwitch::Off),
            (Some("on"), PhotosSwitch::On),
            (Some(" 1 "), PhotosSwitch::On),
            (Some("true"), PhotosSwitch::On),
        ];
        for (value, expected) in CASES {
            assert_eq!(PhotosSwitch::from_var(*value), *expected, "{value:?}");
        }
        assert_eq!(PhotosSwitch::default(), PhotosSwitch::Off);
    }
}
