//! Acceptance 5 (PLAN §6 W6f): two machines against one fake WebDAV.

use super::rig::{Machine, Server};
use crate::datasets::photos::{ContentId, Edit, Mark, Stored};
use crate::engine::Outcome;
use std::collections::BTreeSet;
use std::time::Instant;

fn ids(machine: &Machine) -> BTreeSet<ContentId> {
    machine.library.held().expect("held").into_iter().collect()
}

#[tokio::test]
async fn acceptance_5_a_thousand_photos_imported_on_a_appear_on_b() {
    let started = Instant::now();
    let server = Server::start().await;
    let a = Machine::new(&server, "a", 1_000);
    let b = Machine::new(&server, "b", 1_000);

    let imported = a
        .library
        .import_many(&a.fixtures("x", 1_000))
        .expect("import");
    assert_eq!(imported.len(), 1_000);
    assert!(imported.iter().all(|i| i.stored == Stored::New));

    let up = a.sync().await;
    assert_eq!(
        (up.originals.outcome, up.originals.uploaded),
        (Outcome::Done, 1_000)
    );
    assert_eq!(up.metadata.uploaded, 1, "A's manifest");
    let down = b.sync().await;
    assert_eq!(
        (down.originals.outcome, down.originals.fetched),
        (Outcome::Done, 1_000)
    );
    assert_eq!(down.metadata.fetched, 1);

    assert_eq!(ids(&a), ids(&b));
    assert_eq!(ids(&b).len(), 1_000);
    // Every original on B is the bytes A imported (a name is the hash of its bytes, and the
    // dataset refuses a store that does not match it), and B's view names each photo.
    for item in &imported {
        let on_b = b.library.original(&item.id).expect("original on B");
        assert_eq!(ContentId::of(&std::fs::read(on_b).expect("read")), item.id);
    }
    let view = b.library.manifest().expect("manifest");
    assert_eq!(view, a.library.manifest().expect("manifest"));
    let named = imported
        .iter()
        .filter(|i| view.photo(&i.id).is_some_and(|p| p.name.is_some()))
        .count();
    assert_eq!(named, 1_000);
    assert_eq!(server.puts("originals"), 1_000);

    // Nothing left to do on either side.
    assert!(a.sync().await.is_quiet());
    assert!(b.sync().await.is_quiet());
    eprintln!("1000 photos A to B: {:?}", started.elapsed());
}

#[tokio::test]
async fn acceptance_5_a_favourite_set_on_both_while_offline_resolves_by_clock() {
    let server = Server::start().await;
    let a = Machine::new(&server, "a", 1_000);
    let b = Machine::new(&server, "b", 1_000);
    let ids_in: Vec<ContentId> = a
        .library
        .import_many(&[
            a.file("later_b.jpg", b"photo one"),
            a.file("later_a.jpg", b"photo two"),
        ])
        .expect("import")
        .into_iter()
        .map(|i| i.id)
        .collect();
    let (one, two) = (ids_in[0].clone(), ids_in[1].clone());
    a.sync().await;
    b.sync().await;

    // Both go offline and favourite (or un-favourite) each photo; on `one` B's clock is the
    // later, on `two` A's is. Different fields of one photo do not clash either.
    a.millis.set(10_000);
    b.millis.set(20_000);
    a.library
        .edit(vec![Edit::Favourite(one.clone(), Mark::On)])
        .expect("edit");
    b.library
        .edit(vec![Edit::Favourite(one.clone(), Mark::Off)])
        .expect("edit");
    a.millis.set(40_000);
    b.millis.set(30_000);
    a.library
        .edit(vec![Edit::Favourite(two.clone(), Mark::On)])
        .expect("edit");
    b.library
        .edit(vec![Edit::Favourite(two.clone(), Mark::Off)])
        .expect("edit");
    a.library
        .edit(vec![Edit::Caption(one.clone(), "beach".into())])
        .expect("edit");
    b.library
        .edit(vec![Edit::Album(one.clone(), "Trip".into(), Mark::On)])
        .expect("edit");

    // They come back online, in either order, and settle.
    let (ra, rb, ra2) = (a.sync().await, b.sync().await, a.sync().await);
    for report in [&ra, &rb, &ra2] {
        assert!(report.metadata.conflicts.is_empty() && report.originals.conflicts.is_empty());
    }
    let (view_a, view_b) = (
        a.library.manifest().expect("a"),
        b.library.manifest().expect("b"),
    );
    assert_eq!(view_a, view_b, "both machines hold one view");
    let (p_one, p_two) = (
        view_a.photo(&one).expect("one"),
        view_a.photo(&two).expect("two"),
    );
    assert_eq!(
        p_one.favourite,
        Mark::Off,
        "B's later clock wins on the first photo"
    );
    assert_eq!(
        p_two.favourite,
        Mark::On,
        "A's later clock wins on the second photo"
    );
    assert_eq!(p_one.caption, "beach", "a field only A wrote survives");
    assert!(
        p_one.albums.contains("Trip"),
        "a field only B wrote survives"
    );
    assert!(a.sync().await.is_quiet() && b.sync().await.is_quiet());
}

