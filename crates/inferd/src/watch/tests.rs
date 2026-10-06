use super::*;
use crate::probe::testing::{chat, found};
use crate::probed::Standing;
use crate::report::{ReportFault, Reports};
use porter_core::capability::LlmFeature;
use porter_core::{AccountId, Claim};
use porter_fake_servers::{FakeModels, ModelDef, Wire, net::Bind};
use porter_http::HyperHttp;
use std::sync::Mutex;

fn up(runtime: Runtime, port: u16) -> Found {
    found(
        runtime,
        port,
        vec![chat("m", "m", 2048, &[LlmFeature::Chat])],
    )
}

fn installed(standing: Standing, found: Option<Found>) -> Installed {
    Installed {
        standing,
        found,
        told: Told::Yes,
    }
}

#[test]
fn a_look_changes_what_differs_from_what_is_installed() {
    let ollama = up(Runtime::Ollama, 11_434);
    let moved = up(Runtime::Ollama, 11_435);
    let mut other_model = ollama.clone();
    other_model.models = vec![chat("n", "n", 2048, &[LlmFeature::Chat])];
    let table = vec![
        (
            "first sight",
            BTreeMap::new(),
            vec![ollama.clone()],
            vec![Change::Up(ollama.clone())],
        ),
        (
            "nothing new",
            BTreeMap::from([(
                Runtime::Ollama,
                installed(Standing::Online, Some(ollama.clone())),
            )]),
            vec![ollama.clone()],
            vec![],
        ),
        (
            "other models",
            BTreeMap::from([(
                Runtime::Ollama,
                installed(Standing::Online, Some(ollama.clone())),
            )]),
            vec![other_model.clone()],
            vec![Change::Up(other_model)],
        ),
        (
            "moved port",
            BTreeMap::from([(
                Runtime::Ollama,
                installed(Standing::Online, Some(ollama.clone())),
            )]),
            vec![moved.clone()],
            vec![Change::Up(moved)],
        ),
        (
            "stopped",
            BTreeMap::from([(
                Runtime::Ollama,
                installed(Standing::Online, Some(ollama.clone())),
            )]),
            vec![],
            vec![Change::Down(Runtime::Ollama)],
        ),
        (
            "still stopped",
            BTreeMap::from([(
                Runtime::Ollama,
                installed(Standing::Offline, Some(ollama.clone())),
            )]),
            vec![],
            vec![],
        ),
        (
            "back",
            BTreeMap::from([(
                Runtime::Ollama,
                installed(Standing::Offline, Some(ollama.clone())),
            )]),
            vec![ollama.clone()],
            vec![Change::Up(ollama.clone())],
        ),
        ("never seen, never down", BTreeMap::new(), vec![], vec![]),
    ];
    for (name, have, now, want) in table {
        assert_eq!(plan(&have, &now), want, "{name}");
    }
}

#[test]
fn the_wait_doubles_while_nothing_changes_and_starts_over_when_something_does() {
    let (base, longest) = (Duration::from_secs(10), Duration::from_secs(60));
    let s = Duration::from_secs;
    let table = [
        (s(10), false, s(20)),
        (s(20), false, s(40)),
        (s(40), false, s(60)),
        (s(60), false, s(60)),
        (s(60), true, s(10)),
        (s(0), false, s(10)),
    ];
    for (current, changed, want) in table {
        assert_eq!(
            next_wait(current, changed, base, longest),
            want,
            "{current:?} {changed}"
        );
    }
}

/// What accountd was told, in order; it answers as the test says.
#[derive(Debug, Default)]
struct Recorder {
    told: Mutex<Vec<(Runtime, Standing, usize)>>,
    answer: Mutex<Option<ReportFault>>,
}

impl Recorder {
    fn told(&self) -> Vec<(Runtime, Standing, usize)> {
        self.told.lock().expect("lock").clone()
    }

    fn fails_with(&self, fault: Option<ReportFault>) {
        *self.answer.lock().expect("lock") = fault;
    }
}

impl Reports for Arc<Recorder> {
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Standing,
    ) -> crate::cloud::accountd::Boxed<'a, Result<AccountId, ReportFault>> {
        Box::pin(async move {
            self.told
                .lock()
                .expect("lock")
                .push((runtime, standing, claims.len()));
            match *self.answer.lock().expect("lock") {
                Some(fault) => Err(fault),
                None => AccountId::parse(runtime.provider()).map_err(|_| ReportFault::Rejected),
            }
        })
    }
}

fn http() -> HyperHttp {
    HyperHttp::new().with_limits(porter_http::Limits {
        timeout: Duration::from_secs(5),
        max_body: 1 << 20,
    })
}

fn only_ollama(port: u16) -> ProbeConfig {
    ProbeConfig {
        ollama: vec![port],
        ..ProbeConfig::off()
    }
}

async fn fake_on(bind: &Bind) -> porter_fake_servers::Running<porter_fake_servers::ModelsHandle> {
    let fake = FakeModels::bind_on(
        bind,
        Wire::Ollama,
        vec![ModelDef::chat("llama3.2:3b", 8192)],
        None,
    )
    .await
    .expect("bind");
    let handle = fake.handle();
    porter_fake_servers::Running::spawn(fake, handle)
}

