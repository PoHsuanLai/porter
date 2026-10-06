//! `SocketTransport` against a hand-written agent on a Unix socket: accountd calls answered by
//! the real `AccountService`, sessions by the real session server over scripted seams. One
//! connection per call, a hello first on a session.
#![cfg(all(unix, feature = "socket"))]

mod common;

use common::agent::{Agent, Plan, start};
use common::served::{Gate, Route, Scripted};
use inferd::session::RouteDecision;
use porter_client::{
    Accounts, ClientEnv, ClientError, Found, InferSession, LinkChoice, OpenOptions, SocketPath,
    SocketTransport, TransportError,
};
use porter_core::capability::{Access, CuaEnv, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::{CuaNeed, DimsNeed, EmbedNeed, StorageNeed};
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AppId, AppName, Audience, DataClass, Dims, Isolation, Locality, ModelId, Need, Tier,
    Tokens,
};
use porter_fake::{Script, ScriptStep, Scripted as Answer, ScriptedSheets, fake_service};
use porter_infer::{
    AttachIndex, ClientFrame, CuaBegin, CuaStepReply, EmbedReply, EmbedRequest, EmbedRole,
    EmbedVector, ImageSource, InferEvent, InferRefusal, InferReply, InferRequest, LinkHello,
    Readiness, RequestKind, ServedBy, TokenUsage, Traceparent,
};
use std::sync::Arc;

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn served_by() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("qwen").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn ready() -> Route {
    Route(Ok(RouteDecision {
        served: served_by(),
        readiness: Readiness::Ready,
    }))
}

async fn agent(name: &str, answers: Vec<Answer>, route: Route, scripts: Vec<Script>) -> Agent {
    let service = Arc::new(fake_service(ScriptedSheets::answering(answers)).await);
    start(
        name,
        Plan {
            service,
            app: app(),
            route,
            gate: Gate::default(),
            runner: Scripted::new(scripts),
        },
    )
}

fn accounts(agent: &Agent) -> Accounts<SocketTransport> {
    Accounts::over(SocketTransport::at(SocketPath(agent.path.clone())))
}

fn storage(delta: Delta) -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

