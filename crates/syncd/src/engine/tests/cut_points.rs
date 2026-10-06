//! Resumability: a crash between any two journal writes leaves a journal the next run finishes.
//! The scenario runs once cleanly to count its writes and learn the final state, then once per
//! cut point: the process dies before that write, a fresh engine opens the same file, and the
//! sides must converge to the same state.

use super::rig::*;
use crate::engine::SyncError;
use crate::journal::JournalError;
use porter_core::capability::StorageCap;
use porter_sync::ItemState;
use std::collections::BTreeMap;

type Files = BTreeMap<String, Vec<u8>>;

/// The scenario's setup, run to rest: four files on the replica, all synced.
async fn setup(world: &World) -> (TestEngine, Vec<porter_sync::RemoteId>) {
    let ids = vec![
        world.remote_put("keep.txt", b"keep").await,
        world.remote_put("edit.txt", b"old").await,
        world.remote_put("gone.txt", b"gone").await,
        world.remote_put("localdel.txt", b"del").await,
        world.remote_put("mine.txt", b"orig").await,
    ];
    let engine = world.engine();
    settle(&engine, 5).await;
    (engine, ids)
}

/// What happens between two cycles: both sides change, in every way the engine handles.
async fn mutate(world: &World, ids: &[porter_sync::RemoteId]) {
    world.remote_edit(&ids[1], b"new").await;
    world.remote_remove(&ids[2]).await;
    world.remote_put("new_remote.txt", b"from afar").await;
    world.dataset.put("new_local.txt", b"from here");
    world.dataset.put("mine.txt", b"edited here");
    world.dataset.remove("localdel.txt");
}

fn converged() -> Files {
    [
        ("keep.txt", "keep"),
        ("edit.txt", "new"),
        ("mine.txt", "edited here"),
        ("new_remote.txt", "from afar"),
        ("new_local.txt", "from here"),
    ]
    .iter()
    .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec()))
    .collect()
}

async fn assert_converged(world: &World, engine: &TestEngine, what: &str) {
    assert_eq!(world.dataset.snapshot(), converged(), "{what}: local side");
    assert_eq!(world.remote_files().await, converged(), "{what}: replica");
    let rows = engine.journal().items().expect("items");
    assert!(
        rows.iter().all(|r| r.state == ItemState::Synced),
        "{what}: {rows:?}"
    );
    assert_eq!(rows.len(), converged().len(), "{what}: one row per file");
    assert!(
        engine.journal().conflicts().expect("conflicts").is_empty(),
        "{what}"
    );
    assert!(
        engine
            .journal()
            .tombstones()
            .expect("tombstones")
            .is_empty(),
        "{what}: compacted"
    );
}

async fn every_cut_point_converges(name: &str, features: StorageCap) {
    // Clean run: the number of journal writes the second phase makes, and the final state.
    let world = World::new(name, features.clone(), 3);
    let (engine, ids) = setup(&world).await;
    mutate(&world, &ids).await;
    let before = engine.journal().writes();
    engine.sync_once().await.expect("clean cycle");
    let writes = engine.journal().writes() - before;
    settle(&engine, 4).await;
    assert_converged(&world, &engine, "clean").await;
    assert!(
        writes >= 8,
        "the scenario should have many cut points, has {writes}"
    );

    for cut in 0..writes {
        let world = World::new(&format!("{name}-{cut}"), features.clone(), 3);
        let (engine, ids) = setup(&world).await;
        mutate(&world, &ids).await;
        engine.journal().cut_at(cut);
        let died = engine.sync_once().await;
        assert!(
            matches!(died, Err(SyncError::Journal(JournalError::Cut))),
            "cut {cut} of {writes}: {died:?}"
        );
        drop(engine);
        let restarted = world.engine();
        settle(&restarted, 6).await;
        assert_converged(&world, &restarted, &format!("cut {cut} of {writes}")).await;
    }
}

#[tokio::test]
async fn a_crash_between_any_two_writes_is_finished_by_the_next_run() {
    every_cut_point_converges("cuts", sha()).await;
}

#[tokio::test]
async fn and_the_same_without_hashes_where_adoption_compares_bytes() {
    every_cut_point_converges("cuts-hashless", hashless()).await;
}
