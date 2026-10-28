use super::*;
use crate::settings::{ScopeLimits, SpendLimits};
use porter_core::{AppName, Isolation};
use porter_ids::{Tokens, TurnCount};
use rust_decimal::Decimal;
use serde_json::json;
use test_kit::Scratch;

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn account(id: &str) -> AccountId {
    AccountId::parse(id).expect("id")
}

fn used(input: u32, output: u32) -> TokenUsage {
    TokenUsage {
        input: Tokens(input),
        output: Tokens(output),
        cached: Tokens(0),
        charge: None,
    }
}

/// A reply of `input` and `output` tokens that its provider said cost `milli` thousandths of a
/// US dollar.
fn charged(input: u32, output: u32, milli: i64) -> TokenUsage {
    TokenUsage {
        charge: Some(Money::usd(Decimal::new(milli, 3))),
        ..used(input, output)
    }
}

fn milli(milli: i64) -> ChargeTotal {
    ChargeTotal {
        reported: Money::usd(Decimal::new(milli, 3)),
        unpriced: TurnCount(0),
    }
}

/// 2024-02-29 12:00:00 UTC.
const LEAP_NOON: UnixSeconds = UnixSeconds(1_709_208_000);

#[test]
fn days_and_months_are_calendar_ones_in_utc() {
    let cases = [
        ("the epoch", 0, 0, 1970 * 12),
        ("a second before it", -1, -1, 1969 * 12 + 11),
        ("a leap day", 1_709_208_000, 19_782, 2024 * 12 + 1),
        (
            "the first of March 2000",
            951_868_800,
            11_017,
            2000 * 12 + 2,
        ),
        (
            "the last second of 2024",
            1_735_689_599,
            20_088,
            2024 * 12 + 11,
        ),
        ("the first second of 2025", 1_735_689_600, 20_089, 2025 * 12),
    ];
    for (name, at, day, month) in cases {
        assert_eq!(day_of(UnixSeconds(at)), day, "{name}: day");
        assert_eq!(month_of(UnixSeconds(at)), month, "{name}: month");
    }
}

fn line(app_daily: u32) -> SpendLine {
    SpendLine {
        limits: SpendLimits {
            account: ScopeLimits::default(),
            app: ScopeLimits {
                daily: crate::settings::cents_to_limit(app_daily),
                monthly: None,
            },
        },
        ..SpendLine::default()
    }
}

#[test]
fn a_reply_counts_against_the_app_and_the_account_for_the_day_and_the_month() {
    let ledger = Ledger::in_memory();
    let (companion, reader) = (app("org.quire.Companion"), app("org.quire.Reader"));
    ledger.record(
        &companion,
        &account("openrouter"),
        charged(1_000, 200, 700),
        LEAP_NOON,
    );
    ledger.record(
        &reader,
        &account("openrouter"),
        charged(10, 5, 30),
        LEAP_NOON,
    );
    // A reply whose provider reported no charge is a turn, and no money.
    ledger.record(&reader, &account("openrouter"), used(1, 1), LEAP_NOON);
    let spent = |scope: SpendScope, period| ledger.spent(&scope, period, LEAP_NOON);
    assert_eq!(
        spent(SpendScope::App(companion), Period::Daily),
        Spent {
            charges: milli(700),
            tokens_in: 1_000,
            tokens_out: 200
        }
    );
    assert_eq!(
        spent(SpendScope::App(reader), Period::Monthly).charges,
        ChargeTotal {
            unpriced: TurnCount(1),
            ..milli(30)
        }
    );
    let through = SpendScope::Account(account("openrouter"));
    assert_eq!(
        spent(through.clone(), Period::Daily).charges,
        ChargeTotal {
            unpriced: TurnCount(1),
            ..milli(730)
        }
    );
    assert_eq!(spent(through, Period::Monthly).tokens_in, 1_011);
    assert_eq!(
        spent(SpendScope::Account(account("openai")), Period::Daily),
        Spent::default()
    );
}

