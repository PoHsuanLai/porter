use super::*;
use crate::settings::{ScopeLimits, SpendLimits};
use porter_core::{AppName, Isolation};

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
        used(1_000, 200),
        MicroUsd(700),
        LEAP_NOON,
    );
    ledger.record(
        &reader,
        &account("openrouter"),
        used(10, 5),
        MicroUsd(30),
        LEAP_NOON,
    );
    let spent = |scope: SpendScope, period| ledger.spent(&scope, period, LEAP_NOON);
    assert_eq!(
        spent(SpendScope::App(companion), Period::Daily),
        Spent {
            micro_usd: 700,
            tokens_in: 1_000,
            tokens_out: 200
        }
    );
    assert_eq!(
        spent(SpendScope::App(reader), Period::Monthly).micro_usd,
        30
    );
    let through = SpendScope::Account(account("openrouter"));
    assert_eq!(spent(through.clone(), Period::Daily).micro_usd, 730);
    assert_eq!(spent(through, Period::Monthly).tokens_in, 1_010);
    assert_eq!(
        spent(SpendScope::Account(account("openai")), Period::Daily),
        Spent::default()
    );
}

#[test]
fn a_new_day_starts_the_day_over_and_the_month_goes_on() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    ledger.record(
        &who,
        &account("openrouter"),
        used(1, 1),
        MicroUsd(500),
        LEAP_NOON,
    );
    let next_day = UnixSeconds(LEAP_NOON.0 + 86_400);
    let scope = SpendScope::App(who.clone());
    assert_eq!(ledger.spent(&scope, Period::Daily, next_day).micro_usd, 0);
    assert_eq!(
        ledger.spent(&scope, Period::Monthly, next_day).micro_usd,
        0,
        "that was March"
    );
    let same_month = UnixSeconds(LEAP_NOON.0 - 86_400);
    assert_eq!(
        ledger.spent(&scope, Period::Monthly, same_month).micro_usd,
        500
    );
    ledger.record(
        &who,
        &account("openrouter"),
        used(1, 1),
        MicroUsd(5),
        next_day,
    );
    assert_eq!(ledger.spent(&scope, Period::Daily, next_day).micro_usd, 5);
}

#[test]
fn the_caps_say_within_warn_or_stop_and_no_cap_says_within() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    let through = account("openrouter");
    let estimate = MicroUsd(0);
    // 10 cents a day is 100_000 micro-dollars; the warning line is at 80 per cent.
    let cases = [
        ("nothing spent", 0, SpendVerdict::Within),
        ("under the line", 79_999, SpendVerdict::Within),
        ("on the line", 80_000, SpendVerdict::Warn),
        ("under the cap", 99_999, SpendVerdict::Warn),
        ("at the cap", 100_000, SpendVerdict::Stop),
    ];
    for (name, spent, want) in cases {
        let ledger_now = Ledger::in_memory();
        if spent > 0 {
            ledger_now.record(&who, &through, used(0, 0), MicroUsd(spent), LEAP_NOON);
        }
        let got = ledger_now.verdict(line(10), &who, &through, estimate, LEAP_NOON);
        assert_eq!(got, want, "{name}");
    }
    assert_eq!(
        ledger.verdict(
            SpendLine::default(),
            &who,
            &through,
            MicroUsd(u64::MAX / 2),
            LEAP_NOON
        ),
        SpendVerdict::Within,
        "no cap, no limit"
    );
}

#[test]
fn the_estimate_of_the_turn_about_to_run_counts_toward_the_cap() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    ledger.record(
        &who,
        &account("openrouter"),
        used(0, 0),
        MicroUsd(99_000),
        LEAP_NOON,
    );
    let cheap = ledger.verdict(
        line(10),
        &who,
        &account("openrouter"),
        MicroUsd(10),
        LEAP_NOON,
    );
    let dear = ledger.verdict(
        line(10),
        &who,
        &account("openrouter"),
        MicroUsd(1_000),
        LEAP_NOON,
    );
    assert_eq!((cheap, dear), (SpendVerdict::Warn, SpendVerdict::Stop));
}

#[test]
fn the_strictest_cap_of_the_four_decides() {
    let who = app("org.quire.Companion");
    let through = account("openrouter");
    let ledger = Ledger::in_memory();
    ledger.record(&who, &through, used(0, 0), MicroUsd(150_000), LEAP_NOON);
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
        ledger.verdict(line, &who, &through, MicroUsd(0), LEAP_NOON),
        SpendVerdict::Stop,
        "the app's daily cap is passed though the account's monthly one is not"
    );
    let other = app("org.quire.Reader");
    assert_eq!(
        ledger.verdict(line, &other, &through, MicroUsd(0), LEAP_NOON),
        SpendVerdict::Within,
        "another app has spent nothing and the account's cap is far off"
    );
}

#[test]
fn a_ledger_opened_on_a_file_keeps_what_it_was_told_across_a_restart() {
    let dir = std::env::temp_dir().join(format!("inferd-spend-{}", std::process::id()));
    let file = dir.join("state").join("spend.json");
    let who = app("org.quire.Companion");
    {
        let ledger = Ledger::open(file.clone());
        ledger.record(
            &who,
            &account("openrouter"),
            used(7, 3),
            MicroUsd(1_234),
            LEAP_NOON,
        );
    }
    let again = Ledger::open(file.clone());
    let scope = SpendScope::App(who);
    assert_eq!(
        again.spent(&scope, Period::Daily, LEAP_NOON),
        Spent {
            micro_usd: 1_234,
            tokens_in: 7,
            tokens_out: 3
        }
    );
    let text = std::fs::read_to_string(&file).expect("the file");
    assert!(!text.contains("sk-"), "money and counts only: {text}");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&file).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_does_not_read_starts_the_ledger_empty() {
    let dir = std::env::temp_dir().join(format!("inferd-spend-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("spend.json");
    std::fs::write(&file, "not json").expect("write");
    let ledger = Ledger::open(file);
    let scope = SpendScope::App(app("org.quire.Companion"));
    assert_eq!(
        ledger.spent(&scope, Period::Daily, LEAP_NOON),
        Spent::default()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn usage_names_what_the_app_used_and_each_cap_that_is_set() {
    let ledger = Ledger::in_memory();
    let who = app("org.quire.Companion");
    ledger.record(
        &who,
        &account("openrouter"),
        used(40, 9),
        MicroUsd(321),
        LEAP_NOON,
    );
    let rows = ledger.usage_of(line(10), &who, LEAP_NOON);
    let want = [
        ("tokens_in_day", 40),
        ("tokens_out_day", 9),
        ("spend_day_micro_usd", 321),
        ("spend_month_micro_usd", 321),
        ("cap_day_micro_usd", 100_000),
    ];
    let got: Vec<(&str, u64)> = rows.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    assert_eq!(got, want);
}

#[test]
fn a_turn_not_yet_run_is_estimated_at_a_thousand_tokens_each_way() {
    let price = porter_core::PriceTable {
        input_per_mtok: MicroUsd(4_000_000),
        output_per_mtok: MicroUsd(20_000_000),
    };
    assert_eq!(estimate(&price), MicroUsd(4_000 + 20_000));
}
