//! The driver's sleeps are bounded by the poll interval (reliability finding rel-10): the timer
//! stops while the computer is suspended and the wall clock does not, so one long sleep would
//! run late by as long as the computer slept.

use super::rig::*;
use crate::driver::{Driver, step};
use crate::scheduler::{MeteredPolicy, Network, Settings};
use crate::service::{Access, DatasetName, Hub};
use porter_core::capability::{Delta, StorageCap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};

#[test]
fn a_sleep_never_runs_longer_than_the_poll_interval_and_never_less_than_a_second_apart() {
    // (seconds to go, poll interval, the step)
    const CASES: &[(u64, u32, u64)] = &[
        (10, 60, 10),
        (60, 60, 60),
        (61, 60, 60),
        (21_600, 60, 60),
        (21_600, 1, 1),
        // A zero interval would spin: at least a second.
        (30, 0, 1),
        (0, 60, 0),
    ];
    for (remaining, poll, want) in CASES {
        assert_eq!(
            step(*remaining, *poll),
            Duration::from_secs(*want),
            "{remaining} s to go, polling every {poll} s"
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
    for _ in 0..500 {
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
    for _ in 0..500 {
        if has("b.txt") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    running.abort();
    assert!(
        has("b.txt"),
        "no cycle within ten seconds of the clock jumping"
    );
}