#[test]
fn a_new_day_starts_the_day_over_and_the_month_goes_on() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    ledger.record(&who, &account("openrouter"), charged(1, 1, 500), LEAP_NOON);
    let next_day = UnixSeconds(LEAP_NOON.0 + 86_400);
    let scope = SpendScope::App(who.clone());
    let reported = |period, at| ledger.spent(&scope, period, at).charges;
    assert_eq!(reported(Period::Daily, next_day), ChargeTotal::default());
    assert_eq!(
        reported(Period::Monthly, next_day),
        ChargeTotal::default(),
        "that was March"
    );
    let same_month = UnixSeconds(LEAP_NOON.0 - 86_400);
    assert_eq!(reported(Period::Monthly, same_month), milli(500));
    ledger.record(&who, &account("openrouter"), charged(1, 1, 5), next_day);
    assert_eq!(reported(Period::Daily, next_day), milli(5));
}

#[test]
fn the_caps_say_within_warn_or_stop_and_no_cap_says_within() {
    let who = app("org.quire.Companion");
    let through = account("openrouter");
    // 10 cents a day is 100 thousandths of a dollar; the warning line is at 80 per cent.
    let cases = [
        ("nothing spent", 0, SpendVerdict::Within),
        ("under the line", 79, SpendVerdict::Within),
        ("on the line", 80, SpendVerdict::Warn),
        ("under the cap", 99, SpendVerdict::Warn),
        ("at the cap", 100, SpendVerdict::Stop),
    ];
    for (name, spent, want) in cases {
        let ledger = Ledger::in_memory();
        if spent > 0 {
            ledger.record(&who, &through, charged(0, 0, spent), LEAP_NOON);
        }
        let got = ledger.verdict(line(10), &who, &through, LEAP_NOON);
        assert_eq!(got, want, "{name}");
    }
    let ledger = Ledger::in_memory();
    ledger.record(&who, &through, charged(0, 0, 1_000_000), LEAP_NOON);
    assert_eq!(
        ledger.verdict(SpendLine::default(), &who, &through, LEAP_NOON),
        SpendVerdict::Within,
        "no cap, no limit"
    );
}

/// A cap compares what providers reported: turns that reported nothing are counted and never meet
/// it, and the turn that passes it is the last one, not the one it stops.
#[test]
fn only_reported_charges_meet_a_cap_and_the_turn_that_passes_it_is_the_last() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    let through = account("openai");
    for _ in 0..1_000 {
        ledger.record(&who, &through, used(500, 500), LEAP_NOON);
    }
    assert_eq!(
        ledger.verdict(line(1), &who, &through, LEAP_NOON),
        SpendVerdict::Within,
        "a thousand turns with no charge are no money"
    );
    let scope = SpendScope::App(who.clone());
    assert_eq!(
        ledger
            .spent(&scope, Period::Daily, LEAP_NOON)
            .charges
            .unpriced,
        TurnCount(1_000)
    );
    // The reply that carries the charge is counted whole, and the next turn is stopped.
    ledger.record(&who, &through, charged(10, 10, 10), LEAP_NOON);
    assert_eq!(
        ledger.verdict(line(1), &who, &through, LEAP_NOON),
        SpendVerdict::Stop
    );
}

#[test]
fn the_strictest_cap_of_the_four_decides() {
    let who = app("org.quire.Companion");
    let through = account("openrouter");
    let ledger = Ledger::in_memory();
    ledger.record(&who, &through, charged(0, 0, 150), LEAP_NOON);
    let line = SpendLine {
        limits: SpendLimits {
            account: ScopeLimits {
                daily: None,
                monthly: crate::settings::cents_to_limit(1_000),
            },
            app: ScopeLimits {
                daily: crate::settings::cents_to_limit(10),
                monthly: None,
            },
        },
        ..SpendLine::default()
    };
    assert_eq!(
        ledger.verdict(line, &who, &through, LEAP_NOON),
        SpendVerdict::Stop,
        "the app's daily cap is passed though the account's monthly one is not"
    );
    let other = app("org.quire.Reader");
    assert_eq!(
        ledger.verdict(line, &other, &through, LEAP_NOON),
        SpendVerdict::Within,
        "another app has spent nothing and the account's cap is far off"
    );
}

#[test]
fn a_ledger_opened_on_a_file_keeps_what_it_was_told_across_a_restart() {
    let dir = Scratch::new("inferd-spend");
    let file = dir.join("state").join("spend.json");
    let who = app("org.quire.Companion");
    {
        let ledger = Ledger::open_at(file.clone(), LEAP_NOON);
        ledger.record(
            &who,
            &account("openrouter"),
            charged(7, 3, 1_234),
            LEAP_NOON,
        );
        ledger.record(&who, &account("openrouter"), used(1, 1), LEAP_NOON);
    }
    let again = Ledger::open_at(file.clone(), LEAP_NOON);
    let scope = SpendScope::App(who);
    assert_eq!(
        again.spent(&scope, Period::Daily, LEAP_NOON),
        Spent {
            charges: ChargeTotal {
                unpriced: TurnCount(1),
                ..milli(1_234)
            },
            tokens_in: 8,
            tokens_out: 4
        }
    );
    let text = std::fs::read_to_string(&file).expect("the file");
    assert!(!text.contains("sk-"), "money and counts only: {text}");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&file).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