async fn offer(accounts: &Accounts<SocketTransport>) -> porter_client::ConsentOffer {
    match accounts
        .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::NeedsConsent(offer) => offer,
        other => panic!("expected NeedsConsent, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn find_choose_token_grants_and_revoke_work_over_the_socket() {
    let agent = agent(
        "calls",
        vec![Answer::AllowFirst(GrantScope::Always)],
        ready(),
        vec![],
    )
    .await;
    let photos = accounts(&agent);
    let offer = offer(&photos).await;

    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("chosen");
    assert_eq!(chosen.account.as_str(), "fake-storage");
    let token = photos
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("token");
    assert_eq!(token.value.expose(), "fake:fake-storage:webdav");
    assert_eq!(photos.grants().await.expect("grants").len(), 1);
    photos.revoke(&chosen.grant).await.expect("revoke");
    assert_eq!(photos.grants().await.expect("grants"), vec![]);
    assert_eq!(
        photos.revoke(&chosen.grant).await,
        Err(ClientError::Refused(Refusal::UnknownGrant))
    );
    // `add_account` is `todo!()` in porter-service until W3c; its socket row joins then.
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sheet_waiting_on_the_person_blocks_no_other_call() {
    let agent = agent("sheet", vec![Answer::Hang], ready(), vec![]).await;
    let photos = Arc::new(accounts(&agent));
    let offer = offer(&photos).await;
    let waiting = {
        let photos = Arc::clone(&photos);
        tokio::spawn(async move {
            photos
                .request_grant(&offer, &ParentWindow::Unparented)
                .await
        })
    };
    // Its sheet is open (the call has not answered); another call goes through meanwhile.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    assert_eq!(photos.grants().await.expect("grants"), vec![]);
    waiting.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_listening_is_unreachable_and_connect_skips_the_link() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("socket-nobody");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = SocketPath(dir.join("none.sock"));
    let transport = SocketTransport::at(path.clone());
    let accounts = Accounts::over(transport);
    assert_eq!(
        accounts
            .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
            .await
            .map(|_| ()),
        Err(ClientError::Transport(TransportError::Unreachable))
    );
    let env = ClientEnv {
        links: vec![LinkChoice::Socket(path)],
    };
    assert!(matches!(
        Accounts::connect(&env).await,
        Err(ClientError::Transport(TransportError::Unreachable))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_reaches_an_agent_and_the_link_answers() {
    let agent = agent("connect", vec![], ready(), vec![]).await;
    let env = ClientEnv {
        links: vec![
            LinkChoice::Socket(SocketPath(agent.path.with_file_name("nobody.sock"))),
            LinkChoice::Socket(SocketPath(agent.path.clone())),
        ],
    };
    let connected = Accounts::connect(&env)
        .await
        .expect("the second link is reachable");
    let found = connected
        .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert!(matches!(found, Found::NeedsConsent(_)));
}

fn embeddings() -> Need {
    Need::Embeddings(EmbedNeed {
        dims: DimsNeed::Exactly(Dims(2)),
        modalities: [porter_core::capability::Modality::Text].into(),
    })
}

fn embed_request() -> InferRequest {
    InferRequest::Embed(EmbedRequest {
        inputs: vec!["a note".into()],
        role: EmbedRole::Document,
        dims: DimsNeed::Exactly(Dims(2)),
        class: DataClass::Notes,
        usage: Usage::Background,
    })
}

fn embed_reply() -> InferReply {
    InferReply::Embed(EmbedReply {
        vectors: vec![EmbedVector(vec![0.5, 0.25])],
        usage: TokenUsage {
            input: Tokens(1),
            output: Tokens(0),
            cached: Tokens(0),
        },
        served: served_by(),
    })
}

fn embed_script() -> Script {
    Script {
        kind: RequestKind::Embed,
        steps: vec![ScriptStep::Emit(InferEvent::Finished(embed_reply()))],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_opens_with_a_hello_and_streams_what_the_machine_says() {
    let agent = agent("session", vec![], ready(), vec![embed_script()]).await;
    let photos = accounts(&agent);
    let parent = Traceparent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        .expect("traceparent");
    let options = OpenOptions {
        traceparent: Some(parent),
        ..OpenOptions::default()
    };
    let mut session = photos
        .session_with(&embeddings(), DataClass::Notes, Tier::Fast, &options)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(embed_request()))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served_by())));
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(embed_reply()))
    );

    let hellos = agent.hellos.lock().expect("lock").clone();
    let LinkHello::Open(open) = &hellos[0];
    assert_eq!(
        (&open.need, open.class, open.tier, &open.options),
        (&embeddings(), DataClass::Notes, Tier::Fast, &options)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_one_call_form_and_a_refused_route_work_over_the_socket() {
    let good = agent("infer", vec![], ready(), vec![embed_script()]).await;
    assert_eq!(
        accounts(&good)
            .infer(&embeddings(), DataClass::Notes, Tier::Fast, embed_request())
            .await,
        Ok(embed_reply())
    );
    let refused = agent(
        "refused",
        vec![],
        Route(Err(InferRefusal::NeedsGrant)),
        vec![],
    )
    .await;
    assert_eq!(
        accounts(&refused)
            .infer(&embeddings(), DataClass::Notes, Tier::Fast, embed_request())
            .await,
        Err(ClientError::InferRefused(InferRefusal::NeedsGrant))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_rides_the_socket_as_a_memfd() {
    let ack = InferReply::CuaStep(CuaStepReply {
        thought: None,
        actions: vec![],
        dropped: vec![],
        safety: vec![],
    });
    let done = InferReply::CuaStep(CuaStepReply {
        thought: Some("done".into()),
        actions: vec![cua_action::CuaAction::Observe],
        dropped: vec![],
        safety: vec![],
    });
    let scripts = vec![
        Script {
            kind: RequestKind::CuaBegin,
            steps: vec![ScriptStep::Emit(InferEvent::Finished(ack.clone()))],
        },
        Script {
            kind: RequestKind::CuaStep,
            steps: vec![ScriptStep::Emit(InferEvent::Finished(done.clone()))],
        },
    ];
    let agent = agent("memfd", vec![], ready(), scripts).await;
    let need = Need::ComputerUse(CuaNeed {
        environments: [CuaEnv::Desktop].into(),
    });
    let mut session = accounts(&agent)
        .session(&need, DataClass::Screen, Tier::Best)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(InferRequest::CuaBegin(CuaBegin {
            goal: "rename".into(),
            hints: vec![],
            env: CuaEnv::Desktop,
        })))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served_by())));
    assert_eq!(session.next().await, Ok(InferEvent::Finished(ack)));

    let frame = rustix::fs::memfd_create("frame", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd");
    let step = ClientFrame::Request(InferRequest::CuaStep(porter_infer::CuaStepRequest {
        step: porter_infer::StepIndex(0),
        window: porter_infer::WindowGeometry {
            logical: cua_action::Size::new(cua_action::Coord(1), cua_action::Coord(1)),
            scale: cua_action::Scale120(120),
        },
        frame: porter_infer::FrameImage {
            source: ImageSource::Attached(AttachIndex(0)),
            layout: porter_infer::FrameLayout::Encoded(porter_infer::MediaKind::Png),
        },
        cursor: None,
        prev: vec![],
        masked: porter_infer::MaskedRegions(0),
        tree: porter_infer::TreeText::Absent,
        notes: vec![],
    }));
    session
        .send_attached(step, vec![frame])
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Finished(done)));
    let kinds: Vec<(RequestKind, usize)> = agent
        .runner
        .seen
        .lock()
        .expect("lock")
        .iter()
        .map(|(r, n)| (r.kind(), *n))
        .collect();
    assert_eq!(
        kinds,
        vec![(RequestKind::CuaBegin, 0), (RequestKind::CuaStep, 1)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_hangs_up_mid_call_is_closed_not_hung() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("socket-hangup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&path).expect("bind");
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    let photos = Accounts::over(SocketTransport::at(SocketPath(path)));
    let got = photos
        .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
        .await
        .map(|_| ());
    assert_eq!(got, Err(ClientError::Transport(TransportError::Closed)));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_authenticated_relay_arrives_as_a_descriptor_on_the_reply_and_a_refusal_as_one_without()
{
    use porter_client::{AuthenticatedStream, Relayed, Transport};
    use std::io::Read;

    let agent = agent(
        "relay",
        vec![Answer::AllowFirst(GrantScope::Always)],
        ready(),
        vec![],
    )
    .await;
    let photos = accounts(&agent);
    let offer = offer(&photos).await;
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("chosen");
    let (grant, url) = (chosen.grant.clone(), chosen.endpoints[0].url.clone());
    let transport = SocketTransport::at(SocketPath(agent.path.clone()));

    let Relayed::Stream(AuthenticatedStream::Fd(fd)) = transport
        .open_authenticated(&grant, &url)
        .await
        .expect("carried")
    else {
        panic!("expected a relay descriptor");
    };
    let mut stream = std::os::unix::net::UnixStream::from(fd);
    let expected = format!("relay for {url}\r\n");
    let mut line = vec![0u8; expected.len()];
    stream
        .read_exact(&mut line)
        .expect("the relay speaks first");
    assert_eq!(String::from_utf8(line).expect("text"), expected);

    let elsewhere = porter_core::EndpointUrl::parse("https://elsewhere.example").expect("url");
    match transport
        .open_authenticated(&grant, &elsewhere)
        .await
        .expect("carried")
    {
        Relayed::Refused(refusal) => assert_eq!(refusal, Refusal::EndpointNotGranted),
        other => panic!("expected a refusal, got {other:?}"),
    }
}
