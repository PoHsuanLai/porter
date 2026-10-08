//! Each row changes what inferd decides: the file's text goes through the same `resolve` the
//! daemon uses and into the same `choose` (and `Engines::route_detailed`) a session is routed by.

use super::*;
use crate::config::InferdConfig;
use crate::engines::Engines;
use crate::peers::Role;
use crate::router::{Decided, Listed, choose};
use crate::session::SessionSpec;
use crate::supervise::{Ports, Supervised};
use crate::testkit::{Recorder, Scratch, models};
use engine_supervisor::{FakeGpu, FakeReadyProbe, GpuMemory, Probe, SupervisorConfig};
use model_catalog::MiB;
use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::need::LlmNeed;
use porter_core::{
    AccountId, Billing, DataClass, Locality, MicroUsd, ModelId, Need, Permille, Tier, Tokens,
};
use porter_infer::{
    AutoEvict, EngineLoad, InferRefusal, LicenceClass, LocalOnly, ModelCard, ModelRef, Pick,
    PickRefusal, Policy, Readiness, ShowReason, Slot, SwapCost, Why,
};
use std::time::Duration;

fn config(text: &str) -> InferdConfig {
    InferdConfig::from_toml(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn settings(text: &str) -> Settings {
    let resolved = resolve(&config(text));
    assert_eq!(resolved.rejected, Vec::<String>::new(), "{text}");
    resolved.settings
}

fn need() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn card(account: &str, model: &str, locality: Locality) -> ModelCard {
    ModelCard {
        account: AccountId::parse(account).expect("id"),
        model: ModelId::parse(model).expect("id"),
        locality,
        billing: Billing::Free,
        capabilities: vec![Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(8192),
            max_output: Tokens(1024),
            wire: LlmWire::ChatCompletions,
        })],
    }
}

fn listed(card: ModelCard, readiness: Readiness) -> Listed {
    Listed::new(card, readiness, SwapCost::Resident, LicenceClass::Open)
}

fn local(model: &str) -> Listed {
    listed(card("local", model, Locality::OnDevice), Readiness::Ready)
}

fn cloud(model: &str) -> Listed {
    listed(
        card("anthropic", model, Locality::Cloud { region: None }),
        Readiness::Ready,
    )
}

/// What the file `text` decides for `class` at `tier` over `models`.
fn decide(
    text: &str,
    class: DataClass,
    tier: Tier,
    models: &[Listed],
) -> Result<Decided, PickRefusal> {
    let s = settings(text);
    choose(&need(), class, tier, models, &s.policy, &s.tiers, s.auto)
}

fn refusal(result: Result<Decided, PickRefusal>) -> InferRefusal {
    result.expect_err("refused").refusal
}

fn served(result: Result<Decided, PickRefusal>) -> String {
    result.expect("served").chosen.model.as_str().to_owned()
}

#[test]
fn no_file_is_the_proposed_settings() {
    let resolved = resolve(&InferdConfig::default());
    assert_eq!(resolved.settings, Settings::default());
    assert_eq!(resolved.settings.policy, Policy::proposed());
    assert_eq!(resolved.settings.spend.warn_at, Permille(800));
    assert!(resolved.rejected.is_empty());
}

#[test]
fn local_only_removes_every_cloud_model_and_off_lets_the_next_rule_speak() {
    let models = [cloud("claude")];
    let on = decide(
        "[ai]\nlocal_only = \"on\"\n",
        DataClass::Public,
        Tier::Balanced,
        &models,
    );
    assert_eq!(refusal(on), InferRefusal::Unavailable);
    // Off: a cloud model is reachable; for Public data the rule that stops it is consent.
    let off = decide(
        "[ai]\nlocal_only = \"off\"\n",
        DataClass::Public,
        Tier::Balanced,
        &models,
    );
    assert_eq!(refusal(off), InferRefusal::NeedsGrant);
    // No row is the proposed `on`.
    let none = decide("", DataClass::Public, Tier::Balanced, &models);
    assert_eq!(refusal(none), InferRefusal::Unavailable);
}