/// Stops a fake and gives its listener the moment it needs to close, so the next look is refused
/// rather than accepted by a socket whose task is already gone.
async fn stop<T>(fake: T) {
    drop(fake);
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn port_of(fake: &porter_fake_servers::ModelsHandle) -> u16 {
    fake.base_url()
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .expect("number")
}

fn watch(config: ProbeConfig, engines: &Engines, recorder: &Arc<Recorder>) -> Watch<HyperHttp> {
    Watch::new(
        http(),
        config,
        engines.clone(),
        Arc::new(Arc::clone(recorder)),
        PathBuf::from("/nonexistent"),
    )
}

#[tokio::test]
async fn a_runtime_that_comes_up_is_installed_reported_ok_and_one_that_stops_goes_offline_and_back()
{
    let fake = fake_on(&Bind::Loopback).await;
    let port = port_of(&fake);
    let (engines, recorder) = (Engines::default(), Arc::new(Recorder::default()));
    let mut watch = watch(only_ollama(port), &engines, &recorder);

    assert!(watch.look().await);
    assert_eq!(recorder.told(), [(Runtime::Ollama, Standing::Online, 1)]);
    let listed = engines.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].card.account.as_str(), "ollama");
    assert_eq!(listed[0].readiness, porter_infer::Readiness::Ready);

    // Nothing changed: nothing is reported again.
    assert!(!watch.look().await);
    assert_eq!(recorder.told().len(), 1);

    // The runtime stops: its account goes offline, its model stays, unavailable.
    stop(fake).await;
    assert!(watch.look().await);
    assert_eq!(recorder.told()[1], (Runtime::Ollama, Standing::Offline, 0));
    assert_eq!(
        engines.listed()[0].readiness,
        porter_infer::Readiness::Unavailable
    );
    assert!(
        !watch.look().await,
        "a runtime that stays down is not reported again"
    );
    assert_eq!(recorder.told().len(), 2);

    // And back, where it was.
    let _again = fake_on(&Bind::Port(port)).await;
    assert!(watch.look().await);
    assert_eq!(recorder.told()[2], (Runtime::Ollama, Standing::Online, 1));
    assert_eq!(
        engines.listed()[0].readiness,
        porter_infer::Readiness::Ready
    );
}

#[tokio::test]
async fn a_report_accountd_did_not_get_is_asked_again_and_a_rejected_one_is_not() {
    let fake = fake_on(&Bind::Loopback).await;
    let (engines, recorder) = (Engines::default(), Arc::new(Recorder::default()));
    let mut watch = watch(only_ollama(port_of(&fake)), &engines, &recorder);

    recorder.fails_with(Some(ReportFault::Unreachable));
    watch.look().await;
    // The models are served whether or not accountd heard.
    assert_eq!(engines.listed().len(), 1);
    watch.look().await;
    assert_eq!(
        recorder.told().len(),
        2,
        "unreachable: asked again at the next look"
    );

    recorder.fails_with(None);
    watch.look().await;
    watch.look().await;
    assert_eq!(
        recorder.told().len(),
        3,
        "told once it answers, and then not again"
    );

    // A rejection is final for that content.
    let (engines, recorder) = (Engines::default(), Arc::new(Recorder::default()));
    let mut watch = self::watch(only_ollama(port_of(&fake)), &engines, &recorder);
    recorder.fails_with(Some(ReportFault::Rejected));
    watch.look().await;
    watch.look().await;
    assert_eq!(recorder.told().len(), 1);
}

#[tokio::test]
async fn a_runtime_that_was_never_there_is_never_reported() {
    let closed = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let (engines, recorder) = (Engines::default(), Arc::new(Recorder::default()));
    let mut watch = watch(only_ollama(closed), &engines, &recorder);
    assert!(!watch.look().await);
    assert_eq!(recorder.told(), []);
    assert!(engines.listed().is_empty());
}

#[tokio::test]
async fn rescan_looks_now_and_answers_when_the_look_is_done() {
    let (engines, recorder) = (Engines::default(), Arc::new(Recorder::default()));
    // A wait too long to matter: only the start and `now` look.
    let fake = fake_on(&Bind::Loopback).await;
    let config = ProbeConfig {
        every_s: 3600,
        longest_s: 3600,
        ..only_ollama(port_of(&fake))
    };
    let probing = watch(config, &engines, &recorder).spawn();
    probing.now().await;
    assert_eq!(
        engines.listed().len(),
        1,
        "the look that Rescan asked for is finished"
    );
    stop(fake).await;
    probing.now().await;
    assert_eq!(
        engines.listed()[0].readiness,
        porter_infer::Readiness::Unavailable
    );
    assert_eq!(
        recorder
            .told()
            .iter()
            .map(|(_, standing, _)| *standing)
            .collect::<Vec<_>>(),
        [Standing::Online, Standing::Offline]
    );
}
