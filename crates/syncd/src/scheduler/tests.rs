use super::*;

const NOW: i64 = 10_000;

fn base() -> Inputs {
    Inputs {
        delta: Delta::Poll,
        last: Last::Never,
        push: PushSignal::Quiet,
        pausing: Pausing::Running,
        network: Network::Unmetered,
        settings: Settings {
            poll_base: 60,
            poll_max: 3600,
            push_window: 2,
            batch_window: 30,
            metered: MeteredPolicy::Pause,
            time_scale: 1,
        },
    }
}

#[derive(Debug)]
enum Expect {
    Now,
    /// At a time within these bounds (absolute seconds, inclusive).
    Between(i64, i64),
    Hold(Hold),
}

fn check(name: &str, inputs: Inputs, expect: Expect) {
    // The jitter is seeded, and a table row holds for every seed: try several.
    for seed in 0..25 {
        let wake = next_wake(&inputs, UnixSeconds(NOW), &mut Jitter::seeded(seed));
        let ok = match (&expect, wake) {
            (Expect::Now, Wake::Now) => true,
            (Expect::Between(lo, hi), Wake::At(UnixSeconds(at))) => (*lo..=*hi).contains(&at),
            (Expect::Hold(want), Wake::Hold(got)) => *want == got,
            _ => false,
        };
        assert!(ok, "{name} (seed {seed}): wanted {expect:?}, got {wake:?}");
    }
}

#[test]
fn a_dataset_that_never_ran_runs_now() {
    check("never", base(), Expect::Now);
}

#[test]
fn polling_waits_the_interval_and_doubles_it_for_every_idle_cycle() {
    let finished = |at: i64, idle: u32| Inputs {
        last: Last::Finished {
            at: UnixSeconds(at),
            idle,
        },
        ..base()
    };
    // Equal jitter: at least half the interval, at most all of it.
    check(
        "first idle cycle: 60 s",
        finished(NOW, 1),
        Expect::Between(NOW + 30, NOW + 60),
    );
    check(
        "second: 120 s",
        finished(NOW, 2),
        Expect::Between(NOW + 60, NOW + 120),
    );
    check(
        "fourth: 480 s",
        finished(NOW, 4),
        Expect::Between(NOW + 240, NOW + 480),
    );
    check(
        "capped at poll_max",
        finished(NOW, 30),
        Expect::Between(NOW + 1800, NOW + 3600),
    );
    check(
        "a cycle that changed things polls at the base again",
        finished(NOW, 0),
        Expect::Between(NOW + 30, NOW + 60),
    );
    check("overdue runs now", finished(NOW - 7200, 3), Expect::Now);
}

#[test]
fn a_failure_backs_off_exponentially_and_never_sooner_than_retry_after() {
    let failed = |failures: u32, retry_after: u32| Inputs {
        last: Last::Failed {
            at: UnixSeconds(NOW),
            failures,
            retry_after,
        },
        ..base()
    };
    check(
        "first failure",
        failed(1, 0),
        Expect::Between(NOW + 30, NOW + 60),
    );
    check(
        "third failure",
        failed(3, 0),
        Expect::Between(NOW + 120, NOW + 240),
    );
    check(
        "capped",
        failed(50, 0),
        Expect::Between(NOW + 1800, NOW + 3600),
    );
    check(
        "retry-after longer than the backoff wins",
        failed(1, 900),
        Expect::Between(NOW + 900, NOW + 900),
    );
    check(
        "backoff longer than retry-after wins",
        failed(3, 10),
        Expect::Between(NOW + 120, NOW + 240),
    );
}

#[test]
fn push_runs_after_its_window_and_polls_only_as_a_safety_net() {
    let push = |last: Last, signal: PushSignal| Inputs {
        delta: Delta::Push,
        last,
        push: signal,
        ..base()
    };
    let idle = Last::Finished {
        at: UnixSeconds(NOW),
        idle: 1,
    };
    check(
        "no signal: the slow safety poll",
        push(idle, PushSignal::Quiet),
        Expect::Between(NOW + 1800, NOW + 3600),
    );
    check(
        "a signal waits the batching window",
        push(idle, PushSignal::Since(UnixSeconds(NOW))),
        Expect::Between(NOW + 2, NOW + 2),
    );
    check(
        "a signal whose window passed runs now",
        push(idle, PushSignal::Since(UnixSeconds(NOW - 5))),
        Expect::Now,
    );
    check(
        "a signal does not make a poller run early",
        Inputs {
            delta: Delta::Poll,
            push: PushSignal::Since(UnixSeconds(NOW)),
            last: idle,
            ..base()
        },
        Expect::Between(NOW + 30, NOW + 60),
    );
}

#[test]
fn the_user_the_network_and_the_metered_setting_hold_the_dataset() {
    let held = |pausing, network, metered| Inputs {
        pausing,
        network,
        settings: Settings {
            metered,
            ..base().settings
        },
        last: Last::Finished {
            at: UnixSeconds(NOW - 9000),
            idle: 1,
        },
        ..base()
    };
    use MeteredPolicy::{Allow, Pause};
    check(
        "paused",
        held(Pausing::Paused, Network::Unmetered, Pause),
        Expect::Hold(Hold::UserPaused),
    );
    check(
        "paused beats offline",
        held(Pausing::Paused, Network::Offline, Pause),
        Expect::Hold(Hold::UserPaused),
    );
    check(
        "offline",
        held(Pausing::Running, Network::Offline, Allow),
        Expect::Hold(Hold::Offline),
    );
    check(
        "metered and paused by setting",
        held(Pausing::Running, Network::Metered, Pause),
        Expect::Hold(Hold::Metered),
    );
    check(
        "metered and allowed (overdue, so now)",
        held(Pausing::Running, Network::Metered, Allow),
        Expect::Now,
    );
    check(
        "unmetered ignores the setting",
        held(Pausing::Running, Network::Unmetered, Pause),
        Expect::Now,
    );
}

#[test]
fn a_held_dataset_that_never_ran_still_waits() {
    let inputs = Inputs {
        network: Network::Offline,
        ..base()
    };
    check(
        "offline before the first cycle",
        inputs,
        Expect::Hold(Hold::Offline),
    );
}

#[test]
fn wake_ups_within_the_window_are_batched_and_fire_at_the_latest() {
    let at = |s: i64| UnixSeconds(s);
    let wakes = [
        ("c", at(100)),
        ("a", at(10)),
        ("b", at(25)),
        ("d", at(95)),
        ("e", at(500)),
    ];
    let batches = coalesce(&wakes, 30);
    assert_eq!(
        batches,
        vec![
            (at(25), vec!["a", "b"]),
            (at(100), vec!["d", "c"]),
            (at(500), vec!["e"]),
        ]
    );
    assert!(coalesce::<&str>(&[], 30).is_empty());
    // A zero window batches only identical times.
    let same = coalesce(&[("x", at(5)), ("y", at(5)), ("z", at(6))], 0);
    assert_eq!(same, vec![(at(5), vec!["x", "y"]), (at(6), vec!["z"])]);
}

#[test]
fn the_same_seed_schedules_the_same_wake_up() {
    let inputs = Inputs {
        last: Last::Finished {
            at: UnixSeconds(NOW),
            idle: 3,
        },
        ..base()
    };
    let one = next_wake(&inputs, UnixSeconds(NOW), &mut Jitter::seeded(42));
    let two = next_wake(&inputs, UnixSeconds(NOW), &mut Jitter::seeded(42));
    assert_eq!(one, two);
}