#[test]
fn a_floor_refuses_the_cloud_until_the_row_widens_it() {
    let models = [cloud("claude")];
    let with = |floor: &str| {
        decide(
            &format!("[ai]\nlocal_only = \"off\"\n[ai.floor]\nnotes = \"{floor}\"\n"),
            DataClass::Notes,
            Tier::Balanced,
            &models,
        )
    };
    assert_eq!(
        refusal(with("on_device")),
        InferRefusal::RequiresCloud(DataClass::Notes)
    );
    assert_eq!(
        refusal(with("local_network")),
        InferRefusal::RequiresCloud(DataClass::Notes)
    );
    assert_eq!(refusal(with("anywhere")), InferRefusal::NeedsGrant);
}

#[test]
fn every_class_has_its_own_floor_row_and_voice_and_prompt_default_to_this_computer() {
    let base = settings("").policy;
    assert_eq!(base.floor(DataClass::Voice), porter_infer::Floor::OnDevice);
    assert_eq!(base.floor(DataClass::Prompt), porter_infer::Floor::OnDevice);
    for class in CLASSES {
        let path = format!("ai.floor.{}", keys_slug(class));
        let mut table = toml::Table::new();
        file::put(&mut table, &path, "local_network".into());
        let policy = settings(&table.to_string()).policy;
        for other in CLASSES {
            let want = if other == class {
                porter_infer::Floor::LocalNetwork
            } else {
                base.floor(other)
            };
            assert_eq!(policy.floor(other), want, "{path} for {other:?}");
        }
    }
}

fn keys_slug<T: serde::Serialize>(value: T) -> String {
    keys::slug_of(&value)
}

#[test]
fn the_model_row_decides_between_models_and_auto_and_empty_hand_back_to_the_catalogue() {
    let models = [
        listed(
            card("local", "alpha", Locality::OnDevice),
            Readiness::Loadable,
        ),
        local("beta"),
    ];
    let row = |value: &str| format!("[ai.model.llm]\nbest = \"{value}\"\n");
    let best = |text: &str| decide(text, DataClass::Notes, Tier::Best, &models);
    assert_eq!(served(best(&row("local/beta"))), "beta");
    assert_eq!(served(best(&row("local/alpha"))), "alpha");
    // Another tier is not touched by the row.
    assert_eq!(
        served(decide(
            &row("local/beta"),
            DataClass::Notes,
            Tier::Fast,
            &models
        )),
        "alpha"
    );
    // Empty: the catalogue's order, as before any row.
    let empty = best(&row("")).expect("served");
    assert_eq!(
        (empty.chosen.model.as_str(), empty.why),
        ("alpha", Why::CatalogueOrder)
    );
    // Auto is `pick`, which says why.
    let auto = best(&row("auto")).expect("served");
    assert_eq!((auto.chosen.model.as_str(), auto.why), ("beta", Why::Warm));
    // A named model that is not there is refused, never replaced.
    assert!(best(&row("local/ghost")).is_err());
}

#[test]
fn never_evict_refuses_the_swap_idle_only_makes() {
    let swapping = Listed {
        swap: SwapCost::Evicts {
            victim: ModelRef {
                account: AccountId::parse("local").expect("id"),
                model: ModelId::parse("old").expect("id"),
            },
            load: EngineLoad::Idle,
            cold_start_estimate_s: 90,
        },
        ..listed(
            card("local", "alpha", Locality::OnDevice),
            Readiness::Loadable,
        )
    };
    let models = [swapping];
    let with = |evict: &str| {
        decide(
            &format!("[ai.model.llm]\nbalanced = \"auto\"\n[ai.auto]\nallow_evict = \"{evict}\"\n"),
            DataClass::Notes,
            Tier::Balanced,
            &models,
        )
    };
    assert!(matches!(
        with("idle_only").expect("swaps").why,
        Why::Evicted { .. }
    ));
    assert_eq!(refusal(with("never")), InferRefusal::Unavailable);
    assert_eq!(settings("").auto.allow_evict, AutoEvict::IdleOnly);
}

