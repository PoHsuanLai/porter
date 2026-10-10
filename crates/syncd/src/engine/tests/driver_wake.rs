//! The driver's sleeps are bounded by the poll interval (reliability finding rel-10): the timer
//! stops while the computer is suspended and the wall clock does not, so one long sleep would
//! run late by as long as the computer slept.

use super::rig::*;
use crate::driver::{Driver, step};
use crate::scheduler::{MeteredPolicy, Network, Pausing, Settings};
use crate::service::{Access, DatasetName, Hub};
use porter_core::capability::{Delta, StorageCap};
use porter_core::{AppId, AppName, Isolation};
use porter_dbus::{Caller, CallerRole};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};

#[test]
fn a_sleep_never_runs_longer_than_the_poll_interval_and_never_less_than_a_second_apart() {
    // (seconds to go, poll interval, scheduler seconds per real second, the step in ms)
    const CASES: &[(u64, u32, u32, u64)] = &[
        (10, 60, 1, 10_000),
        (60, 60, 1, 60_000),
        (61, 60, 1, 60_000),
        (21_600, 60, 1, 60_000),
        (21_600, 1, 1, 1_000),
        // A zero interval would spin: at least a second.
        (30, 0, 1, 1_000),
        (0, 60, 1, 0),
        // A test's faster scheduler clock: ten of its seconds to the real second.
        (21_600, 1, 10, 100),
        (5, 60, 10, 500),
    ];
    for (remaining, poll, scale, want) in CASES {
        assert_eq!(
            step(*remaining, *poll, *scale),
            Duration::from_millis(*want),
            "{remaining} s to go, polling every {poll} s, {scale} to the second"
        );
    }
}

#[tokio::test]
async fn a_clock_that_jumped_while_the_computer_slept_runs_the_next_cycle_within_a_step() {
    // A push replica polls only every `poll_max` (an hour here) after a quiet cycle, so its
    // next wake is far off; the steps are `poll_base` (a second).
    let world = World::new(
        "wake",
        StorageCap {
            delta: Delta::Push,
            ..sha()
        },
        10,
    );
    world.remote_put("a.txt", b"first").await;
    let hub = Hub::default();
    let handle = hub.register(
        DatasetName::parse("acct_1/files").expect("name"),
        Access::default(),
    );
    let settings = Settings {
        poll_base: 1,
        poll_max: 3600,
        push_window: 0,
        batch_window: 0,
        metered: MeteredPolicy::Pause,
        time_scale: 1,
    };
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(
        world.engine(),
        handle,
        settings,
        network,
        Arc::new(Notify::new()),
        1,
    );
    let running = tokio::spawn(driver.run());

    let has = |path: &str| world.dataset.snapshot().contains_key(path);
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if has("a.txt") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(has("a.txt"), "the first cycle ran at once");

    // Something new on the server, and the computer sleeps for two hours: the wall clock moves
    // on, the timer does not. The driver, asleep until about an hour from the first cycle,
    // must notice within a step.
    world.remote_put("b.txt", b"second").await;
    world.clock.set(5_000 + 7_200);
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if has("b.txt") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    running.abort();
    assert!(
        has("b.txt"),
        "no cycle after the clock jumped (the driver notices within a step, not an hour)"
    );
}

#[tokio::test]
async fn a_pause_made_while_a_cycle_waits_for_its_turn_lets_no_cycle_through() {
    let world = World::new("paused", sha(), 10);
    world.remote_put("a.txt", b"first").await;
    let hub = Hub::default();
    let name = DatasetName::parse("acct_1/files").expect("name");
    let handle = hub.register(name.clone(), Access::default());
    // A cycle of the last look still runs: the driver's first cycle waits for the lock.
    let running_cycle = hub.hold_cycles(&name).await.expect("registered");
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(
        world.engine(),
        handle,
        Settings::quick(),
        network,
        Arc::new(Notify::new()),
        1,
    );
    let running = tokio::spawn(driver.run());
    // The driver (this runtime's only other task) reaches the lock and waits there.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let person = Caller {
        app: AppId {
            name: AppName::parse("org.quire.Settings").expect("name"),
            isolation: Isolation::Flatpak,
        },
        role: CallerRole::Settings,
    };
    assert!(hub.set_pausing(&person, &name, Pausing::Paused));
    drop(running_cycle);
    // Time for the cycle that would run, were the pause ignored (it ends in milliseconds).
    tokio::time::sleep(Duration::from_millis(500)).await;
    running.abort();
    assert!(
        !world.dataset.snapshot().contains_key("a.txt"),
        "a cycle began after the person paused"
    );
}

/// Polls every second at first and an hour once a push replica is quiet: after the first cycle
/// the next look is an hour away, so only a wake-up starts another.
fn far_polls() -> Settings {
    Settings {
        poll_base: 1,
        poll_max: 3600,
        push_window: 0,
        batch_window: 0,
        metered: MeteredPolicy::Pause,
        time_scale: 1,
    }
}

async fn cycles_ended(hub: &Hub, name: &DatasetName, want: u64) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if hub.cycles(name).is_some_and(|cycles| cycles.ended >= want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail("the cycles end");
}

#[tokio::test]
async fn sync_now_starts_a_cycle_at_once_though_the_next_look_is_an_hour_away() {
    let world = World::new(
        "sync_now",
        StorageCap {
            delta: Delta::Push,
            ..sha()
        },
        10,
    );
    world.remote_put("a.txt", b"first").await;
    let hub = Hub::default();
    let name = DatasetName::parse("acct_1/files").expect("name");
    let handle = hub.register(name.clone(), Access::default());
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(
        world.engine(),
        handle,
        far_polls(),
        network,
        Arc::new(Notify::new()),
        1,
    );
    let running = tokio::spawn(driver.run());
    cycles_ended(&hub, &name, 1).await;
    assert!(world.dataset.snapshot().contains_key("a.txt"));

    world.remote_put("b.txt", b"second").await;
    let begun = hub.cycles(&name).expect("registered").begun;
    hub.sync_now(&name).expect("not paused");
    cycles_ended(&hub, &name, begun + 1).await;
    running.abort();
    assert!(
        world.dataset.snapshot().contains_key("b.txt"),
        "the cycle Sync now asked for saw the change"
    );
}

#[tokio::test]
async fn a_sync_now_asked_while_the_driver_is_busy_is_followed_by_one_more_cycle() {
    let world = World::new(
        "sync_busy",
        StorageCap {
            delta: Delta::Push,
            ..sha()
        },
        10,
    );
    world.remote_put("a.txt", b"first").await;
    let hub = Hub::default();
    let name = DatasetName::parse("acct_1/files").expect("name");
    let handle = hub.register(name.clone(), Access::default());
    // A cycle of the last look still runs: the driver's first cycle waits for the lock.
    let busy = hub.hold_cycles(&name).await.expect("registered");
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(
        world.engine(),
        handle,
        far_polls(),
        network,
        Arc::new(Notify::new()),
        1,
    );
    let running = tokio::spawn(driver.run());
    // The driver (this runtime's only other task) reaches the lock and waits there.
    tokio::task::yield_now().await;
    hub.sync_now(&name).expect("not paused");
    drop(busy);
    // The first cycle, and the one Sync now asked for after it; the next look is an hour away.
    cycles_ended(&hub, &name, 2).await;
    running.abort();
}
