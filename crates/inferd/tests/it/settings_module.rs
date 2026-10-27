//! inferd's settings module on a private bus: `org.quire.SettingsModule1` at
//! `/org/quire/Inference1/settings`. Describe, Get and Set by the Settings role; everyone else is
//! `NotPermitted`; a Set reaches the file and the next route; `Rescan` reads the file again.

use crate::hosting;

use ds_settings::live::{AnyOwner, LiveClient, LiveError, LiveSchema};
use ds_settings::schema::{
    AgentSetting, ChoicePlace, ChoiceWord, Exposure, KeyKind, KeyPath, Page,
};
use hosting::bus::{PrivateBus, within};
use inferd::audit::Memory;
use inferd::engines::Engines;
use inferd::peers::{Caller, Role, TablePeers};
use inferd::service::{Inference, serve_with_settings};
use inferd::settings::{ConfigFile, InferdSettings, Reload};
use inferd::supervise::Supervised;
use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::clock::FixedClock;
use porter_core::{
    AccountId, AppId, AppName, Billing, Isolation, Locality, ModelId, Tokens, UnixSeconds,
};
use porter_dbus::InferenceProxy;
use porter_dbus::names::{INFERENCE_BUS, INFERENCE_PATH, INFERENCE_SETTINGS_PATH};
use porter_infer::{Pick, Policy, Slot, TierMap};
use std::path::PathBuf;
use std::sync::Arc;
use zbus::Connection;
use zbus::export::futures_core::Stream;

struct Rig {
    bus: PrivateBus,
    _daemon: Connection,
    engines: Engines,
    file: PathBuf,
    peers: Arc<TablePeers>,
}

fn who(name: &str, role: Role) -> Caller {
    Caller {
        app: AppId {
            name: AppName::parse(name).expect("app name"),
            isolation: Isolation::Unsandboxed,
        },
        role,
    }
}

fn sonnet() -> porter_infer::ModelCard {
    porter_infer::ModelCard {
        account: AccountId::parse("anthropic").expect("id"),
        model: ModelId::parse("sonnet").expect("id"),
        locality: Locality::Cloud { region: None },
        billing: Billing::PlanBudget,
        capabilities: vec![Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(100_000),
            max_output: Tokens(4096),
            wire: LlmWire::Messages,
        })],
    }
}

impl Rig {
    async fn start() -> Rig {
        Self::start_with(vec![sonnet()]).await
    }

    async fn start_with(cards: Vec<porter_infer::ModelCard>) -> Rig {
        let bus = PrivateBus::start();
        let file = bus.scratch().join("inferd.toml");
        let engines = Engines::new(
            Vec::new(),
            Supervised::idle(),
            Policy::proposed(),
            TierMap::default(),
        )
        .with_remote(cards);
        let daemon = bus.connect().await;
        let peers = Arc::new(TablePeers::new());
        serving(&daemon, &engines, &peers, &file)
            .await
            .expect("serve Inference1 and its settings module");
        Rig {
            bus,
            _daemon: daemon,
            engines,
            file,
            peers,
        }
    }

    /// A client connection that the daemon knows as `role`.
    async fn client(&self, role: Role) -> Connection {
        let connection = self.bus.connect().await;
        self.peers.introduce(
            connection.unique_name().expect("name").as_str(),
            who("org.quire.Client", role),
        );
        connection
    }

    async fn live(&self, connection: &Connection) -> LiveClient {
        LiveClient::new(connection, INFERENCE_BUS, INFERENCE_SETTINGS_PATH, AnyOwner)
            .expect("client")
    }
}

/// inferd as its `main` serves it: `Inference1` and the settings module over `engines`, the
/// settings written to `file`.
fn serving(
    daemon: &Connection,
    engines: &Engines,
    peers: &Arc<TablePeers>,
    file: &std::path::Path,
) -> impl std::future::Future<Output = zbus::Result<()>> + Send + 'static {
    let reload = Reload::new(ConfigFile::new(file.to_owned()), engines.clone());
    // As the daemon wires it: what the module changes about the engines is announced.
    let news = Arc::new(tokio::sync::Notify::new());
    let inference = Inference::new(
        engines.clone(),
        Arc::clone(peers),
        Memory::default(),
        FixedClock(UnixSeconds(1_700_000_000)),
    )
    .reloading(reload.clone())
    .keys_news(Arc::clone(&news));
    // A program the person keeps in the test's own folder counts as theirs alone.
    let below = file.parent().expect("a folder").to_owned();
    let settings = InferdSettings::new(Arc::clone(peers), reload)
        .announcing(news)
        .programs_held_below(below);
    let daemon = daemon.clone();
    async move { serve_with_settings(&daemon, inference, settings).await }
}