fn scratch(name: &str) -> Scratch {
    Scratch::new(&format!("inferd-spend-{name}"))
}

/// rel-7: before, a file that did not read started the ledger empty, so every cap saw nothing
/// spent and the next request went through, however much had been spent. Now the file is moved
/// aside, and a cap of the day and the month stops until they end.
#[test]
fn a_file_that_does_not_read_is_set_aside_and_the_caps_stop_until_the_period_ends() {
    let dir = scratch("bad");
    let file = dir.join("spend.json");
    std::fs::write(&file, "not json").expect("write");
    let who = app("org.quire.Companion");
    let through = account("openrouter");
    let ledger = Ledger::open_at(file.clone(), LEAP_NOON);
    // The unreadable file is kept, under a name that says when.
    let aside = dir.join(format!("spend.json.corrupt-{}", LEAP_NOON.0));
    assert_eq!(std::fs::read_to_string(&aside).expect("kept"), "not json");
    // Nothing is known to be spent, and a cap stops all the same.
    let scope = SpendScope::App(who.clone());
    assert_eq!(
        ledger.spent(&scope, Period::Daily, LEAP_NOON),
        Spent::default()
    );
    let stopped = |ledger: &Ledger, at: UnixSeconds| ledger.verdict(line(10), &who, &through, at);
    assert_eq!(stopped(&ledger, LEAP_NOON), SpendVerdict::Stop);
    // With no cap set there is nothing to hold to.
    assert_eq!(
        ledger.verdict(SpendLine::default(), &who, &through, LEAP_NOON),
        SpendVerdict::Within
    );
    // The next day the daily cap counts again from nothing (the lost day is over).
    let tomorrow = UnixSeconds(LEAP_NOON.0 + 86_400);
    assert_eq!(stopped(&ledger, tomorrow), SpendVerdict::Within);
    // Records go on, and the stop is in the file: a restart in the same day does not forget it.
    ledger.record(&who, &through, charged(1, 1, 5), LEAP_NOON);
    let again = Ledger::open_at(file.clone(), LEAP_NOON);
    assert_eq!(stopped(&again, LEAP_NOON), SpendVerdict::Stop);
    assert_eq!(
        again.spent(&scope, Period::Daily, LEAP_NOON).charges,
        milli(5),
        "what was recorded since is kept"
    );
    // A restart after the day is over has nothing to remember for the daily cap.
    let later = Ledger::open_at(file, tomorrow);
    assert_eq!(stopped(&later, tomorrow), SpendVerdict::Within);
}

#[test]
fn a_monthly_cap_stays_stopped_to_the_end_of_the_month_and_a_missing_file_is_just_new() {
    let dir = scratch("monthly");
    let file = dir.join("spend.json");
    let who = app("org.quire.Companion");
    let through = account("openrouter");
    // No file yet: a new ledger, nothing set aside, caps work as usual.
    let fresh = Ledger::open_at(file.clone(), LEAP_NOON);
    assert_eq!(
        fresh.verdict(line(10), &who, &through, LEAP_NOON),
        SpendVerdict::Within
    );
    assert!(
        !dir.join(format!("spend.json.corrupt-{}", LEAP_NOON.0))
            .exists()
    );
    // A directory where the file should be cannot be read either: lost, not new.
    let blocked_root = scratch("monthly-dir");
    let blocked = blocked_root.join("spend.json");
    std::fs::create_dir_all(&blocked).expect("dir in the way");
    let monthly = SpendLine {
        limits: SpendLimits {
            account: ScopeLimits::default(),
            app: ScopeLimits {
                daily: None,
                monthly: crate::settings::cents_to_limit(10),
            },
        },
        ..SpendLine::default()
    };
    // Lost on the 19th of February 2024; the 20th is the same month; 1 March is the next.
    let mid_february = UnixSeconds(LEAP_NOON.0 - 10 * 86_400);
    let lost = Ledger::open_at(blocked, mid_february);
    for (name, at, want) in [
        ("the day", mid_february, SpendVerdict::Stop),
        (
            "the next day, same month",
            UnixSeconds(mid_february.0 + 86_400),
            SpendVerdict::Stop,
        ),
        (
            "the month after",
            UnixSeconds(LEAP_NOON.0 + 86_400),
            SpendVerdict::Within,
        ),
    ] {
        assert_eq!(lost.verdict(monthly, &who, &through, at), want, "{name}");
    }
}

