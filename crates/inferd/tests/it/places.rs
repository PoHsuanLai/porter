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
    let need = porter_core::Need::Llm(porter_core::need::LlmNeed {
        features: [porter_core::capability::LlmFeature::Chat].into(),
        context: porter_core::Tokens(1000),
    });
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
    let heard = within_secs(5, async {
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

async fn within_secs(secs: u64, work: impl std::future::Future<Output = ()>) -> bool {
    tokio::time::timeout(std::time::Duration::from_secs(secs), work)
        .await
        .is_ok()
}