fn key(path: &str) -> KeyPath {
    KeyPath(path.to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_settings_module_answers_a_call_made_as_soon_as_the_name_is_owned() {
    // Whoever sees `org.quire.Inference1` owned calls the module at once; an inferd that served
    // the module after claiming the name answered such a call `UnknownObject`.
    let bus = PrivateBus::start();
    let file = bus.scratch().join("inferd.toml");
    let engines = Engines::new(
        Vec::new(),
        Supervised::idle(),
        Policy::proposed(),
        TierMap::default(),
    )
    .with_remote(vec![sonnet()]);
    let peers = Arc::new(TablePeers::new());
    let client = bus.connect().await;
    peers.introduce(
        client.unique_name().expect("name").as_str(),
        who("org.quire.Settings", Role::Settings),
    );
    let mut owned = zbus::fdo::DBusProxy::new(&client)
        .await
        .expect("proxy")
        .receive_name_owner_changed_with_args(&[(0, INFERENCE_BUS)])
        .await
        .expect("subscribe");
    let daemon = bus.connect().await;
    let served = tokio::spawn(serving(&daemon, &engines, &peers, &file));
    within(
        "org.quire.Inference1 to be owned",
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut owned).poll_next(cx)),
    )
    .await
    .expect("the owner change");
    // At once: no wait between seeing the name and calling the module.
    let describe = async {
        LiveClient::new(&client, INFERENCE_BUS, INFERENCE_SETTINGS_PATH, AnyOwner)?
            .describe()
            .await
    };
    let schema = within("the settings module to answer", describe).await;
    assert!(schema.is_ok(), "{schema:?}");
    within("inferd to serve", served)
        .await
        .expect("the serving task")
        .expect("inferd serves");
}

