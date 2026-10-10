//! Where the assistant may run, end to end on a private bus: `Inference1.Places` lists this
//! computer, the person's own computers and the cloud accounts, for the three callers that may
//! ask and nobody else.

use crate::hosting;

use hosting::accountd::{FakeAccount, Standing};
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::lab::Lab;
use hosting::rig::{Hosted, Plan, Trust, World};
use inferd::attached::{Attached, Place, Reach};
use inferd::peers::{Caller, Role};
use porter_client::{ClientError, TransportError};
use porter_core::{AppId, AppName, DataClass, Isolation, ModelId, Tier};
use porter_fake_servers::net::Bind;
use porter_infer::{ComputerName, PlaceKind, PlaceRow, PlaceState};

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

/// A world with one model on this computer, a lab machine called `lab` with one model, and two
/// cloud accounts: a working one and one that has to be signed in again.
async fn world(lab_dir: &str, role: Role, who: &str) -> (World, Lab) {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pl-{}-{lab_dir}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    let lab = Lab::start(&Bind::Socket(dir), "lab", &[entries::SERVED], None).await;
    let plan = Plan {
        catalog: vec![
            ("a-attached.toml", entries::attached()),
            ("b-chat.toml", entries::chat()),
            ("c-claude.toml", entries::claude()),
            ("d-luna.toml", entries::luna()),
        ],
        attached: vec![Attached {
            id: ModelId::parse(entries::ATTACHED).expect("id"),
            reach: Reach::Socket(lab.socket()),
            key_file: None,
            place: Place::MyNetwork,
            computer: Some(ComputerName::parse("lab").expect("name")),
        }],
        role,
        app: Some(app(who)),
        hosted: Some(Hosted {
            accounts: vec![
                FakeAccount {
                    id: "openrouter",
                    standing: Standing::Ask,
                },
                FakeAccount {
                    id: "openai",
                    standing: Standing::Ask,
                },
            ],
            chat: Script {
                chat: vec![Chat::Say(vec!["hi"])],
                dims: 0,
            },
            trust: Trust::ScratchCa,
        }),
        ..Plan::default()
    };
    let world = World::start(plan).await;
    let accountd = world.accountd.as_ref().expect("accountd");
    accountd.describe("openrouter", "Spare router", Some("OpenRouter"), "ok");
    accountd.describe("openai", "Work OpenAI", Some("OpenAI"), "needs_reauth");
    (world, lab)
}

fn row<'a>(rows: &'a [PlaceRow], id: &str) -> &'a PlaceRow {
    rows.iter()
        .find(|one| one.id.as_str() == id)
        .unwrap_or_else(|| panic!("no place {id} in {:?}", ids(rows)))
}

fn ids(rows: &[PlaceRow]) -> Vec<&str> {
    rows.iter().map(|one| one.id.as_str()).collect()
}

