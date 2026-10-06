//! inferd's settings module on a private bus: `org.quire.SettingsModule1` at
//! `/org/quire/Inference1/settings`. Describe, Get and Set by the Settings role; everyone else is
//! `NotPermitted`; a Set reaches the file and the next route; `Rescan` reads the file again.

mod hosting;

use ds_settings::live::{LiveClient, LiveError, LiveSchema};
use ds_settings::schema::{AgentSetting, KeyKind, KeyPath, Page};
use hosting::bus::PrivateBus;
use inferd::audit::Memory;
use inferd::clock::FixedClock;
use inferd::engines::Engines;
use inferd::peers::{Caller, Role, TablePeers};
use inferd::service::{Inference, serve_on};
use inferd::settings::{ConfigFile, InferdSettings, Reload, serve_settings};
use inferd::supervise::Supervised;
use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::{
    AccountId, AppId, AppName, Billing, Isolation, Locality, ModelId, Tokens, UnixSeconds,
};
use porter_dbus::{INFERENCE_BUS, INFERENCE_PATH, INFERENCE_SETTINGS_PATH, InferenceProxy};
use porter_infer::{Pick, Policy, Slot, TierMap};
use std::path::PathBuf;
use std::sync::Arc;
use zbus::Connection;

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
        let bus = PrivateBus::start();
        let file = bus.scratch().join("inferd.toml");
        let engines = Engines::new(
            Vec::new(),
            Supervised::idle(),
            Policy::proposed(),
            TierMap::default(),
        )
        .with_remote(vec![sonnet()]);
        let reload = Reload::new(ConfigFile::new(file.clone()), engines.clone());
        let daemon = bus.connect().await;
        let peers = Arc::new(TablePeers::new());
        serve_on(
            &daemon,
            Inference::new(
                engines.clone(),
                Arc::clone(&peers),
                Memory::default(),
                FixedClock(UnixSeconds(1_700_000_000)),
            )
            .reloading(reload.clone()),
        )
        .await
        .expect("serve Inference1");
        serve_settings(&daemon, InferdSettings::new(Arc::clone(&peers), reload))
            .await
            .expect("serve the settings module");
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
        LiveClient::new(connection, INFERENCE_BUS, INFERENCE_SETTINGS_PATH)
            .await
            .expect("client")
    }
}

fn key(path: &str) -> KeyPath {
    KeyPath(path.to_owned())
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
            "ai.model.text.best"
        ]
    );
    for spec in &schema.key {
        // The choices: the catalogue default, Automatic, and the models inferd knows.
        assert_eq!(
            spec.kind,
            KeyKind::Menu {
                variants: vec![String::new(), "auto".into(), "anthropic/sonnet".into()]
            }
        );
        assert_eq!(spec.default, text(""));
        assert_eq!(spec.page, Page::Intelligence);
        assert_eq!(spec.section.0, "Models");
        assert_eq!(spec.agent, AgentSetting::HandsOff);
        assert_eq!(spec.labels.of(""), Some("Catalogue default"));
        assert_eq!(spec.labels.of("auto"), Some("Automatic"));
        assert_eq!(spec.labels.of("anthropic/sonnet"), Some("sonnet"));
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
    assert!(matches!(in_force(), Some(Pick::Auto(_))));
    settings.set(&slot, &text("")).await.expect("set");
    assert_eq!(in_force(), None);
    assert_eq!(settings.get(&slot).await.expect("get"), text(""));
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
        Some(Pick::Auto(_))
    ));
}
