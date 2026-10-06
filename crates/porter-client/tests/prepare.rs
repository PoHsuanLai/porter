//! `Transport::prepare`: over the bus it is `Inference1.Prepare` (the need, class, tier and trace
//! context cross as arguments, the slug that comes back is a `Readiness`), in process it asks
//! the host, and a transport that has no inferd says so.
#![cfg(feature = "dbus")]

mod common;

use common::bus::PrivateBus;
use common::inferd::{Behaviour, FakeInferd, Seen, slugs};
use porter_client::{
    Accounts, ClientError, DbusTransport, InProcess, NoBroker, SessionHost, TransportError,
};
use porter_core::capability::SpeechMode;
use porter_core::consent::Usage;
use porter_core::need::SpeechNeed;
use porter_core::{AppId, AppName, DataClass, Isolation, Need, Permille, Tier};
use porter_fake::{FakeInferSession, FakeService, ScriptedSheets, fake_service};
use porter_infer::{OpenOptions, Readiness, Traceparent};
use std::sync::{Arc, Mutex};

fn stt() -> Need {
    Need::Speech(SpeechNeed {
        modes: [SpeechMode::Stt].into(),
    })
}

async fn bus_rig(
    slug: &str,
) -> (
    PrivateBus,
    zbus::Connection,
    Arc<Mutex<Seen>>,
    Accounts<DbusTransport>,
) {
    let bus = PrivateBus::start();
    let (fake, seen) = FakeInferd::new(Behaviour::Scripted(vec![]));
    let daemon = bus.connect().await;
    fake.preparing(slug).serve(&daemon).await;
    let accounts = Accounts::over(DbusTransport::over(bus.connect().await));
    (bus, daemon, seen, accounts)
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_over_the_bus_carries_the_arguments_and_reads_the_readiness_slug() {
    let cases = [
        ("ready", Ok(Readiness::Ready)),
        ("loading", Ok(Readiness::Loading)),
        ("loadable", Ok(Readiness::Loadable)),
        ("downloading", Ok(Readiness::Downloading(Permille(0)))),
        ("downloadable", Ok(Readiness::Downloadable)),
        ("unavailable", Ok(Readiness::Unavailable)),
    ];
    for (slug, expected) in cases {
        let (_bus, _daemon, _seen, accounts) = bus_rig(slug).await;
        let got = accounts
            .prepare(
                &stt(),
                DataClass::Voice,
                Tier::Balanced,
                &OpenOptions::default(),
            )
            .await;
        assert_eq!(got, expected, "{slug}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_sends_need_class_tier_and_trace_context() {
    let (_bus, _daemon, seen, accounts) = bus_rig("ready").await;
    let parent = Traceparent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        .expect("traceparent");
    let options = OpenOptions {
        traceparent: Some(parent.clone()),
        usage: Some(Usage::Background),
    };
    accounts
        .prepare(&stt(), DataClass::Voice, Tier::Fast, &options)
        .await
        .expect("prepared");
    let (class, tier) = slugs(DataClass::Voice, Tier::Fast);
    let seen = seen.lock().expect("lock");
    assert_eq!(seen.opens.len(), 0, "a prepare opens no session");
    assert_eq!(seen.prepares.len(), 1);
    let call = &seen.prepares[0];
    assert_eq!(call.need, stt());
    assert_eq!(
        (call.class.as_str(), call.tier.as_str()),
        (class.as_str(), tier.as_str())
    );
    assert_eq!(call.traceparent.as_deref(), Some(parent.as_str()));
    assert_eq!(call.usage.as_deref(), Some("background"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_that_inferd_refuses_is_denied_with_the_slug_and_no_daemon_is_unreachable() {
    let (_bus, _daemon, _seen, accounts) = bus_rig("needs_grant").await;
    let refused = accounts
        .prepare(
            &stt(),
            DataClass::Voice,
            Tier::Fast,
            &OpenOptions::default(),
        )
        .await;
    assert_eq!(
        refused,
        Err(ClientError::Transport(TransportError::Denied(
            "inferd refused: needs_grant".to_owned()
        )))
    );

    let bus = PrivateBus::start();
    let alone = Accounts::over(DbusTransport::over(bus.connect().await));
    assert_eq!(
        alone
            .prepare(
                &stt(),
                DataClass::Voice,
                Tier::Fast,
                &OpenOptions::default()
            )
            .await,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Voice").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

#[derive(Debug, Default)]
struct Warmer {
    asked: Mutex<Vec<(AppId, DataClass, Tier)>>,
}

impl SessionHost for Warmer {
    type Session = FakeInferSession;

    async fn open(
        &self,
        _: &AppId,
        _: &Need,
        _: DataClass,
        _: Tier,
        _: &OpenOptions,
    ) -> Result<FakeInferSession, TransportError> {
        Err(TransportError::Unreachable)
    }

    async fn prepare(
        &self,
        app: &AppId,
        _: &Need,
        class: DataClass,
        tier: Tier,
        _: &OpenOptions,
    ) -> Result<Readiness, TransportError> {
        self.asked
            .lock()
            .expect("lock")
            .push((app.clone(), class, tier));
        Ok(Readiness::Loading)
    }
}

#[tokio::test]
async fn in_process_prepare_asks_the_host_for_the_app_it_names_and_no_broker_is_unreachable() {
    let service: Arc<FakeService> = Arc::new(fake_service(ScriptedSheets::answering([])).await);
    let warmer = Arc::new(Warmer::default());
    let accounts = Accounts::over(
        InProcess::new(Arc::clone(&service), app()).with_broker(Arc::clone(&warmer)),
    );
    assert_eq!(
        accounts
            .prepare(
                &stt(),
                DataClass::Voice,
                Tier::Balanced,
                &OpenOptions::default()
            )
            .await,
        Ok(Readiness::Loading)
    );
    assert_eq!(
        *warmer.asked.lock().expect("lock"),
        vec![(app(), DataClass::Voice, Tier::Balanced)]
    );

    let none = Accounts::over(InProcess::new(service, app()).with_broker(NoBroker));
    assert_eq!(
        none.prepare(
            &stt(),
            DataClass::Voice,
            Tier::Balanced,
            &OpenOptions::default()
        )
        .await,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}