fn text(value: &str) -> toml::Value {
    toml::Value::String(value.to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_settings_path_is_under_inference1_and_named_in_porter_dbus() {
    assert_eq!(
        INFERENCE_SETTINGS_PATH,
        format!("{INFERENCE_PATH}/settings")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_lists_the_picker_rows_of_the_slots_inferd_has_models_for() {
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let schema: LiveSchema = settings.describe().await.expect("schema");
    schema.check().expect("a clean schema");
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    assert_eq!(
        paths,
        [
            "ai.model.text.fast",
            "ai.model.text.balanced",
            "ai.model.text.best",
            "ai.auto.pool.text",
            "engines.vllm_python",
            "engines.llama_server",
        ]
    );
    // The two programs are Advanced text rows in "Local models", empty by default.
    for spec in schema
        .key
        .iter()
        .filter(|k| k.path.0.starts_with("engines."))
    {
        assert_eq!(spec.kind, KeyKind::Text);
        assert_eq!(spec.default, text(""));
        assert_eq!(spec.exposure, Exposure::Advanced);
        assert_eq!(spec.section.0, "Local models");
        assert_eq!(spec.agent, AgentSetting::HandsOff);
    }
    let labels: Vec<&str> = schema
        .key
        .iter()
        .filter(|k| k.path.0.starts_with("engines."))
        .map(|k| k.label.0.as_str())
        .collect();
    assert_eq!(labels, ["vLLM's Python", "llama-server"]);
    let pickers = || {
        schema
            .key
            .iter()
            .filter(|spec| spec.path.0.starts_with("ai.model."))
    };
    for spec in pickers() {
        // The choices: none (the text slot has no default), Automatic, and the models inferd
        // knows.
        assert_eq!(
            spec.kind,
            KeyKind::Menu {
                variants: vec![String::new(), "auto".into(), "anthropic/sonnet".into()]
            }
        );
        assert_eq!(spec.default, text(""));
        assert_eq!(spec.page, Page::Intelligence);
        // A section per slot, the tier as the label (best reads "Demanding").
        assert_eq!(spec.section.0, "Models / Text");
        assert_eq!(spec.agent, AgentSetting::HandsOff);
        assert_eq!(spec.labels.of(""), Some("Not chosen"));
        assert_eq!(spec.labels.of("auto"), Some("Automatic"));
        assert_eq!(spec.labels.of("anthropic/sonnet"), Some("sonnet"));
    }
    let labels: Vec<&str> = pickers().map(|spec| spec.label.0.as_str()).collect();
    assert_eq!(labels, ["Fast", "Balanced", "Demanding"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_on_this_computer_is_grouped_and_the_default_and_automatic_are_not() {
    let mut here = sonnet();
    here.account = AccountId::parse("llama").expect("id");
    here.model = ModelId::parse("tiny").expect("id");
    here.locality = Locality::OnDevice;
    let rig = Rig::start_with(vec![sonnet(), here]).await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let schema: LiveSchema = settings.describe().await.expect("schema");
    schema.check().expect("a clean schema");
    let pickers = schema
        .key
        .iter()
        .filter(|spec| spec.path.0.starts_with("ai.model."));
    for spec in pickers {
        let group = |word: &str| {
            spec.groups
                .get(&ChoiceWord(word.to_owned()))
                .map(|g| g.0.as_str())
        };
        assert_eq!(group("llama/tiny"), Some("On this computer"));
        // Not on this computer and not a curated hosted entry: no group.
        assert_eq!(group("anthropic/sonnet"), None);
        assert_eq!(group(""), None);
        assert_eq!(group("auto"), None);
        // A model this computer runs is placed here; one inferd does not start names no engine.
        assert_eq!(
            spec.place_of("llama/tiny"),
            Some(&ChoicePlace::ThisComputer { engine: None })
        );
        assert_eq!(spec.place_of("anthropic/sonnet"), None);
        assert_eq!(spec.place_of(""), None);
        assert_eq!(spec.place_of("auto"), None);
        let sections: Vec<(Option<&str>, Vec<&str>)> = spec
            .grouped_choices()
            .iter()
            .map(|s| (s.group.map(|g| g.0.as_str()), s.choices.clone()))
            .collect();
        assert_eq!(
            sections,
            [
                (None, vec!["", "auto", "anthropic/sonnet"]),
                (Some("On this computer"), vec!["llama/tiny"]),
            ]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_on_the_persons_network_with_no_computer_of_its_own_is_listed_with_local() {
    let mut there = sonnet();
    there.account = AccountId::parse("llama").expect("id");
    there.model = ModelId::parse("tiny").expect("id");
    there.locality = Locality::LocalNetwork;
    let rig = Rig::start_with(vec![sonnet(), there]).await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let schema: LiveSchema = settings.describe().await.expect("schema");
    schema.check().expect("a clean schema");
    // The text row and the list Automatic chooses from say the same.
    for spec in schema.key.iter().filter(|spec| {
        spec.path.0.starts_with("ai.model.") || spec.path.0.starts_with("ai.auto.pool.")
    }) {
        let word = ChoiceWord("llama/tiny".to_owned());
        assert_eq!(
            spec.groups.get(&word).map(|g| g.0.as_str()),
            Some("On your network")
        );
        assert_eq!(
            spec.place_of("llama/tiny"),
            Some(&ChoicePlace::ThisComputer { engine: None })
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_of_a_probed_runtime_is_listed_under_this_computer_and_stays_listed_when_it_stops()
{
    use inferd::probe::{Found, Probed, Runtime};
    use inferd::probed::Standing;
    use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
    use porter_core::{Claim, Offer, Provenance, Subject, Tokens};
    let id = ModelId::parse("llama3.2-3b").expect("id");
    let found = Found {
        runtime: Runtime::Ollama,
        port: porter_provider::Port(11_434),
        models: vec![Probed {
            id: id.clone(),
            name: "llama3.2:3b".into(),
            claims: vec![Claim {
                subject: Subject::Model(id),
                offer: Offer::Present(Capability::Llm(LlmCap {
                    features: [LlmFeature::Chat].into(),
                    context: Tokens(8192),
                    max_output: Tokens(8192),
                    wire: LlmWire::OllamaNative,
                })),
                provenance: Provenance::Discovered,
            }],
        }],
    };
    let rig = Rig::start_with(vec![]).await;
    rig.engines.probed().set(
        Runtime::Ollama,
        Standing::Online,
        Some(found.local_models(std::path::Path::new("/nonexistent"))),
    );
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    for standing in [Standing::Online, Standing::Offline] {
        rig.engines.probed().set(Runtime::Ollama, standing, None);
        let schema: LiveSchema = settings.describe().await.expect("schema");
        for spec in schema
            .key
            .iter()
            .filter(|spec| matches!(spec.kind, KeyKind::Menu { .. } | KeyKind::List(_)))
        {
            let group = spec
                .groups
                .get(&ChoiceWord("ollama/llama3.2-3b".to_owned()))
                .map(|g| g.0.as_str());
            assert_eq!(group, Some("On this computer"), "{standing:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_set_by_the_settings_role_is_validated_written_and_in_force_for_the_next_route() {
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let slot = key("ai.model.text.balanced");
    let in_force = || {
        rig.engines
            .settings()
            .tiers
            .pick(Slot::Text, porter_core::Tier::Balanced)
    };
    assert_eq!(settings.get(&slot).await.expect("get"), text(""));
    assert_eq!(in_force(), None);

    settings
        .set(&slot, &text("anthropic/sonnet"))
        .await
        .expect("set");
    assert_eq!(
        settings.get(&slot).await.expect("get"),
        text("anthropic/sonnet")
    );
    assert!(matches!(in_force(), Some(Pick::Named(m)) if m.model.as_str() == "sonnet"));
    // Written to inferd.toml at the row's path.
    let written: toml::Table = std::fs::read_to_string(&rig.file)
        .expect("file")
        .parse()
        .expect("toml");
    assert_eq!(
        written["ai"]["model"]["text"]["balanced"].as_str(),
        Some("anthropic/sonnet")
    );

    settings.set(&slot, &text("auto")).await.expect("set");
    assert!(matches!(in_force(), Some(Pick::Auto { .. })));
    settings.set(&slot, &text("")).await.expect("set");
    assert_eq!(in_force(), None);
    assert_eq!(settings.get(&slot).await.expect("get"), text(""));
}

#[tokio::test(flavor = "multi_thread")]
async fn automatics_pool_is_a_list_of_the_slots_choices_written_and_in_force() {
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let pool = key("ai.auto.pool.text");
    let list = |items: &[&str]| toml::Value::Array(items.iter().map(|one| text(one)).collect());
    assert_eq!(settings.get(&pool).await.expect("get"), list(&[]));
    // Listed as a key, a list over the slot's models (no unset choice, no Automatic).
    let schema = settings.describe().await.expect("describe");
    let spec = schema
        .key
        .iter()
        .find(|k| k.path.0 == "ai.auto.pool.text")
        .expect("the pool is a key");
    assert_eq!(
        spec.kind,
        KeyKind::List(Box::new(KeyKind::Menu {
            variants: vec!["anthropic/sonnet".into()]
        }))
    );
    settings
        .set(&pool, &list(&["anthropic/sonnet", "anthropic/sonnet"]))
        .await
        .expect("set");
    assert_eq!(
        settings.get(&pool).await.expect("get"),
        list(&["anthropic/sonnet"])
    );
    assert_eq!(rig.engines.settings().tiers.pool(Slot::Text).len(), 1);
    for bad in [
        list(&["local/ghost"]),
        list(&["auto"]),
        text("anthropic/sonnet"),
    ] {
        let got = settings.set(&pool, &bad).await;
        assert!(matches!(got, Err(LiveError::BadValue(_))), "{bad}: {got:?}");
    }
    assert_eq!(
        settings.get(&pool).await.expect("get"),
        list(&["anthropic/sonnet"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_set_that_names_no_model_inferd_knows_is_a_bad_value_and_changes_nothing() {
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let slot = key("ai.model.text.fast");
    for bad in [
        text("local/ghost"),
        text("sonnet"),
        text("anthropic/"),
        toml::Value::Integer(3),
    ] {
        let got = settings.set(&slot, &bad).await;
        assert!(matches!(got, Err(LiveError::BadValue(_))), "{bad}: {got:?}");
    }
    // A model of another kind is not a choice for this one.
    let got = settings
        .set(&key("ai.model.embeddings.fast"), &text("anthropic/sonnet"))
        .await;
    assert!(matches!(got, Err(LiveError::BadValue(_))), "{got:?}");
    for unknown in [
        "ai.model.text.huge",
        "ai.model.tv.fast",
        "ai.local_only",
        "accounts.x",
    ] {
        let got = settings.get(&key(unknown)).await;
        assert!(
            matches!(got, Err(LiveError::UnknownKey(_))),
            "{unknown}: {got:?}"
        );
    }
    assert!(!rig.file.exists(), "nothing was written");
    assert_eq!(rig.engines.settings().tiers, TierMap::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_coding_agent_is_a_choice_of_the_text_slot_alone_listed_in_its_own_group() {
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let slot = key("ai.model.text.balanced");
    settings
        .set(&slot, &text("agent/claude-code"))
        .await
        .expect("set");
    assert!(matches!(
        rig.engines.settings().tiers.pick(Slot::Text, porter_core::Tier::Balanced),
        Some(Pick::Named(m)) if m.is_agent()
    ));
    let schema = settings.describe().await.expect("describe");
    let spec = schema.key.iter().find(|k| k.path == slot).expect("the row");
    let group = spec
        .groups
        .get(&ChoiceWord("agent/claude-code".to_owned()))
        .map(|group| group.0.as_str());
    assert_eq!(group, Some("Coding Agents"));
    // Not a choice of any other slot, and never in Automatic's pool.
    let list = toml::Value::Array(vec![text("agent/claude-code")]);
    let refused = [
        (
            key("ai.model.computer_use.balanced"),
            text("agent/claude-code"),
        ),
        (key("ai.auto.pool.text"), list),
    ];
    for (path, bad) in refused {
        let got = settings.set(&path, &bad).await;
        assert!(
            matches!(got, Err(LiveError::BadValue(_))),
            "{path:?}: {got:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_but_the_settings_role_may_describe_get_or_set() {
    let rig = Rig::start().await;
    let slot = key("ai.model.text.fast");
    let stranger = rig.bus.connect().await;
    for connection in [
        rig.client(Role::App).await,
        rig.client(Role::Cua).await,
        stranger,
    ] {
        let client = rig.live(&connection).await;
        assert!(matches!(
            client.describe().await,
            Err(LiveError::NotPermitted(_))
        ));
        assert!(matches!(
            client.get(&slot).await,
            Err(LiveError::NotPermitted(_))
        ));
        assert!(matches!(
            client.set(&slot, &text("auto")).await,
            Err(LiveError::NotPermitted(_))
        ));
    }
    assert!(!rig.file.exists());
    assert_eq!(rig.engines.settings().tiers, TierMap::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn rescan_reads_the_file_again_and_a_file_that_does_not_read_is_its_error() {
    let rig = Rig::start().await;
    let connection = rig.client(Role::App).await;
    let inference = InferenceProxy::new(&connection).await.expect("proxy");
    std::fs::write(&rig.file, "[ai.auto]\nallow_evict = \"never\"\n").expect("write");
    assert_eq!(
        rig.engines.settings().auto.allow_evict,
        porter_infer::AutoEvict::IdleOnly
    );
    inference.rescan().await.expect("rescan");
    assert_eq!(
        rig.engines.settings().auto.allow_evict,
        porter_infer::AutoEvict::Never
    );
    std::fs::write(&rig.file, "[ai\n").expect("write");
    assert!(inference.rescan().await.is_err());
    assert_eq!(
        rig.engines.settings().auto.allow_evict,
        porter_infer::AutoEvict::Never
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_kind_row_in_the_file_is_the_slots_row_and_a_set_keeps_the_two_in_step() {
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    std::fs::write(
        &rig.file,
        "[ai.model.llm]\nbalanced = \"anthropic/sonnet\"\n",
    )
    .expect("write");
    let connection = rig.client(Role::App).await;
    InferenceProxy::new(&connection)
        .await
        .expect("proxy")
        .rescan()
        .await
        .expect("rescan");
    let slot = key("ai.model.text.balanced");
    assert_eq!(
        settings.get(&slot).await.expect("get"),
        text("anthropic/sonnet")
    );
    // The old key still names the same row for a client that has not moved.
    assert_eq!(
        settings
            .get(&key("ai.model.llm.balanced"))
            .await
            .expect("get"),
        text("anthropic/sonnet")
    );
    settings.set(&slot, &text("auto")).await.expect("set");
    let written: toml::Table = std::fs::read_to_string(&rig.file)
        .expect("file")
        .parse()
        .expect("toml");
    assert_eq!(
        written["ai"]["model"]["text"]["balanced"].as_str(),
        Some("auto")
    );
    assert_eq!(
        written["ai"]["model"]["llm"]["balanced"].as_str(),
        Some("auto"),
        "the old row cannot say otherwise"
    );
    assert!(matches!(
        rig.engines
            .settings()
            .tiers
            .pick(Slot::Text, porter_core::Tier::Balanced),
        Some(Pick::Auto { .. })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slot_with_no_model_here_carries_no_note_about_missing_programs() {
    let bus = PrivateBus::start();
    let file = bus.scratch().join("inferd.toml");
    let engines = Engines::new(
        Vec::new(),
        Supervised::idle(),
        Policy::proposed(),
        TierMap::default(),
    )
    .with_remote(vec![sonnet()]);
    let peers = Arc::new(TablePeers::new());
    let daemon = bus.connect().await;
    // No program is named, so every engine is missing: the picker's Local row asks which to
    // download, so the row itself says nothing about it.
    serving(&daemon, &engines, &peers, &file)
        .await
        .expect("serve");
    let client = bus.connect().await;
    peers.introduce(
        client.unique_name().expect("name").as_str(),
        who("org.quire.Settings", Role::Settings),
    );
    let settings =
        LiveClient::new(&client, INFERENCE_BUS, INFERENCE_SETTINGS_PATH, AnyOwner).expect("client");
    let schema = settings.describe().await.expect("schema");
    let row = schema
        .key
        .iter()
        .find(|k| k.path.0 == "ai.model.text.best")
        .expect("the text row");
    assert_eq!(row.note, None);
    assert!(!row.help.0.contains("not installed"), "{}", row.help.0);
}

fn helper(id: &str) -> porter_infer::Chooser {
    porter_infer::Chooser::Helper(porter_core::HelperId::parse(id).expect("helper id"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_may_set_a_helpers_pool_and_it_is_written_and_in_force() {
    let rig = Rig::start().await;
    let shell = rig.client(Role::Shell).await;
    let proxy = InferenceProxy::new(&shell).await.expect("proxy");
    proxy
        .set_helper_pool(
            "text",
            "h.1",
            vec!["anthropic/sonnet".into(), "anthropic/sonnet".into()],
        )
        .await
        .expect("the shell sets a helper's pool");
    let tiers = rig.engines.settings().tiers.clone();
    assert_eq!(tiers.pool_of(&helper("h.1"), Slot::Text).len(), 1);
    assert!(tiers.pool(Slot::Text).is_empty(), "not the assistant's");
    assert!(tiers.pool_of(&helper("h.2"), Slot::Text).is_empty());
    let written = std::fs::read_to_string(&rig.file).expect("the file");
    assert!(written.contains("\"text.helper.h.1\""), "{written}");
    // Settings reads it back through the settings module by the same key.
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    assert_eq!(
        settings
            .get(&key("ai.auto.pool.text.helper.h.1"))
            .await
            .expect("get"),
        toml::Value::Array(vec![text("anthropic/sonnet")])
    );
    // An empty pool removes the row: the helper is not on Automatic any more.
    proxy
        .set_helper_pool("text", "h.1", Vec::new())
        .await
        .expect("clear");
    let tiers = rig.engines.settings().tiers.clone();
    assert!(tiers.pool_of(&helper("h.1"), Slot::Text).is_empty());
    let written = std::fs::read_to_string(&rig.file).expect("the file");
    assert!(!written.contains("helper"), "{written}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_helpers_pool_names_only_models_inferd_knows_and_the_settings_role_may_set_it_too() {
    let rig = Rig::start().await;
    let settings = rig.client(Role::Settings).await;
    let proxy = InferenceProxy::new(&settings).await.expect("proxy");
    for (slot, helper, models) in [
        ("text", "h.1", vec!["local/ghost"]),
        ("text", "h.1", vec!["auto"]),
        ("tv", "h.1", vec![]),
        ("text", "", vec![]),
    ] {
        let got = proxy
            .set_helper_pool(slot, helper, models.into_iter().map(Into::into).collect())
            .await;
        assert!(
            matches!(&got, Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with("InvalidArgs")),
            "{slot}/{helper}: {got:?}"
        );
    }
    assert!(!rig.file.exists());
    proxy
        .set_helper_pool("text", "h.1", vec!["anthropic/sonnet".into()])
        .await
        .expect("Settings sets it");
    assert_eq!(
        rig.engines
            .settings()
            .tiers
            .pool_of(&helper("h.1"), Slot::Text)
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_may_write_no_other_setting_and_an_app_no_helper_pool() {
    let rig = Rig::start().await;
    let shell = rig.live(&rig.client(Role::Shell).await).await;
    for path in [
        "ai.auto.pool.text",
        "ai.auto.pool.text.helper.h.1",
        "ai.model.text.fast",
    ] {
        let got = shell.set(&key(path), &toml::Value::Array(Vec::new())).await;
        assert!(
            matches!(got, Err(LiveError::NotPermitted(_))),
            "{path}: {got:?}"
        );
    }
    let app = rig.client(Role::App).await;
    let got = InferenceProxy::new(&app)
        .await
        .expect("proxy")
        .set_helper_pool("text", "h.1", Vec::new())
        .await;
    assert!(
        matches!(&got, Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with("AccessDenied")),
        "{got:?}"
    );
    assert!(!rig.file.exists());
    assert_eq!(rig.engines.settings().tiers, TierMap::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_program_is_written_read_back_announced_and_cleared_and_a_bad_path_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::start().await;
    let settings = rig.live(&rig.client(Role::Settings).await).await;
    let listener = rig.client(Role::App).await;
    let inference = InferenceProxy::new(&listener).await.expect("proxy");
    let mut heard = inference
        .receive_engines_changed()
        .await
        .expect("subscribe");
    let row = key("engines.llama_server");
    // A program the person holds, in the rig's folder, which only they can change.
    let folder = rig.file.parent().expect("a folder");
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let held = folder.join("llama-server");
    std::fs::write(&held, "").expect("write");
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let held = held.to_string_lossy().into_owned();
    let held = held.as_str();
    let written = || -> toml::Table {
        std::fs::read_to_string(&rig.file)
            .expect("file")
            .parse()
            .expect("toml")
    };
    assert_eq!(settings.get(&row).await.expect("get"), text(""));

    settings.set(&row, &text(held)).await.expect("set");
    assert_eq!(settings.get(&row).await.expect("get"), text(held));
    assert_eq!(written()["engines"]["llama_server"].as_str(), Some(held));
    within(
        "EnginesChanged",
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut heard).poll_next(cx)),
    )
    .await
    .expect("an open stream");
    // The other program is its own row.
    assert_eq!(
        settings
            .get(&key("engines.vllm_python"))
            .await
            .expect("get"),
        text("")
    );

    // Refused in plain words, and the row keeps what it had.
    let scratch = test_kit::Scratch::new("program-row");
    let loose = scratch.join("python");
    std::fs::write(&loose, "").expect("write");
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o775)).expect("chmod");
    let loose = loose.to_string_lossy().into_owned();
    for (bad, words) in [
        ("python", "whole path"),
        ("/nonexistent/python", "no program"),
        (loose.as_str(), "can be changed by other people"),
    ] {
        let got = settings.set(&row, &text(bad)).await;
        assert!(
            matches!(&got, Err(LiveError::BadValue(why)) if why.contains(words)),
            "{bad}: {got:?}"
        );
    }
    assert!(
        matches!(
            settings.set(&row, &toml::Value::Integer(1)).await,
            Err(LiveError::BadValue(_))
        ),
        "a program is text"
    );
    assert_eq!(settings.get(&row).await.expect("get"), text(held));

    // Empty takes the key out of the file: inferd finds the program itself again.
    settings.set(&row, &text("")).await.expect("clear");
    assert_eq!(settings.get(&row).await.expect("get"), text(""));
    assert!(written()["engines"].get("llama_server").is_none());
}
