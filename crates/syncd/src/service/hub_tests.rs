use super::*;
use porter_core::{AppId, Isolation};

fn caller(name: &str, role: CallerRole) -> Caller {
    Caller {
        app: AppId {
            name: AppName::parse(name).expect("name"),
            isolation: Isolation::Flatpak,
        },
        role,
    }
}

fn name(text: &str) -> DatasetName {
    DatasetName::parse(text).expect("dataset name")
}

fn owned_by(app: &str) -> Access {
    Access {
        owners: [AppName::parse(app).expect("name")].into(),
    }
}

#[test]
fn a_dataset_name_is_an_account_segment_a_slash_and_a_slug() {
    assert_eq!(name("acct_1/pim").to_string(), "acct_1/pim");
    for bad in [
        "", "pim", "a/", "/pim", "a/b/c", "a-b/pim", "a/Pim", "../pim",
    ] {
        assert_eq!(DatasetName::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn who_sees_a_dataset_by_role_and_ownership() {
    let access = owned_by("org.quire.Photos");
    let cases = [
        (
            "the owner",
            caller("org.quire.Photos", CallerRole::App),
            true,
        ),
        (
            "another app",
            caller("org.example.Other", CallerRole::App),
            false,
        ),
        (
            "settings",
            caller("org.quire.Settings", CallerRole::Settings),
            true,
        ),
        (
            "a porter daemon",
            caller("org.quire.Inference", CallerRole::PorterDaemon),
            true,
        ),
        (
            "an agent",
            caller("org.quire.Photos", CallerRole::Agent),
            false,
        ),
        ("cua", caller("org.quire.Cua", CallerRole::Cua), false),
    ];
    for (who, caller, sees) in cases {
        assert_eq!(access.admits(&caller), sees, "{who}");
    }
}

#[test]
fn a_hub_lists_shows_pauses_and_forgets_only_what_a_caller_may_see() {
    let hub = Hub::default();
    let mine = hub.register(name("a1/photos_originals"), owned_by("org.quire.Photos"));
    let _theirs = hub.register(name("a2/pim"), owned_by("org.quire.Sill"));
    let photos = caller("org.quire.Photos", CallerRole::App);
    let settings = caller("org.quire.Settings", CallerRole::Settings);
    assert_eq!(hub.names_for(&photos), ["a1/photos_originals"]);
    assert_eq!(hub.names_for(&settings), ["a1/photos_originals", "a2/pim"]);

    mine.publish(StatusSnapshot {
        pending: 3,
        ..StatusSnapshot::default()
    });
    let status = hub
        .status_for(&photos, &name("a1/photos_originals"))
        .expect("visible");
    assert_eq!((status.pending, status.pausing), (3, Pausing::Running));
    assert_eq!(hub.status_for(&photos, &name("a2/pim")), None);

    assert!(
        !hub.set_pausing(&photos, &name("a2/pim"), Pausing::Paused),
        "not theirs"
    );
    assert!(hub.set_pausing(&photos, &name("a1/photos_originals"), Pausing::Paused));
    assert_eq!(mine.pausing(), Pausing::Paused);
    let status = hub
        .status_for(&settings, &name("a1/photos_originals"))
        .expect("status");
    assert_eq!(
        status.pausing,
        Pausing::Paused,
        "the pause shows in the status"
    );

    let account = AccountDir::parse("a1").expect("account");
    assert_eq!(
        hub.forget_account(&account),
        vec![name("a1/photos_originals")]
    );
    assert!(!mine.is_registered());
    assert_eq!(hub.names_for(&settings), ["a2/pim"]);
}

#[tokio::test]
async fn a_handle_learns_of_a_pause_and_of_being_dropped() {
    let hub = Hub::default();
    let mut handle = hub.register(name("a1/pim"), Access::default());
    let settings = caller("org.quire.Settings", CallerRole::Settings);
    assert!(hub.set_pausing(&settings, &name("a1/pim"), Pausing::Paused));
    assert!(handle.changed().await, "the switch changed");
    hub.forget_account(&AccountDir::parse("a1").expect("account"));
    assert!(!handle.changed().await, "dropped: the engine stops");
}

#[tokio::test]
async fn events_reach_subscribers_and_nobody_listening_is_fine() {
    let hub = Hub::default();
    let handle = hub.register(name("a1/pim"), Access::default());
    handle.tell(Event::Progress {
        dataset: name("a1/pim"),
        fetched: 0,
        uploaded: 0,
    });
    let mut events = hub.subscribe();
    handle.tell(Event::Progress {
        dataset: name("a1/pim"),
        fetched: 2,
        uploaded: 1,
    });
    let got = events.recv().await.expect("event");
    assert_eq!(got.dataset(), &name("a1/pim"));
    assert!(matches!(
        got,
        Event::Progress {
            fetched: 2,
            uploaded: 1,
            ..
        }
    ));
}

#[test]
fn the_shell_hears_every_hold_and_nothing_else_of_a_dataset_it_does_not_own() {
    let hub = Hub::default();
    let handle = hub.register(name("a1/notes"), owned_by("org.example.Notes"));
    let shell = caller("org.quire.Sill", CallerRole::SheetHost);
    let owner = caller("org.example.Notes", CallerRole::App);
    let stranger = caller("org.example.Other", CallerRole::App);
    let agent = caller("org.quire.Agent", CallerRole::Agent);
    let hold = Event::Held {
        dataset: name("a1/notes"),
        held: Some(MassDelete {
            discard: 3,
            held: 3,
        }),
    };
    let progress = Event::Progress {
        dataset: name("a1/notes"),
        fetched: 1,
        uploaded: 0,
    };
    for (who, caller, hold_heard, progress_heard) in [
        ("the shell", &shell, true, false),
        ("the owner", &owner, true, true),
        ("a stranger", &stranger, false, false),
        ("an agent", &agent, false, false),
    ] {
        assert_eq!(hub.hears(caller, &hold), hold_heard, "{who}: hold");
        assert_eq!(
            hub.hears(caller, &progress),
            progress_heard,
            "{who}: progress"
        );
    }
    assert!(
        !hub.sees(&shell, &name("a1/notes")),
        "it still sees no data"
    );
    assert_eq!(hub.names_for(&shell), Vec::<String>::new());

    assert!(hub.holds_for(&shell).is_empty(), "nothing held yet");
    handle.publish(StatusSnapshot {
        needs_confirmation: Some(MassDelete {
            discard: 3,
            held: 3,
        }),
        ..StatusSnapshot::default()
    });
    let held = vec![(
        name("a1/notes"),
        MassDelete {
            discard: 3,
            held: 3,
        },
    )];
    assert_eq!(hub.holds_for(&shell), held);
    assert_eq!(hub.holds_for(&stranger), vec![]);
}

#[test]
fn forgetting_one_dataset_stops_it_and_leaves_the_others() {
    let hub = Hub::default();
    let gone = hub.register(name("a1/pim_cal_work"), Access::default());
    let kept = hub.register(name("a1/pim_cal_personal"), Access::default());
    assert!(hub.forget(&name("a1/pim_cal_work")));
    assert!(!hub.forget(&name("a1/pim_cal_work")), "already gone");
    assert!(!gone.is_registered());
    assert!(kept.is_registered());
}

#[tokio::test(start_paused = true)]
async fn a_stop_waits_for_a_cycle_that_runs_for_minutes_on_a_starved_machine() {
    // A cycle that takes two minutes: a stop that gave up at 30 s would go on, and its caller
    // delete the files the cycle is still writing.
    let cycling = Cycling::default();
    let held = cycling.0.clone().lock_owned().await;
    let waiting = tokio::spawn({
        let cycling = cycling.clone();
        async move { cycling.finished().await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    drop(held);
    assert!(waiting.await.expect("task"), "the stop saw the cycle end");

    // One that never ends is given up on, after the long wait.
    let stuck = Cycling::default();
    let _held = stuck.0.clone().lock_owned().await;
    assert!(!stuck.finished().await);
}

#[tokio::test(start_paused = true)]
async fn holding_the_cycles_waits_for_the_one_running_and_starts_no_other() {
    let hub = Hub::default();
    let engine = hub.register(name("a1/pim"), Access::default());
    assert!(hub.hold_cycles(&name("a1/nothing")).await.is_none());

    // A cycle in flight: the hold comes only when it ends, however long that is.
    let running = engine.begin_cycle().await.expect("registered");
    let holding = tokio::spawn({
        let hub = hub.clone();
        async move { hub.hold_cycles(&name("a1/pim")).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    assert!(!holding.is_finished(), "the cycle still runs");
    drop(running);
    let held = holding.await.expect("task").expect("the dataset is there");

    // While it is held, no cycle starts; once it is dropped, one can.
    let next = tokio::time::timeout(std::time::Duration::from_secs(60), engine.begin_cycle()).await;
    assert!(next.is_err(), "no cycle starts under the hold");
    drop(held);
    assert!(engine.begin_cycle().await.is_some());
}

#[tokio::test(start_paused = true)]
async fn sync_now_wakes_the_driver_once_whenever_it_is_asked_and_a_paused_dataset_refuses() {
    let hub = Hub::default();
    let dataset = name("a1/pim");
    let mut handle = hub.register(dataset.clone(), Access::default());
    async fn nothing_waits(handle: &mut Handle) -> bool {
        let waiting = handle.nudged();
        tokio::time::timeout(std::time::Duration::from_secs(60), waiting)
            .await
            .is_err()
    }
    assert_eq!(
        hub.sync_now(&name("a1/none")),
        Err(SyncNowError::NoSuchDataset)
    );

    // Asked while a cycle runs: kept, so the look after the cycle starts the next at once.
    let running = handle.begin_cycle().await.expect("registered");
    assert_eq!(hub.sync_now(&dataset), Ok(()));
    drop(running);
    assert!(matches!(handle.nudged().await, Nudge::SyncNow));
    assert!(nothing_waits(&mut handle).await, "one request, one wake-up");

    // Asked twice before the driver looks: one cycle follows, not two.
    assert_eq!(hub.sync_now(&dataset), Ok(()));
    assert_eq!(hub.sync_now(&dataset), Ok(()));
    assert!(matches!(handle.nudged().await, Nudge::SyncNow));
    assert!(nothing_waits(&mut handle).await);

    // Paused: refused, and the driver is told of the pause only.
    let settings = caller("org.quire.Settings", CallerRole::Settings);
    assert!(hub.set_pausing(&settings, &dataset, Pausing::Paused));
    assert_eq!(hub.sync_now(&dataset), Err(SyncNowError::Paused));
    assert!(matches!(handle.nudged().await, Nudge::Pause));
    assert!(nothing_waits(&mut handle).await);
    assert!(hub.set_pausing(&settings, &dataset, Pausing::Running));
    assert_eq!(hub.sync_now(&dataset), Ok(()));
}

#[test]
fn the_cycle_counts_say_which_cycle_began_after_a_moment() {
    let hub = Hub::default();
    let dataset = name("a1/pim");
    let handle = hub.register(dataset.clone(), Access::default());
    assert_eq!(hub.cycles(&name("a1/none")), None);
    assert_eq!(hub.names(), std::slice::from_ref(&dataset));
    let counts = |hub: &Hub| hub.cycles(&dataset).map(|c| (c.begun, c.ended));
    assert_eq!(counts(&hub), Some((0, 0)));
    handle.cycle_begun();
    assert_eq!(counts(&hub), Some((1, 0)), "running");
    handle.cycle_ended();
    handle.cycle_begun();
    handle.cycle_ended();
    assert_eq!(counts(&hub), Some((2, 2)));
}