#[test]
fn auto_mode_reads_its_one_value() {
    assert_eq!(
        settings("[ai.auto]\nmode = \"warm_first\"\n").auto,
        porter_infer::AutoPolicy::default()
    );
    let resolved = resolve(&config("[ai.auto]\nmode = \"coldest_first\"\n"));
    assert_eq!(resolved.rejected, vec!["ai.auto.mode".to_owned()]);
}

fn engines(scratch: &Scratch) -> Engines {
    let models = models(scratch);
    let supervised = Supervised::start(
        models.iter().map(|m| m.spec.clone()).collect(),
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: FakeGpu(GpuMemory {
                total: MiB(16_000),
                used_by_others: MiB(0),
            }),
        },
    );
    Engines::new(
        models,
        supervised,
        Policy::proposed(),
        porter_infer::TierMap::default(),
    )
}

fn spec() -> SessionSpec {
    SessionSpec {
        need: need(),
        class: DataClass::Notes,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

#[tokio::test]
async fn show_reason_off_stops_the_answers_saying_why_and_a_reload_applies_it() {
    let scratch = Scratch::new("settings-show");
    let engines = engines(&scratch);
    let file = ConfigFile::new(scratch.path().join("inferd.toml"));
    let reload = Reload::new(file.clone(), engines.clone());
    let show = || {
        engines
            .route_detailed(&spec(), Role::App)
            .expect("route")
            .0
            .show
    };
    assert_eq!(show(), ShowReason::On);
    file.set("ai.auto.show_reason", "off".into())
        .expect("write");
    // Nothing is in force until the file is read again.
    assert_eq!(show(), ShowReason::On);
    assert_eq!(reload.now().expect("reload").rejected, Vec::<String>::new());
    assert_eq!(show(), ShowReason::Off);
    file.set("ai.auto.show_reason", "on".into()).expect("write");
    reload.now().expect("reload");
    assert_eq!(show(), ShowReason::On);
}

#[tokio::test]
async fn a_reload_changes_the_route_of_the_next_session_and_a_bad_file_changes_nothing() {
    let scratch = Scratch::new("settings-reload");
    let engines = engines(&scratch);
    let file = ConfigFile::new(scratch.path().join("inferd.toml"));
    let reload = Reload::new(file.clone(), engines.clone());
    let route = || {
        engines
            .route_detailed(&spec(), Role::App)
            .map(|(r, _)| r.served.model)
    };
    assert!(route().is_ok());
    // A named model that is not here: refused, naming it.
    file.set("ai.model.llm.balanced", "local/ghost".into())
        .expect("write");
    reload.now().expect("reload");
    assert!(route().is_err());
    // A file that does not read keeps what is in force.
    std::fs::write(file.path(), "[ai\nbroken").expect("write");
    assert!(reload.now().is_err());
    assert!(route().is_err());
    file.set("ai.model.llm.balanced", "".into()).ok();
    std::fs::write(
        file.path(),
        "[ai.model.llm]\nbalanced = \"local/tiny-chat\"\n",
    )
    .expect("write");
    reload.now().expect("reload");
    assert_eq!(route().expect("served").as_str(), "tiny-chat");
}

#[tokio::test]
async fn the_watch_notices_a_change_made_by_someone_else() {
    let scratch = Scratch::new("settings-watch");
    let engines = engines(&scratch);
    let file = ConfigFile::new(scratch.path().join("inferd.toml"));
    let watch = Reload::new(file.clone(), engines.clone()).watch(Duration::from_millis(20));
    let show = || engines.settings().auto.show_reason;
    assert_eq!(show(), ShowReason::On);
    std::fs::write(file.path(), "[ai.auto]\nshow_reason = \"off\"\n").expect("write");
    for _ in 0..100 {
        if show() == ShowReason::Off {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    watch.abort();
    assert_eq!(show(), ShowReason::Off);
}

#[test]
fn the_warn_line_is_the_rows_permille() {
    let cap = |text: &str| {
        settings(text).spend.cap(
            SpendScope::Account(AccountId::parse("anthropic").expect("id")),
            Period::Daily,
            MicroUsd(1_000_000),
        )
    };
    let verdict = |cap: &SpendCap, spent: u64| spend_verdict(cap, MicroUsd(spent), MicroUsd(0));
    let default = cap("");
    assert_eq!(default.warn_at, Permille(800));
    assert_eq!(verdict(&default, 600_000), SpendVerdict::Within);
    let early = cap("[ai.spend]\nwarn_permille = 500\n");
    assert_eq!(early.warn_at, Permille(500));
    assert_eq!(verdict(&early, 499_999), SpendVerdict::Within);
    assert_eq!(verdict(&early, 500_000), SpendVerdict::Warn);
    assert_eq!(verdict(&early, 1_000_000), SpendVerdict::Stop);
    // 1..=1000 only; anything else is the default, and says so.
    for bad in ["0", "1001", "-5"] {
        let resolved = resolve(&config(&format!("[ai.spend]\nwarn_permille = {bad}\n")));
        assert_eq!(resolved.settings.spend, SpendLine::default(), "{bad}");
        assert_eq!(resolved.rejected, vec![SPEND_WARN.to_owned()], "{bad}");
    }
    for edge in ["1", "1000"] {
        assert!(
            resolve(&config(&format!("[ai.spend]\nwarn_permille = {edge}\n")))
                .rejected
                .is_empty()
        );
    }
}

use porter_infer::{SpendCap, SpendVerdict, spend_verdict};

fn companion() -> porter_core::AppId {
    porter_core::AppId {
        name: porter_core::AppName::parse("org.quire.Companion").expect("name"),
        isolation: porter_core::Isolation::Unsandboxed,
    }
}

#[test]
fn the_cap_rows_are_cents_per_scope_and_period_and_zero_is_no_cap() {
    let through = AccountId::parse("openrouter").expect("id");
    let caps = |text: &str| -> Vec<(SpendScope, Period, MicroUsd, Permille)> {
        settings(text)
            .spend
            .caps_for(&companion(), &through)
            .into_iter()
            .map(|cap| (cap.scope, cap.period, cap.limit, cap.warn_at))
            .collect()
    };
    assert_eq!(caps(""), vec![], "no row, no cap");
    assert_eq!(
        caps("[ai.spend]\napp_daily_cents = 0\n"),
        vec![],
        "zero is no cap"
    );
    let app = SpendScope::App(companion());
    let account = SpendScope::Account(through.clone());
    assert_eq!(
        caps(
            "[ai.spend]\nwarn_permille = 500\naccount_daily_cents = 250\naccount_monthly_cents = 1\n\
             app_daily_cents = 10000000\napp_monthly_cents = 7\n"
        ),
        vec![
            (
                account.clone(),
                Period::Daily,
                MicroUsd(2_500_000),
                Permille(500)
            ),
            (account, Period::Monthly, MicroUsd(10_000), Permille(500)),
            (
                app.clone(),
                Period::Daily,
                MicroUsd(100_000_000_000),
                Permille(500)
            ),
            (app, Period::Monthly, MicroUsd(70_000), Permille(500)),
        ]
    );
}

#[test]
fn a_cap_out_of_range_is_no_cap_and_is_named() {
    for (row, path) in [
        ("account_daily_cents", SPEND_ACCOUNT_DAILY),
        ("account_monthly_cents", SPEND_ACCOUNT_MONTHLY),
        ("app_daily_cents", SPEND_APP_DAILY),
        ("app_monthly_cents", SPEND_APP_MONTHLY),
    ] {
        for bad in ["-1", "10000001"] {
            let resolved = resolve(&config(&format!("[ai.spend]\n{row} = {bad}\n")));
            assert_eq!(
                resolved.settings.spend,
                SpendLine::default(),
                "{row} = {bad}"
            );
            assert_eq!(resolved.rejected, vec![path.to_owned()], "{row} = {bad}");
        }
        for edge in ["0", "1", "10000000"] {
            assert!(
                resolve(&config(&format!("[ai.spend]\n{row} = {edge}\n")))
                    .rejected
                    .is_empty(),
                "{row} = {edge}"
            );
        }
    }
}

#[test]
fn the_old_tables_still_read_and_the_rows_at_their_paths_win_over_them() {
    let old = "[policy]\nlocal_only = \"off\"\nfloors = [{ class = \"mail\", floor = \"anywhere\" }]\n\
               [[tiers.rows]]\nkind = \"llm\"\ntier = \"fast\"\nmodel = { account = \"local\", model = \"alpha\" }\n\
               [[tiers.rows]]\nkind = \"llm\"\ntier = \"best\"\nmodel = { account = \"local\", model = \"beta\" }\n";
    let s = settings(old);
    assert_eq!(s.policy.local_only, LocalOnly::Off);
    assert_eq!(
        s.policy.floor(DataClass::Mail),
        porter_infer::Floor::Anywhere
    );
    assert!(
        matches!(s.tiers.pick(Slot::Text, Tier::Fast), Some(Pick::Named(m)) if m.model.as_str() == "alpha")
    );
    // The same file with rows at the settings paths: they win, field by field.
    let both = format!(
        "{old}\n[ai]\nlocal_only = \"on\"\n[ai.floor]\nmail = \"on_device\"\n[ai.model.llm]\nfast = \"auto\"\nbest = \"\"\n"
    );
    let s = settings(&both);
    assert_eq!(s.policy.local_only, LocalOnly::On);
    assert_eq!(
        s.policy.floor(DataClass::Mail),
        porter_infer::Floor::OnDevice
    );
    assert!(matches!(
        s.tiers.pick(Slot::Text, Tier::Fast),
        Some(Pick::Auto(_))
    ));
    assert_eq!(s.tiers.pick(Slot::Text, Tier::Best), None);
    // What the old tables said and the rows did not touch stays.
    assert_eq!(
        s.policy.floor(DataClass::Photos),
        porter_infer::Floor::Anywhere
    );
}

#[test]
fn a_value_a_row_does_not_accept_is_named_and_falls_back_for_that_row_only() {
    let text = "[ai]\nlocal_only = \"maybe\"\n[ai.floor]\nmail = \"nowhere\"\ntelepathy = \"anywhere\"\nnotes = \"anywhere\"\n\
                [ai.model.llm]\nfast = \"no-slash\"\nhuge = \"local/a\"\nbalanced = \"local/alpha\"\n\
                [ai.auto]\nallow_evict = \"sometimes\"\nshow_reason = \"off\"\n";
    let resolved = resolve(&config(text));
    let mut rejected = resolved.rejected.clone();
    rejected.sort();
    assert_eq!(
        rejected,
        [
            "ai.auto.allow_evict",
            "ai.floor.mail",
            "ai.floor.telepathy",
            "ai.local_only",
            "ai.model.llm.fast",
            "ai.model.llm.huge",
        ]
    );
    let s = resolved.settings;
    assert_eq!(s.policy.local_only, LocalOnly::On);
    assert_eq!(
        s.policy.floor(DataClass::Notes),
        porter_infer::Floor::Anywhere
    );
    assert_eq!(
        s.policy.floor(DataClass::Mail),
        porter_infer::Floor::OnDevice
    );
    assert_eq!(s.auto.show_reason, ShowReason::Off);
    assert!(s.tiers.pick(Slot::Text, Tier::Balanced).is_some());
    assert_eq!(s.tiers.pick(Slot::Text, Tier::Fast), None);
}

#[test]
fn set_writes_one_row_at_its_path_and_keeps_the_rest() {
    let scratch = Scratch::new("settings-set");
    let file = ConfigFile::new(scratch.path().join("sub").join("inferd.toml"));
    assert_eq!(
        file.read().expect("missing is the default"),
        InferdConfig::default()
    );
    file.set("ai.model.llm.fast", "auto".into()).expect("write");
    file.set("ai.model.llm.best", "local/beta".into())
        .expect("write");
    file.set("ai.auto.allow_evict", "never".into())
        .expect("write");
    let read = file.read().expect("reads");
    assert_eq!(read.ai.model["llm"]["fast"], "auto");
    assert_eq!(read.ai.model["llm"]["best"], "local/beta");
    assert_eq!(read.ai.auto.allow_evict.as_deref(), Some("never"));
    // rel-8: the write is synced and renamed, and nothing is left staged. (A crash that loses an
    // unsynced file cannot be made in a test; this keeps the synced path working.)
    let staged = std::fs::read_dir(file.path().parent().expect("dir"))
        .expect("dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(staged, 0);
    // A file that does not parse is left as it was.
    std::fs::write(file.path(), "[ai\n").expect("write");
    assert!(file.set("ai.local_only", "off".into()).is_err());
    assert_eq!(std::fs::read_to_string(file.path()).expect("read"), "[ai\n");
}

fn picked(settings: &Settings, slot: Slot) -> Option<Pick> {
    settings.tiers.pick(slot, Tier::Balanced)
}

#[test]
fn a_slot_row_and_the_old_kind_row_load_to_the_same_slot() {
    let cases = [
        ("text", "llm", Slot::Text),
        ("voice_in", "speech_in", Slot::VoiceIn),
        ("voice_out", "speech_out", Slot::VoiceOut),
    ];
    for (new, old, slot) in cases {
        let from_new = settings(&format!("[ai.model.{new}]\nbalanced = \"local/alpha\"\n"));
        let from_old = settings(&format!("[ai.model.{old}]\nbalanced = \"local/alpha\"\n"));
        assert_eq!(from_new.tiers, from_old.tiers, "{new} / {old}");
        assert!(
            matches!(picked(&from_new, slot), Some(Pick::Named(m)) if m.model.as_str() == "alpha"),
            "{new}"
        );
    }
    let rows = [
        "text",
        "voice_in",
        "voice_out",
        "image_in",
        "computer_use",
        "embeddings",
    ];
    for slug in rows {
        let slot = from_slug::<Slot>(slug).expect(slug);
        let got = settings(&format!("[ai.model.{slug}]\nbalanced = \"auto\"\n"));
        assert!(matches!(picked(&got, slot), Some(Pick::Auto(_))), "{slug}");
    }
}

#[test]
fn when_a_file_says_both_the_slots_own_row_wins_whatever_the_order() {
    let got = settings(
        "[ai.model.text]\nbalanced = \"local/new\"\n[ai.model.llm]\nbalanced = \"local/old\"\nfast = \"auto\"\n",
    );
    assert!(matches!(
        picked(&got, Slot::Text),
        Some(Pick::Named(m)) if m.model.as_str() == "new"
    ));
    assert!(
        matches!(got.tiers.pick(Slot::Text, Tier::Fast), Some(Pick::Auto(_))),
        "an old row for another tier still applies"
    );
}

#[test]
fn the_describe_images_row_defaults_off_reads_on_and_names_a_bad_value() {
    use porter_infer::DescribeImages;
    assert_eq!(settings("").describe_images, DescribeImages::Off);
    for (word, want) in [("on", DescribeImages::On), ("off", DescribeImages::Off)] {
        let got = settings(&format!("[ai.pipeline]\ndescribe_images = \"{word}\"\n"));
        assert_eq!(got.describe_images, want, "{word}");
    }
    let resolved = resolve(&config("[ai.pipeline]\ndescribe_images = \"maybe\"\n"));
    assert_eq!(resolved.rejected, vec![DESCRIBE_IMAGES.to_owned()]);
    assert_eq!(resolved.settings.describe_images, DescribeImages::Off);
}

#[test]
fn a_language_model_is_in_the_slots_its_features_put_it_in() {
    let mut vision = card("local", "seer", Locality::OnDevice);
    if let Some(Capability::Llm(llm)) = vision.capabilities.first_mut() {
        llm.features
            .extend([LlmFeature::Vision, LlmFeature::AudioIn]);
    }
    assert_eq!(
        keys::slots_of(&vision),
        vec![Slot::Text, Slot::VoiceIn, Slot::ImageIn]
    );
    assert_eq!(
        keys::slots_of(&card("local", "plain", Locality::OnDevice)),
        vec![Slot::Text]
    );
}