#[test]
fn usage_names_what_the_app_used_and_each_cap_that_is_set() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    ledger.record(&who, &account("openrouter"), charged(40, 9, 321), LEAP_NOON);
    ledger.record(&who, &account("openai"), used(1, 1), LEAP_NOON);
    let rows = ledger.usage_of(line(10), &who, LEAP_NOON);
    let money = |milli: i64| UsageValue::Money(Money::usd(Decimal::new(milli, 3)));
    let want = [
        ("tokens_in_day", UsageValue::Count(41)),
        ("tokens_out_day", UsageValue::Count(10)),
        ("reported_day", money(321)),
        ("reported_month", money(321)),
        ("unpriced_turns_day", UsageValue::Count(1)),
        ("unpriced_turns_month", UsageValue::Count(1)),
        ("cap_day", money(100)),
    ];
    let got: Vec<(&str, UsageValue)> = rows.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    assert_eq!(got, want);
    assert_eq!(written(Money::usd(Decimal::new(9500, 6))), "0.0095 USD");
    assert_eq!(written(Money::usd(Decimal::ZERO)), "0 USD");
}

/// A ledger file from before the meter counted only reported charges has dollar amounts worked out
/// from prices. They are not read as money: the token counts and the lost period carry over, the
/// charges start empty, the file is rewritten in the new format, and the daemon says so.
#[test]
fn a_ledger_file_from_before_reported_charges_keeps_tokens_and_drops_its_dollars() {
    let dir = scratch("before");
    let file = dir.join("spend.json");
    let who = app("org.quire.Companion");
    let mut row = serde_json::to_value(Key {
        scope: SpendScope::App(who.clone()),
        period: Period::Daily,
        bucket: day_of(LEAP_NOON),
    })
    .expect("key")
    .as_object()
    .expect("an object")
    .clone();
    row.insert(
        "spent".to_owned(),
        json!({"micro_usd": 123_456_789, "tokens_in": 70, "tokens_out": 30}),
    );
    let before = json!({ "rows": [row] }).to_string();
    std::fs::write(&file, before).expect("write");
    let ledger = Ledger::open_at(file.clone(), LEAP_NOON);
    let scope = SpendScope::App(who.clone());
    assert_eq!(
        ledger.spent(&scope, Period::Daily, LEAP_NOON),
        Spent {
            charges: ChargeTotal::default(),
            tokens_in: 70,
            tokens_out: 30
        },
        "the old micro-dollars are not a charge"
    );
    assert_eq!(
        ledger.verdict(line(10), &who, &account("openrouter"), LEAP_NOON),
        SpendVerdict::Within
    );
    assert!(
        !dir.join(format!("spend.json.corrupt-{}", LEAP_NOON.0))
            .exists(),
        "an old file is read, not set aside"
    );
    let text = std::fs::read_to_string(&file).expect("the file");
    assert!(text.contains("\"format\":2"), "rewritten at once: {text}");
    assert!(!text.contains("micro_usd"), "{text}");
    ledger.record(&who, &account("openrouter"), charged(1, 1, 5), LEAP_NOON);
    let again = Ledger::open_at(file, LEAP_NOON);
    assert_eq!(
        again.spent(&scope, Period::Daily, LEAP_NOON),
        Spent {
            charges: milli(5),
            tokens_in: 71,
            tokens_out: 31
        }
    );
}

/// A file a later inferd wrote is not guessed at: it is set aside like one that does not read.
#[test]
fn a_ledger_file_of_a_format_not_known_is_set_aside() {
    let dir = scratch("later");
    let file = dir.join("spend.json");
    std::fs::write(&file, r#"{"format":3,"rows":[]}"#).expect("write");
    let _ledger = Ledger::open_at(file, LEAP_NOON);
    assert!(
        dir.join(format!("spend.json.corrupt-{}", LEAP_NOON.0))
            .exists()
    );
}