fn models(row: &PlaceRow) -> Vec<&str> {
    row.models.iter().map(|one| one.id.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_lists_this_computer_a_named_computer_and_the_cloud_accounts() {
    let (world, _lab) = world("list", Role::Settings, "org.quire.Settings").await;
    let rows = world.accounts.places().await.expect("places");
    assert_eq!(
        ids(&rows),
        vec![
            "this-computer",
            "computer:lab",
            "account:openrouter",
            "account:openai"
        ]
    );
    let here = row(&rows, "this-computer");
    assert_eq!(here.kind, PlaceKind::ThisComputer);
    assert_eq!(here.provider, None);
    assert_eq!(here.state, PlaceState::Ready);
    assert_eq!(models(here), vec!["tiny-chat"]);
    assert_eq!(here.models[0].name, "Tiny chat");

    let lab = row(&rows, "computer:lab");
    assert_eq!(lab.kind, PlaceKind::OwnComputer);
    assert_eq!(lab.name, "lab");
    assert_eq!(lab.state, PlaceState::Ready);
    assert_eq!(models(lab), vec![entries::ATTACHED]);

    let router = row(&rows, "account:openrouter");
    assert_eq!(router.kind, PlaceKind::CloudAccount);
    assert_eq!(router.name, "Spare router");
    assert_eq!(router.provider.as_deref(), Some("OpenRouter"));
    assert_eq!(router.state, PlaceState::Ready);
    assert_eq!(models(router), vec!["claude-opus-5.5", "gpt-6-luna"]);
    assert_eq!(router.models[0].name, "Claude Opus 5.5");

    // An account that must be signed in again is a place, but serves nothing now.
    let stale = row(&rows, "account:openai");
    assert_eq!(stale.name, "Work OpenAI");
    assert_eq!(stale.state, PlaceState::NotReady);
    assert_eq!(models(stale), Vec::<&str>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_that_does_not_answer_is_listed_but_not_ready() {
    let (world, lab) = world("down", Role::Settings, "org.quire.Settings").await;
    drop(lab);
    let rows = world.accounts.places().await.expect("places");
    let down = row(&rows, "computer:lab");
    assert_eq!(down.state, PlaceState::NotReady);
    assert_eq!(models(down), Vec::<&str>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_settings_the_shell_and_the_companion_may_list_the_places() {
    let (world, _lab) = world("callers", Role::Settings, "org.quire.Settings").await;
    let me = world.client.unique_name().expect("unique name").to_string();
    let cases = [
        (Role::Settings, "org.quire.Settings", true),
        (Role::Shell, "org.quire.Shell", true),
        (Role::Placer, "org.quire.Companion", true),
        (Role::Placer, "org.quire.Reader", false),
        (Role::Placer, "org.quire.Intents", false),
        (Role::App, "org.quire.Memory", false),
        (Role::Cua, "org.quire.Cua", false),
        (Role::AgentLauncher, "org.quire.AgentLauncher", false),
    ];
    for (role, name, allowed) in cases {
        world.peers.introduce(
            &me,
            Caller {
                app: app(name),
                role,
            },
        );
        let got = world.accounts.places().await;
        match (allowed, got) {
            (true, Ok(rows)) => assert!(!rows.is_empty(), "{name}"),
            (false, Err(ClientError::Transport(TransportError::Denied(_)))) => {}
            (_, other) => panic!("{name} as {role:?}: {other:?}"),
        }
    }
    // Nobody the table names is refused too.
    let stranger = world.bus.connect().await;
    let accounts = porter_client::Accounts::over(porter_client::DbusTransport::over(stranger));
    assert!(matches!(
        accounts.places().await,
        Err(ClientError::Transport(TransportError::Denied(_)))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_may_list_the_places_and_ask_for_no_model() {
    let (world, _lab) = world("shell", Role::Shell, "org.quire.Shell").await;
    assert!(world.accounts.places().await.is_ok());
    let need = porter_core::Need::Llm(porter_core::need::LlmNeed::new(
        [porter_core::capability::LlmFeature::Chat].into(),
        porter_core::Tokens(1000),
    ));
    let opened = world
        .accounts
        .session(&need, DataClass::Public, Tier::Balanced)
        .await;
    assert!(
        matches!(
            opened,
            Err(ClientError::Transport(TransportError::Denied(_)))
        ),
        "{:?}",
        opened.map(|_| ())
    );
    let prepared = world
        .accounts
        .prepare(
            &need,
            DataClass::Public,
            Tier::Balanced,
            &porter_infer::OpenOptions::default(),
        )
        .await;
    assert!(matches!(
        prepared,
        Err(ClientError::Transport(TransportError::Denied(_)))
    ));
}

/// An account that appears, goes or changes state is told to inferd by accountd (a joined porter
/// daemon hears it); inferd says `EnginesChanged`, so a listener re-reads `Places`. The fake
/// accountd sends the signal accountd's roster would.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_that_appears_makes_inferd_say_engines_changed() {
    use porter_dbus::{BusStream, InferenceProxy};
    let (world, _lab) = world("news", Role::Settings, "org.quire.Settings").await;
    let proxy = InferenceProxy::new(&world.client).await.expect("proxy");
    let mut changed = proxy.receive_engines_changed().await.expect("stream");
    let held = world.accountd_bus.as_ref().expect("accountd's connection");
    let daemon = world.daemon.unique_name().expect("name").to_owned();
    let path = porter_dbus::zvariant::ObjectPath::try_from("/org/quire/Accounts1/account/new")
        .expect("path");
    let heard = within_generous(async {
        loop {
            held.emit_signal(
                Some(daemon.clone()),
                "/org/quire/Accounts1",
                "org.quire.Accounts1.Manager",
                "AccountAdded",
                &(&path,),
            )
            .await
            .expect("signal");
            let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut changed).poll_next(cx));
            if let Ok(Some(_)) =
                tokio::time::timeout(std::time::Duration::from_millis(150), next).await
            {
                return;
            }
        }
    })
    .await;
    assert!(heard, "EnginesChanged after an account appeared");
}

async fn within_generous(work: impl std::future::Future<Output = ()>) -> bool {
    tokio::time::timeout(porter_fake::GENEROUS, work)
        .await
        .is_ok()
}

// ---- routing inside the allowed set -------------------------------------------------------

mod routing {
    use super::*;
    use hosting::bus::within;
    use inferd::router::placed::Allowed;
    use inferd::session::SessionSpec;
    use porter_client::InferSession;
    use porter_core::capability::LlmFeature;
    use porter_core::consent::Usage;
    use porter_core::need::LlmNeed;
    use porter_core::{Need, Tokens};
    use porter_dbus::InferenceProxy;
    use porter_infer::{
        ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRequest, LocalOnly, MessagePart,
        NoPlaceReason, OpenOptions, PlaceId, Policy, Role as ChatRole,
    };
    use std::collections::BTreeMap;

    const COMPANION: &str = "org.quire.Companion";

    fn llm() -> Need {
        Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(1000)))
    }

    fn chat_request() -> InferRequest {
        InferRequest::Chat(ChatRequest::new(
            vec![ChatMessage {
                role: ChatRole::User,
                parts: vec![MessagePart::Text("hello".into())],
            }],
            Tier::Balanced,
            DataClass::Public,
            Usage::Interactive,
        ))
    }

    fn place(text: &str) -> PlaceId {
        PlaceId::parse(text).expect("place")
    }

    /// A local chat model, a lab machine called `lab` with one model, and OpenRouter granted to
    /// the companion; the caller is the companion (a placer) unless the test says otherwise.
    pub async fn placed_world(dir: &str, role: Role, who: &str) -> (World, Lab) {
        placed_world_with(dir, role, who, true).await
    }

    /// `placed_world`, with the local chat model only when `local` says so.
    pub async fn placed_world_with(dir: &str, role: Role, who: &str, local: bool) -> (World, Lab) {
        let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pr-{}-{dir}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("scratch");
        let lab = Lab::start(&Bind::Socket(scratch), "lab", &[entries::SERVED], None).await;
        let mut catalog = vec![
            ("a-attached.toml", entries::attached()),
            ("c-claude.toml", entries::claude()),
        ];
        let mut scripts = Vec::new();
        if local {
            catalog.push(("b-chat.toml", entries::chat()));
            scripts.push((
                "tiny-chat",
                Script {
                    chat: vec![Chat::Say(vec!["local"])],
                    dims: 0,
                },
            ));
        }
        let plan = Plan {
            catalog,
            scripts,
            attached: vec![Attached {
                id: ModelId::parse(entries::ATTACHED).expect("id"),
                reach: Reach::Socket(lab.socket()),
                key_file: None,
                place: Place::MyNetwork,
                computer: Some(ComputerName::parse("lab").expect("name")),
            }],
            policy: Policy {
                local_only: LocalOnly::Off,
                floors: Vec::new(),
            },
            role,
            app: Some(app(who)),
            hosted: Some(Hosted {
                accounts: vec![FakeAccount {
                    id: "openrouter",
                    standing: Standing::Granted {
                        to: vec![COMPANION, "org.quire.Memory"],
                        grant: "grant-openrouter",
                        key: Some("sk-or-test-key"),
                    },
                }],
                chat: Script {
                    chat: vec![Chat::Say(vec!["cloud"])],
                    dims: 0,
                },
                trust: Trust::ScratchCa,
            }),
            ..Plan::default()
        };
        (World::start(plan).await, lab)
    }

    /// The account the session was routed to, for a first turn opened with `options`.
    async fn served_by(world: &World, options: &OpenOptions) -> String {
        let mut session = within(
            "Open",
            world
                .accounts
                .session_with(&llm(), DataClass::Public, Tier::Balanced, options),
        )
        .await
        .expect("open");
        let _ = session.send(ClientFrame::Request(chat_request())).await;
        loop {
            match within("the next event", session.next())
                .await
                .expect("event")
            {
                InferEvent::Routed(served) => return served.account.to_string(),
                InferEvent::Finished(reply) => panic!("no route: {reply:?}"),
                _ => {}
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_places_routing_is_as_it_always_was_and_with_them_it_is_the_callers() {
        let (world, _lab) = placed_world("order", Role::Placer, COMPANION).await;
        // Today: this computer before the cloud.
        assert_eq!(served_by(&world, &OpenOptions::default()).await, "local");
        // The caller's order decides inside the set.
        let cloud_first = OpenOptions::default()
            .with_places(vec![place("account:openrouter"), place("this-computer")]);
        assert_eq!(served_by(&world, &cloud_first).await, "openrouter");
        let local_first = OpenOptions::default()
            .with_places(vec![place("this-computer"), place("account:openrouter")]);
        assert_eq!(served_by(&world, &local_first).await, "local");
        // A pin names the model at a place.
        let pinned = OpenOptions::default()
            .with_places(vec![place("computer:lab")])
            .with_place_model(
                place("computer:lab"),
                ModelId::parse(entries::ATTACHED).expect("id"),
            );
        assert_eq!(served_by(&world, &pinned).await, "local");
    }

    fn spec() -> SessionSpec {
        SessionSpec {
            need: llm(),
            class: DataClass::Public,
            tier: Tier::Balanced,
            usage: Usage::Interactive,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cloud_account_outside_the_set_is_never_used_even_when_it_is_the_only_one_able() {
        // No model on this computer: the account is the only place able to serve, and it is not
        // in the set.
        let (world, lab) = placed_world_with("outside", Role::Placer, COMPANION, false).await;
        drop(lab);
        let caller = inferd::peers::Caller {
            app: app(COMPANION),
            role: Role::Placer,
        };
        let allowed = Allowed::new(vec![place("computer:lab")], BTreeMap::new());
        let spec = spec();
        let offered = world
            .served
            .offer(&caller.app, spec.class, spec.usage)
            .await;
        let refused = world
            .served
            .route_in(&spec, caller.role, &offered, Some(&allowed))
            .expect_err("nothing in the set can serve");
        assert_eq!(
            refused,
            inferd::router::placed::Unplaced::NoPlace(porter_infer::PlaceRefusal::new(
                NoPlaceReason::NotReady,
                Some(porter_infer::PlaceKind::CloudAccount),
            ))
        );
        // Whatever the answer for the session, nothing was sent to the provider.
        assert!(
            world
                .provider
                .as_ref()
                .expect("provider")
                .requests()
                .is_empty()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn callers_that_may_not_choose_places_are_refused_not_ignored() {
        let (world, _lab) = placed_world("refused", Role::Settings, "org.quire.Settings").await;
        let me = world.client.unique_name().expect("unique name").to_string();
        let options = OpenOptions::default().with_places(vec![place("this-computer")]);
        for (role, name) in [
            (Role::Settings, "org.quire.Settings"),
            (Role::App, "org.quire.Memory"),
            (Role::Cua, "org.quire.Cua"),
        ] {
            world.peers.introduce(
                &me,
                inferd::peers::Caller {
                    app: app(name),
                    role,
                },
            );
            let denied = |what: &str, got: Result<(), ClientError>| {
                assert!(
                    matches!(got, Err(ClientError::Transport(TransportError::Denied(_)))),
                    "{name} {what}: {got:?}"
                );
            };
            denied(
                "open",
                world
                    .accounts
                    .session_with(&llm(), DataClass::Public, Tier::Balanced, &options)
                    .await
                    .map(|_| ()),
            );
            denied(
                "prepare",
                world
                    .accounts
                    .prepare(&llm(), DataClass::Public, Tier::Balanced, &options)
                    .await
                    .map(|_| ()),
            );
            // The same call without the key is served.
            assert!(
                world
                    .accounts
                    .prepare(
                        &llm(),
                        DataClass::Public,
                        Tier::Balanced,
                        &OpenOptions::default()
                    )
                    .await
                    .is_ok(),
                "{name}"
            );
        }
        // A placer is let through.
        world.peers.introduce(
            &me,
            inferd::peers::Caller {
                app: app(COMPANION),
                role: Role::Placer,
            },
        );
        assert!(
            world
                .accounts
                .prepare(&llm(), DataClass::Public, Tier::Balanced, &options)
                .await
                .is_ok()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_places_option_that_is_not_what_the_interface_says_is_invalid_args() {
        let (world, _lab) = placed_world("invalid", Role::Placer, COMPANION).await;
        let proxy = InferenceProxy::new(&world.client).await.expect("proxy");
        let need = porter_dbus::need_to_dbus(&llm());
        let text = |value: &str| {
            porter_dbus::zvariant::OwnedValue::try_from(porter_dbus::zvariant::Value::from(
                value.to_owned(),
            ))
            .expect("value")
        };
        let list = |values: &[&str]| {
            porter_dbus::zvariant::OwnedValue::try_from(porter_dbus::zvariant::Value::new(
                values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>(),
            ))
            .expect("value")
        };
        let pins = |place: &str, model: &str| {
            porter_dbus::zvariant::OwnedValue::try_from(porter_dbus::zvariant::Value::new(
                std::collections::HashMap::from([(place.to_owned(), model.to_owned())]),
            ))
            .expect("value")
        };
        let cases: Vec<(&str, porter_dbus::Details)> = vec![
            (
                "a bad place id",
                [("places".to_owned(), list(&["somewhere"]))].into(),
            ),
            (
                "places as text",
                [("places".to_owned(), text("this-computer"))].into(),
            ),
            (
                "pins without places",
                [("place_models".to_owned(), pins("this-computer", "m"))].into(),
            ),
            (
                "a pin outside the set",
                [
                    ("places".to_owned(), list(&["this-computer"])),
                    ("place_models".to_owned(), pins("account:x", "m")),
                ]
                .into(),
            ),
            (
                "a pin to a bad model",
                [
                    ("places".to_owned(), list(&["this-computer"])),
                    (
                        "place_models".to_owned(),
                        pins("this-computer", "Not A Model"),
                    ),
                ]
                .into(),
            ),
        ];
        for (what, options) in cases {
            let got = proxy
                .availability(&need, "public", &options)
                .await
                .expect_err(what);
            assert!(porter_dbus::is_invalid_args(&got), "{what}: {got:?}");
        }
    }
}

// ---- the typed refusal ---------------------------------------------------------------------

mod refusals {
    use super::routing::placed_world_with;
    use super::*;
    use hosting::rig::World;
    use porter_core::capability::{LlmFeature, Modality};
    use porter_core::need::{DimsNeed, EmbedNeed, LlmNeed};
    use porter_core::{Need, Tokens};
    use porter_dbus::{InferenceProxy, PLACE_ERROR_PREFIX, place_refusal_of};
    use porter_infer::{LocalOnly, NoPlaceReason, OpenOptions, PlaceId, Policy};

    const COMPANION: &str = "org.quire.Companion";

    fn llm() -> Need {
        Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(1000)))
    }

    fn embeddings() -> Need {
        Need::Embeddings(EmbedNeed::new(DimsNeed::Any, [Modality::Text].into()))
    }

    fn only(places: &[&str]) -> OpenOptions {
        OpenOptions::default().with_places(
            places
                .iter()
                .map(|text| PlaceId::parse(text).expect("place"))
                .collect(),
        )
    }

    fn refused(reason: NoPlaceReason, would_need: Option<PlaceKind>) -> ClientError {
        ClientError::Transport(TransportError::NoAllowedPlace { reason, would_need })
    }

    /// The same refusal from `Prepare` and from `Open`, through porter-client.
    async fn both_ways(
        world: &World,
        need: &Need,
        class: DataClass,
        options: &OpenOptions,
        want: ClientError,
    ) {
        let prepared = world
            .accounts
            .prepare(need, class, Tier::Balanced, options)
            .await;
        assert_eq!(prepared, Err(want.clone()), "Prepare");
        let opened = world
            .accounts
            .session_with(need, class, Tier::Balanced, options)
            .await
            .map(|_| ());
        assert_eq!(opened, Err(want), "Open");
    }

    /// Floors as shipped, with the switch that keeps everything on this computer off.
    fn floors_on(world: &World) {
        let mut settings = (*world.served.settings()).clone();
        settings.policy = Policy {
            local_only: LocalOnly::Off,
            ..Policy::proposed()
        };
        world.served.apply(settings);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn not_ready_names_the_cloud_account_that_could_have_served_and_it_is_not_used() {
        let (world, lab) = placed_world_with("r-notready", Role::Placer, COMPANION, false).await;
        drop(lab);
        both_ways(
            &world,
            &llm(),
            DataClass::Public,
            &only(&["computer:lab"]),
            refused(NoPlaceReason::NotReady, Some(PlaceKind::CloudAccount)),
        )
        .await;
        assert!(
            world
                .provider
                .as_ref()
                .expect("provider")
                .requests()
                .is_empty()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn not_ready_without_a_place_outside_that_could_serve_has_no_would_need() {
        // The reader holds no grant on the account, so the account could not have served either.
        let (world, lab) =
            placed_world_with("r-nowould", Role::Placer, "org.quire.Reader", false).await;
        drop(lab);
        both_ways(
            &world,
            &llm(),
            DataClass::Public,
            &only(&["computer:lab"]),
            refused(NoPlaceReason::NotReady, None),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn floor_refused_when_the_class_may_not_leave_and_would_need_follows_the_floor() {
        let (world, _lab) = placed_world_with("r-floor", Role::Placer, COMPANION, true).await;
        floors_on(&world);
        // The prompt may not leave this computer, and the only allowed place is the account.
        both_ways(
            &world,
            &llm(),
            DataClass::Prompt,
            &only(&["account:openrouter"]),
            refused(NoPlaceReason::FloorRefused, Some(PlaceKind::ThisComputer)),
        )
        .await;
        // Local-only on is the same reason.
        let mut settings = (*world.served.settings()).clone();
        settings.policy = Policy::proposed();
        world.served.apply(settings);
        both_ways(
            &world,
            &llm(),
            DataClass::Public,
            &only(&["account:openrouter"]),
            refused(NoPlaceReason::FloorRefused, Some(PlaceKind::ThisComputer)),
        )
        .await;
        assert!(
            world
                .provider
                .as_ref()
                .expect("provider")
                .requests()
                .is_empty()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn floor_refused_has_no_would_need_when_nothing_outside_could_serve() {
        let (world, _lab) = placed_world_with("r-floor2", Role::Placer, COMPANION, false).await;
        floors_on(&world);
        both_ways(
            &world,
            &llm(),
            DataClass::Prompt,
            &only(&["account:openrouter"]),
            refused(NoPlaceReason::FloorRefused, None),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn model_not_offered_when_the_pinned_model_is_not_at_the_place() {
        let (world, _lab) = placed_world_with("r-pin", Role::Placer, COMPANION, true).await;
        let options = only(&["this-computer"]).with_place_model(
            PlaceId::this_computer(),
            porter_core::ModelId::parse("no-such-model").expect("id"),
        );
        // The lab machine and the account, outside the set, could have served.
        both_ways(
            &world,
            &llm(),
            DataClass::Public,
            &options,
            refused(NoPlaceReason::ModelNotOffered, Some(PlaceKind::OwnComputer)),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn none_capable_when_no_allowed_place_does_this_kind_of_work() {
        let (world, _lab) = placed_world_with("r-none", Role::Placer, COMPANION, true).await;
        // A place nothing is known of: it can do nothing, and this computer, outside, could.
        both_ways(
            &world,
            &llm(),
            DataClass::Public,
            &only(&["computer:ghost"]),
            refused(NoPlaceReason::NoneCapable, Some(PlaceKind::ThisComputer)),
        )
        .await;
        // Embeddings: the account has none, and nothing outside has either.
        both_ways(
            &world,
            &embeddings(),
            DataClass::Public,
            &only(&["account:openrouter"]),
            refused(NoPlaceReason::NoneCapable, None),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_empty_set_serves_nothing() {
        let (world, _lab) = placed_world_with("r-empty", Role::Placer, COMPANION, true).await;
        both_ways(
            &world,
            &llm(),
            DataClass::Public,
            &only(&[]),
            refused(NoPlaceReason::NoneCapable, Some(PlaceKind::ThisComputer)),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_reply_is_an_error_named_for_the_reason_with_plain_words_and_the_kind_in_its_body()
    {
        let (world, lab) = placed_world_with("r-wire", Role::Placer, COMPANION, false).await;
        drop(lab);
        let proxy = InferenceProxy::new(&world.client).await.expect("proxy");
        let options = match porter_dbus::zvariant::OwnedValue::try_from(
            porter_dbus::zvariant::Value::new(vec!["computer:lab".to_owned()]),
        ) {
            Ok(value) => [("places".to_owned(), value)].into(),
            Err(e) => panic!("{e}"),
        };
        let need = porter_dbus::need_to_dbus(&llm());
        let error = proxy
            .availability(&need, "public", &options)
            .await
            .expect_err("no allowed place");
        let zbus::Error::MethodError(name, text, _) = &error else {
            panic!("a method error, got {error:?}");
        };
        assert_eq!(
            name.as_str(),
            format!("{PLACE_ERROR_PREFIX}NotReady"),
            "reason-specific name"
        );
        assert_eq!(
            name.as_str(),
            "org.quire.Inference1.Error.NoAllowedPlace.NotReady"
        );
        let words = text.as_deref().expect("words");
        assert_eq!(
            words,
            "None of the places you allowed is ready to do this right now."
        );
        for jargon in ["would_need", "cloud_account", "NoAllowedPlace", "="] {
            assert!(!words.contains(jargon), "{words}");
        }
        assert_eq!(
            place_refusal_of(&error),
            Some(("NotReady".to_owned(), Some("cloud_account".to_owned())))
        );
        // And the body is exactly two strings: the words, then the kind.
        let zbus::Error::MethodError(_, _, message) = &error else {
            unreachable!()
        };
        let body: (String, String) = message.body().deserialize().expect("two strings");
        assert_eq!(body.1, "cloud_account");
        // Another error of the same interface is not mistaken for one.
        let denied = proxy
            .availability(&need, "no-such-class", &porter_dbus::Details::new())
            .await
            .expect_err("invalid");
        assert_eq!(place_refusal_of(&denied), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_set_that_can_serve_is_not_refused() {
        let (world, _lab) = placed_world_with("r-ok", Role::Placer, COMPANION, true).await;
        let options = only(&["this-computer"]);
        let prepared = world
            .accounts
            .prepare(&llm(), DataClass::Public, Tier::Balanced, &options)
            .await;
        assert!(prepared.is_ok(), "{prepared:?}");
        let opened = world
            .accounts
            .session_with(&llm(), DataClass::Public, Tier::Balanced, &options)
            .await
            .map(|_| ());
        assert_eq!(opened, Ok(()));
    }
}

// ---- computers added in Settings -----------------------------------------------------------

mod computers {
    use super::routing::placed_world_with;
    use super::*;
    use hosting::bus::within;
    use inferd::peers::Caller;
    use porter_client::InferSession;
    use porter_dbus::zvariant::{OwnedValue, Value};
    use porter_dbus::{BusStream, COMPUTER_ERROR_PREFIX, Details, InferenceProxy};
    use porter_infer::{
        ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRequest, LocalOnly, MessagePart,
        OpenOptions, PlaceId, Policy, Role as ChatRole,
    };
    use std::path::Path;

    const KEY: &str = "sk-lab-S3CRET-0123456789";
    const COMPANION: &str = "org.quire.Companion";

    fn text(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value.to_owned())).expect("value")
    }

    /// One model of `AddComputer`: its id, and how it is reached and its key.
    fn model(
        id: &str,
        socket: Option<&Path>,
        port: Option<u16>,
        key: Option<&str>,
    ) -> (String, Details) {
        let mut details = Details::new();
        if let Some(socket) = socket {
            details.insert("socket".to_owned(), text(&socket.display().to_string()));
        }
        if let Some(port) = port {
            details.insert(
                "port".to_owned(),
                OwnedValue::try_from(Value::U16(port)).expect("value"),
            );
        }
        if let Some(key) = key {
            details.insert("key".to_owned(), text(key));
        }
        (id.to_owned(), details)
    }

    /// A world with no engine written by hand, and a lab machine that wants `KEY`.
    async fn world(name: &str) -> (World, Lab) {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("cp-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let lab = Lab::start(&Bind::Socket(dir), "lab", &[entries::SERVED], Some(KEY)).await;
        let plan = Plan {
            catalog: vec![("a-attached.toml", entries::attached())],
            policy: Policy {
                local_only: LocalOnly::Off,
                floors: Vec::new(),
            },
            role: Role::Settings,
            app: Some(app("org.quire.Settings")),
            ..Plan::default()
        };
        (World::start(plan).await, lab)
    }

    fn as_caller(world: &World, role: Role, who: &str) {
        let me = world.client.unique_name().expect("unique name").to_string();
        world.peers.introduce(
            &me,
            Caller {
                app: app(who),
                role,
            },
        );
    }

    async fn proxy(world: &World) -> InferenceProxy<'static> {
        InferenceProxy::new(&world.client).await.expect("proxy")
    }

    fn name_and_words(error: &zbus::Error) -> (String, String) {
        match error {
            zbus::Error::MethodError(name, words, _) => {
                (name.to_string(), words.clone().unwrap_or_default())
            }
            other => panic!("a method error, got {other:?}"),
        }
    }

    fn chat_request() -> InferRequest {
        InferRequest::Chat(ChatRequest::new(
            vec![ChatMessage {
                role: ChatRole::User,
                parts: vec![MessagePart::Text("hello".into())],
            }],
            Tier::Balanced,
            DataClass::Public,
            porter_core::consent::Usage::Interactive,
        ))
    }

    fn files_holding(dir: &Path, needle: &str) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(files_holding(&path, needle));
            } else if std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle)) {
                found.push(path);
            }
        }
        found
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn settings_adds_a_computer_that_places_lists_and_the_assistant_uses_with_its_key() {
        let (world, lab) = world("add").await;
        lab.say(&["from the studio"]);
        let proxy = proxy(&world).await;
        let mut changed = proxy.receive_engines_changed().await.expect("stream");
        let placed = within(
            "AddComputer",
            proxy.add_computer(
                "Studio PC",
                vec![model(
                    entries::ATTACHED,
                    Some(&lab.socket()),
                    None,
                    Some(KEY),
                )],
            ),
        )
        .await
        .expect("added");
        assert_eq!(placed, "computer:studio-pc");
        let told = std::future::poll_fn(|cx| std::pin::Pin::new(&mut changed).poll_next(cx));
        assert!(
            tokio::time::timeout(porter_fake::GENEROUS, told)
                .await
                .is_ok(),
            "EnginesChanged after an addition"
        );

        // Places shows it, under the name the person gave, ready, with its model.
        let rows = world.accounts.places().await.expect("places");
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == "computer:studio-pc")
            .unwrap_or_else(|| panic!("the new computer in {rows:?}"));
        assert_eq!(row.name, "Studio PC");
        assert_eq!(row.state, porter_infer::PlaceState::Ready);
        assert_eq!(
            row.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec![entries::ATTACHED]
        );

        // A companion that names the place is answered by it, and the lab saw the key.
        as_caller(&world, Role::Placer, COMPANION);
        let options = OpenOptions::default()
            .with_places(vec![PlaceId::parse("computer:studio-pc").expect("place")]);
        let need = porter_core::Need::Llm(porter_core::need::LlmNeed::new(
            [porter_core::capability::LlmFeature::Chat].into(),
            porter_core::Tokens(1000),
        ));
        let mut session = within(
            "Open",
            world
                .accounts
                .session_with(&need, DataClass::Public, Tier::Balanced, &options),
        )
        .await
        .expect("open");
        let _ = session.send(ClientFrame::Request(chat_request())).await;
        loop {
            let event = within("an event", session.next()).await.expect("event");
            if matches!(event, InferEvent::Finished(_)) {
                break;
            }
        }
        assert_eq!(lab.chats().len(), 1);
        let bearer = format!("Bearer {KEY}");
        assert!(
            lab.seen()
                .iter()
                .any(|seen| seen.authorization.as_deref() == Some(bearer.as_str())),
            "the lab was sent the key"
        );

        // The key is in one file, readable by its owner alone, and nowhere else: not in the
        // list of computers, not in what inferd logged, not in any reply.
        let key_file = world
            .state
            .join("computer-keys")
            .join(format!("studio-pc--{}.key", entries::ATTACHED));
        assert_eq!(std::fs::read_to_string(&key_file).expect("key file"), KEY);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&key_file).expect("meta").permissions(),
        ) & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(files_holding(&world.scratch, KEY), vec![key_file]);
        let lines = format!("{:?}", world.log.lock().expect("lock"));
        assert!(!lines.contains(KEY), "{lines}");
        assert!(!format!("{rows:?} {placed}").contains(KEY));
        let list = std::fs::read_to_string(world.state.join("computers.toml")).expect("list");
        assert!(list.contains("Studio PC") && !list.contains(KEY), "{list}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn removing_a_computer_takes_it_and_its_key_away_and_says_engines_changed() {
        let (world, lab) = world("remove").await;
        let proxy = proxy(&world).await;
        proxy
            .add_computer(
                "Studio PC",
                vec![model(
                    entries::ATTACHED,
                    Some(&lab.socket()),
                    None,
                    Some(KEY),
                )],
            )
            .await
            .expect("added");
        let mut changed = proxy.receive_engines_changed().await.expect("stream");
        proxy
            .remove_computer("computer:studio-pc")
            .await
            .expect("removed");
        let told = std::future::poll_fn(|cx| std::pin::Pin::new(&mut changed).poll_next(cx));
        assert!(
            tokio::time::timeout(porter_fake::GENEROUS, told)
                .await
                .is_ok(),
            "EnginesChanged after a removal"
        );
        let rows = world.accounts.places().await.expect("places");
        assert!(
            rows.iter()
                .all(|row| row.id.as_str() != "computer:studio-pc")
        );
        assert!(
            !world
                .state
                .join("computer-keys")
                .join(format!("studio-pc--{}.key", entries::ATTACHED))
                .exists()
        );
        // Gone is gone.
        let again = proxy
            .remove_computer("studio-pc")
            .await
            .expect_err("not there");
        assert_eq!(
            name_and_words(&again),
            (
                format!("{COMPUTER_ERROR_PREFIX}NotThere"),
                "There is no computer with that name.".to_owned()
            )
        );
        // And the same name can be used again.
        assert!(
            proxy
                .add_computer(
                    "Studio PC",
                    vec![model(entries::ATTACHED, Some(&lab.socket()), None, None)]
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn what_is_refused_reaches_settings_as_a_named_error_with_a_plain_sentence() {
        let (world, lab) = world("refuse").await;
        let proxy = proxy(&world).await;
        let socket = lab.socket();
        let studio = |models: Vec<(String, Details)>| {
            let proxy = proxy.clone();
            async move { proxy.add_computer("Studio PC", models).await }
        };
        let unreachable = format!(
            "\u{201c}{}\u{201d} cannot be reached that way. Give the full path of a connection on this computer, or a port on this computer.",
            entries::ATTACHED
        );
        let bad_key = format!(
            "The key for \u{201c}{}\u{201d} cannot be used.",
            entries::ATTACHED
        );
        type Case<'a> = (&'a str, Vec<(String, Details)>, &'a str, &'a str);
        let cases: Vec<Case<'_>> = vec![
            (
                "no models",
                vec![],
                "NoModels",
                "Add at least one model for this computer.",
            ),
            (
                "an unknown model",
                vec![model("no-such-model", Some(&socket), None, None)],
                "UnknownModel",
                "The assistant does not know a model called \u{201c}no-such-model\u{201d}.",
            ),
            (
                "no way to reach it",
                vec![model(entries::ATTACHED, None, None, None)],
                "BadAddress",
                &unreachable,
            ),
            (
                "two ways to reach it",
                vec![model(entries::ATTACHED, Some(&socket), Some(8000), None)],
                "BadAddress",
                &unreachable,
            ),
            (
                "a relative path",
                vec![model(
                    entries::ATTACHED,
                    Some(Path::new("lab.sock")),
                    None,
                    None,
                )],
                "BadAddress",
                &unreachable,
            ),
            (
                "an empty key",
                vec![model(entries::ATTACHED, Some(&socket), None, Some(" "))],
                "BadKey",
                &bad_key,
            ),
        ];
        for (what, models, name, words) in cases {
            let error = studio(models).await.expect_err(what);
            assert_eq!(
                name_and_words(&error),
                (format!("{COMPUTER_ERROR_PREFIX}{name}"), words.to_owned()),
                "{what}"
            );
            for jargon in ["socket", "loopback", "TOML", "key_file", "API"] {
                assert!(!words.contains(jargon), "{what}: {words}");
            }
        }
        // A blank name, and a name taken.
        let blank = proxy
            .add_computer(
                "  ",
                vec![model(entries::ATTACHED, Some(&socket), None, None)],
            )
            .await
            .expect_err("blank");
        assert_eq!(
            name_and_words(&blank),
            (
                format!("{COMPUTER_ERROR_PREFIX}BadName"),
                "Give the computer a name of one short line.".to_owned()
            )
        );
        studio(vec![model(entries::ATTACHED, Some(&socket), None, None)])
            .await
            .expect("added");
        let taken = proxy
            .add_computer(
                "studio pc",
                vec![model(entries::ATTACHED, Some(&socket), None, None)],
            )
            .await
            .expect_err("taken");
        assert_eq!(
            name_and_words(&taken),
            (
                format!("{COMPUTER_ERROR_PREFIX}AlreadyThere"),
                "A computer with that name is already there.".to_owned()
            )
        );
        // Nothing refused left anything behind: one computer, one key file or none.
        let rows = world.accounts.places().await.expect("places");
        assert_eq!(
            rows.iter()
                .filter(|row| row.kind == PlaceKind::OwnComputer)
                .count(),
            1
        );
        // A value of the wrong type is invalid args, not one of ours.
        let mut wrong = model(entries::ATTACHED, None, None, None);
        wrong.1.insert("port".to_owned(), text("eight thousand"));
        let invalid = proxy
            .add_computer("Other", vec![wrong])
            .await
            .expect_err("invalid");
        assert!(porter_dbus::is_invalid_args(&invalid), "{invalid:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_computer_written_by_hand_is_listed_but_cannot_be_removed_or_added_again() {
        let (world, _lab) =
            placed_world_with("hand", Role::Settings, "org.quire.Settings", false).await;
        let proxy = proxy(&world).await;
        let rows = world.accounts.places().await.expect("places");
        assert!(rows.iter().any(|row| row.id.as_str() == "computer:lab"));
        for name in ["lab", "computer:lab"] {
            let error = proxy.remove_computer(name).await.expect_err("by hand");
            assert_eq!(
                name_and_words(&error),
                (
                    format!("{COMPUTER_ERROR_PREFIX}AddedByHand"),
                    "That computer was added by hand in the settings file, so it can only be removed there."
                        .to_owned()
                ),
                "{name}"
            );
        }
        let again = proxy
            .add_computer("Lab", vec![model("other-model", None, Some(1), None)])
            .await
            .expect_err("taken");
        assert_eq!(
            name_and_words(&again).0,
            format!("{COMPUTER_ERROR_PREFIX}AlreadyThere")
        );
        let model_taken = proxy
            .add_computer("Bench", vec![model(entries::ATTACHED, None, Some(1), None)])
            .await
            .expect_err("model taken");
        assert_eq!(
            name_and_words(&model_taken).0,
            format!("{COMPUTER_ERROR_PREFIX}ModelTaken")
        );
        assert!(world.accounts.places().await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_settings_may_add_or_remove_a_computer() {
        let (world, lab) = world("callers").await;
        let proxy = proxy(&world).await;
        let denied = "org.freedesktop.DBus.Error.AccessDenied";
        for (role, who) in [
            (Role::Placer, COMPANION),
            (Role::Shell, "org.quire.Shell"),
            (Role::App, "org.quire.Memory"),
            (Role::AgentLauncher, "org.quire.AgentLauncher"),
        ] {
            as_caller(&world, role, who);
            let add = proxy
                .add_computer(
                    "Studio PC",
                    vec![model(entries::ATTACHED, Some(&lab.socket()), None, None)],
                )
                .await
                .expect_err(who);
            assert_eq!(name_and_words(&add).0, denied, "{who} add");
            let remove = proxy.remove_computer("studio-pc").await.expect_err(who);
            assert_eq!(name_and_words(&remove).0, denied, "{who} remove");
        }
        as_caller(&world, Role::Settings, "org.quire.Settings");
        let rows = world.accounts.places().await.expect("places");
        assert!(rows.iter().all(|row| row.kind != PlaceKind::OwnComputer));
    }
}