#[tokio::test]
async fn acceptance_5_a_duplicate_import_stores_one_original() {
    let server = Server::start().await;
    let a = Machine::new(&server, "a", 1_000);
    let b = Machine::new(&server, "b", 1_000);

    // Twice on A under two names, and once more on B, offline, before anything synced.
    let first = a
        .library
        .import(&a.file("one.jpg", b"the same pixels"))
        .expect("import");
    let again = a
        .library
        .import(&a.file("copy of one.jpg", b"the same pixels"))
        .expect("import");
    let on_b = b
        .library
        .import(&b.file("IMG_9.jpg", b"the same pixels"))
        .expect("import");
    assert_eq!(
        (first.stored, again.stored, on_b.stored),
        (Stored::New, Stored::Duplicate, Stored::New)
    );
    assert_eq!(first.id, again.id);
    assert_eq!(a.library.held().expect("held"), vec![first.id.clone()]);

    let (ra, rb, ra2) = (a.sync().await, b.sync().await, a.sync().await);
    for report in [&ra, &rb, &ra2] {
        assert!(
            report.originals.conflicts.is_empty(),
            "the same bytes cannot conflict"
        );
    }
    assert_eq!(server.puts("originals"), 1, "one original on the replica");
    assert_eq!(ids(&a), BTreeSet::from([first.id.clone()]));
    assert_eq!(ids(&b), BTreeSet::from([first.id.clone()]));
    assert_eq!(
        std::fs::read(a.library.original(&first.id).expect("a")).expect("read"),
        b"the same pixels"
    );
}

#[tokio::test]
async fn acceptance_5_an_expired_anchor_reconciles_with_zero_re_uploads() {
    let server = Server::start().await;
    let a = Machine::new(&server, "a", 1_000);
    let b = Machine::new(&server, "b", 1_000);
    a.library
        .import_many(&a.fixtures("e", 100))
        .expect("import");
    a.sync().await;
    b.sync().await;
    b.library
        .edit(vec![Edit::Caption(
            ContentId::of(b"x"),
            "b was here".into(),
        )])
        .expect("edit");
    b.sync().await;
    a.sync().await;

    let puts = (server.puts("originals"), server.puts("metadata"));
    let anchors = |m: &Machine| {
        (
            m.originals
                .journal()
                .anchor()
                .expect("journal")
                .expect("anchor")
                .anchor,
            m.metadata
                .journal()
                .anchor()
                .expect("journal")
                .expect("anchor")
                .anchor,
        )
    };
    let before = (anchors(&a), anchors(&b));
    server.dav.expire_sync_tokens();

    for machine in [&a, &b] {
        let synced = machine.sync().await;
        assert!(
            synced.is_quiet(),
            "nothing fetched, uploaded, removed or in conflict"
        );
        assert_eq!(synced.originals.outcome, Outcome::Done);
        assert_eq!(synced.metadata.outcome, Outcome::Done);
    }
    assert_eq!(
        (server.puts("originals"), server.puts("metadata")),
        puts,
        "no re-upload"
    );
    assert_ne!(
        (anchors(&a), anchors(&b)),
        before,
        "the anchors were renewed by a listing"
    );
    assert_eq!(ids(&a), ids(&b));
    assert_eq!(ids(&a).len(), 100);

    // And the dataset still works after it: a new photo on A reaches B.
    let more = a
        .library
        .import(&a.file("after.jpg", b"after the expiry"))
        .expect("import");
    a.sync().await;
    b.sync().await;
    assert!(b.library.original(&more.id).is_some());
}
