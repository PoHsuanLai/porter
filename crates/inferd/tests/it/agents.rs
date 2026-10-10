//! inferd as an external coding agent's model endpoint over an API-key account, end to end on a
//! private bus: the launcher (the world's client connection, role `AgentLauncher`) opens an endpoint
//! over `org.quire.Inference1.Agents`, and a plain HTTP client plays the agent against the loopback
//! listener. The provider is the fake from `hosting` (TLS on loopback, the scratch CA), accountd is
//! the fake one. The routes to a model on this computer are in `agents_local.rs`.

use crate::hosting;

use hosting::accountd::{FakeAccount, Standing};
use hosting::agent::*;
use hosting::engine::Chat;
use hosting::rig::{AgentsPlan, World};
use inferd::peers::{Caller, Role};
use inferd::settings::AgentEndpoint;
use inferd::settings::SpendLine;
use porter_core::MicroUsd;
use porter_dbus::{AgentsProxy, Details};
use porter_infer::{Period, Policy, SpendScope};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// ---- the wire: the real key to the provider, the token to the agent ------------------------

#[tokio::test(flavor = "multi_thread")]
async fn an_anthropic_chat_with_a_tool_call_and_result_streams_through_with_the_real_key() {
    let call = Chat::Call {
        name: "read_file",
        arguments: r#"{"path":"lib.rs"}"#.into(),
    };
    let world = World::start(plan(vec![anthropic_account()], call, say("unused"))).await;
    let mut tap = Tap::start(&world.bus).await;
    let route = account_route("anthropic", &["claude-opus-5-5"]);
    let endpoint = open(&world, &route, "anthropic_messages").await;
    assert_eq!(endpoint.protocol, "anthropic_messages");
    assert!(
        !endpoint.base_url.ends_with("/v1"),
        "Anthropic's SDK appends /v1/messages: {}",
        endpoint.base_url
    );
    assert_ne!(endpoint.token, KEY_ANTHROPIC);

    let sent = anthropic_body("claude-opus-5-5", true);
    let reply = messages(&endpoint, &sent).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.header("content-type"), Some("text/event-stream"));
    assert!(
        reply.body.contains("event: message_start"),
        "{}",
        reply.body
    );
    assert!(
        reply.body.contains("\"type\":\"tool_use\""),
        "{}",
        reply.body
    );
    assert!(
        reply.body.contains("\"name\":\"read_file\""),
        "{}",
        reply.body
    );
    assert!(reply.body.contains("input_json_delta"), "{}", reply.body);
    assert!(
        reply.body.contains("\"stop_reason\":\"tool_use\""),
        "{}",
        reply.body
    );

    // The provider saw the real key in its own header, the agent's request unchanged (the tool
    // result and the cache hint ride through), and nothing of the token.
    let seen = provider_requests(&world);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/messages");
    assert_eq!(seen[0].header("x-api-key"), Some(KEY_ANTHROPIC));
    assert_eq!(seen[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(seen[0].header("authorization"), None);
    assert_eq!(seen[0].body, sent);
    assert!(
        seen[0]
            .headers
            .iter()
            .all(|(_, v)| !v.contains(&endpoint.token))
    );
    // The agent saw only the token.
    assert!(!contains(reply.body.as_bytes(), KEY_ANTHROPIC));
    assert!(
        reply
            .headers
            .iter()
            .all(|(_, v)| !v.contains(KEY_ANTHROPIC))
    );

    // Five tokens in and two out at 4 and 20 dollars a million: 60 micro-dollars, counted
    // against the program and the account, and audited once without content.
    let counted = spent(&world, "anthropic");
    assert_eq!(
        (counted.tokens_in, counted.tokens_out, counted.micro_usd),
        (5, 2, 60)
    );
    let cloud = world.served.cloud().expect("cloud");
    let on_account = cloud.ledger().spent(
        &SpendScope::Account(porter_core::AccountId::parse("anthropic").expect("id")),
        Period::Daily,
        cloud.now(),
    );
    assert_eq!(on_account.micro_usd, 60);
    let audited = world.audit.entries();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].app, agent_app());
    assert_eq!(audited[0].account.as_str(), "anthropic");
    assert_eq!(audited[0].model.as_str(), "claude-opus-5.5");
    assert_eq!(audited[0].cost, Some(MicroUsd(60)));
    assert_eq!(audited[0].usage.input.0, 5);
    assert_eq!(audited[0].usage.output.0, 2);
    let line = serde_json::to_string(&audited[0]).expect("json");
    assert!(
        !line.contains("main.rs") && !line.contains("fn main"),
        "{line}"
    );

    // accountd was asked for the program's own grant, once per request, and for the key once.
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert!(
        calls
            .verdicts
            .iter()
            .all(|(app, class)| app == APP && class == "files")
    );
    assert_eq!(calls.resolved, vec!["grant-anthropic".to_owned()]);

    // No bus message holds the real key; the token is on the bus in the launcher's reply once.
    let bus = tap.drain().await;
    assert!(bus.iter().all(|m| !contains(m, KEY_ANTHROPIC)));
    assert!(
        bus.iter().any(|m| contains(m, APP)),
        "positive control: the scan reads names"
    );
    assert_eq!(
        bus.iter().filter(|m| contains(m, &endpoint.token)).count(),
        1,
        "the token is on the bus once: in the reply to the launcher's open"
    );
    // Nor does an audit line or the spend ledger's file: they hold names, counts and money.
    let lines = serde_json::to_string(&world.audit.entries()).expect("json");
    assert!(
        !lines.contains(KEY_ANTHROPIC) && !lines.contains(&endpoint.token),
        "{lines}"
    );
    assert!(files_holding(&world.scratch, KEY_ANTHROPIC).is_empty());
    assert!(files_holding(&world.scratch, &endpoint.token).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_openai_chat_with_a_tool_call_and_result_streams_through_with_the_real_key() {
    let call = Chat::Call {
        name: "read_file",
        arguments: r#"{"path":"lib.rs"}"#.into(),
    };
    let world = World::start(plan(vec![openai_account()], call, say("unused"))).await;
    let route = account_route("openai", &["gpt-6-luna"]);
    let endpoint = open(&world, &route, "openai_compatible").await;
    assert!(endpoint.base_url.ends_with("/v1"), "{}", endpoint.base_url);

    let sent = openai_body("gpt-6-luna", true);
    let reply = completions(&endpoint, &sent).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(reply.body.contains("\"tool_calls\""), "{}", reply.body);
    assert!(reply.body.contains("read_file"), "{}", reply.body);
    assert!(
        reply.body.trim_end().ends_with("data: [DONE]"),
        "{}",
        reply.body
    );

    let seen = provider_requests(&world);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("Bearer {KEY_OPENAI}").as_str())
    );
    // As sent, but asking for the usage chunk the meter reads.
    let mut want = sent.clone();
    want["stream_options"] = json!({ "include_usage": true });
    assert_eq!(seen[0].body, want);
    assert!(!contains(reply.body.as_bytes(), KEY_OPENAI));

    // Five in at 0.1 and two out at 0.5 dollars a million: one and a half, rounded up to two.
    let counted = spent(&world, "openai");
    assert_eq!(
        (counted.tokens_in, counted.tokens_out, counted.micro_usd),
        (5, 2, 2)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_that_do_not_stream_are_answered_whole_and_metered() {
    let world = World::start(plan(
        vec![anthropic_account(), openai_account()],
        say("whole answer"),
        say("unused"),
    ))
    .await;
    let a = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let reply = messages(&a, &anthropic_body("claude-opus-5-5", false)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert_eq!(reply.json()["content"][0]["text"], "whole answer");
    let o = open(
        &world,
        &account_route("openai", &["gpt-6-luna"]),
        "openai_compatible",
    )
    .await;
    let reply = completions(&o, &openai_body("gpt-6-luna", false)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        reply.json()["choices"][0]["message"]["content"],
        "whole answer"
    );
    assert_eq!(world.audit.entries().len(), 2);
    assert_eq!(spent(&world, "x").micro_usd, 62, "60 and 2");
}

// ---- sessions: tokens, closing, the launcher going away ------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_or_missing_token_is_401_in_the_protocols_own_error_shape() {
    let world = World::start(plan(vec![anthropic_account()], say("x"), say("y"))).await;
    let endpoint = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let body = anthropic_body("claude-opus-5-5", false).to_string();
    let wrong = format!("{}x", endpoint.token);
    let cases: [(&str, &[(&str, &str)]); 4] = [
        ("/v1/messages", &[("x-api-key", &wrong)]),
        ("/v1/messages", &[("x-api-key", KEY_ANTHROPIC)]),
        ("/v1/messages", &[]),
        ("/v1/chat/completions", &[("authorization", "Bearer nope")]),
    ];
    for (path, headers) in cases {
        let reply = send(endpoint.port, "POST", path, headers, &body).await;
        assert_eq!(reply.status, 401, "{path} {headers:?}");
        let error = reply.json();
        if path.ends_with("messages") {
            assert_eq!(error["error"]["type"], "authentication_error");
            assert_eq!(error["type"], "error");
        } else {
            assert_eq!(error["error"]["code"], "invalid_api_key");
        }
    }
    assert!(
        provider_requests(&world).is_empty(),
        "nothing reached the provider"
    );
    assert!(world.audit.entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_session_refuses_and_cancels_what_is_in_flight() {
    let world = World::start(plan(vec![anthropic_account()], Chat::Hang, say("y"))).await;
    let endpoint = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let port = endpoint.port;
    let token = endpoint.token.clone();
    let body = anthropic_body("claude-opus-5-5", true).to_string();
    // The provider holds this request; the agent waits for an answer.
    let waiting = tokio::spawn(async move {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let request = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: x\r\nx-api-key: {token}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.map(|_| raw.len())
    });
    crate::support::eventually("the request reaches the provider", || {
        !provider_requests(&world).is_empty()
    })
    .await;
    assert_eq!(
        provider_requests(&world).len(),
        1,
        "the request is in flight"
    );
    AgentsProxy::new(&world.client)
        .await
        .expect("proxy")
        .close_endpoint(&endpoint.session)
        .await
        .expect("closed");
    let ended = tokio::time::timeout(porter_fake::GENEROUS, waiting)
        .await
        .expect("the in-flight request ends with the session")
        .expect("task");
    assert!(ended.map_or(true, |bytes| bytes == 0), "no answer was sent");
    assert!(refused_to_connect(port).await, "the listener is closed");
    let again = AgentsProxy::new(&world.client)
        .await
        .expect("proxy")
        .close_endpoint(&endpoint.session)
        .await
        .expect_err("no such session");
    assert_eq!(
        error_name(&again),
        "org.quire.Inference1.Error.NoSuchSession"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_launchers_connection_going_away_ends_its_sessions() {
    let world = World::start(plan(vec![anthropic_account()], say("x"), say("y"))).await;
    // A second launcher connection, so the world's own stays up.
    let launcher = world.bus.connect().await;
    world.peers.introduce(
        launcher.unique_name().expect("name").as_str(),
        Caller {
            app: agent_app(),
            role: Role::AgentLauncher,
        },
    );
    let arg = AgentsProxy::new(&launcher)
        .await
        .expect("proxy")
        .open_endpoint(
            PROGRAM,
            &account_route("anthropic", &["claude-opus-5-5"]),
            "files",
            &["anthropic_messages"],
            &Details::new(),
        )
        .await
        .expect("endpoint");
    let endpoint = Endpoint::of(arg);
    let reply = messages(&endpoint, &anthropic_body("claude-opus-5-5", false)).await;
    assert_eq!(reply.status, 200);
    drop(launcher);
    assert!(
        refused_to_connect(endpoint.port).await,
        "the session ended with its launcher"
    );
    // Another launcher's session is not touched by the first one leaving.
    let mine = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    assert_eq!(
        messages(&mine, &anthropic_body("claude-opus-5-5", false))
            .await
            .status,
        200
    );
}

// ---- who may ask, and when -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn only_the_agent_launcher_may_open_an_endpoint() {
    let world = World::start(plan(vec![anthropic_account()], say("x"), say("y"))).await;
    let route = account_route("anthropic", &["claude-opus-5-5"]);
    for role in [Role::App, Role::Cua, Role::Settings] {
        let other = world.bus.connect().await;
        world.peers.introduce(
            other.unique_name().expect("name").as_str(),
            Caller {
                app: agent_app(),
                role,
            },
        );
        let refused = AgentsProxy::new(&other)
            .await
            .expect("proxy")
            .open_endpoint(
                PROGRAM,
                &route,
                "files",
                &["anthropic_messages"],
                &Details::new(),
            )
            .await
            .expect_err("refused");
        assert_eq!(
            error_name(&refused),
            "org.freedesktop.DBus.Error.AccessDenied",
            "{role:?}"
        );
    }
    // A stranger the caller table does not name.
    let stranger = world.bus.connect().await;
    let refused = AgentsProxy::new(&stranger)
        .await
        .expect("proxy")
        .open_endpoint(
            PROGRAM,
            &route,
            "files",
            &["anthropic_messages"],
            &Details::new(),
        )
        .await
        .expect_err("refused");
    assert_eq!(
        error_name(&refused),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    // And the launcher has nothing else of inferd's.
    let refused = world
        .accounts
        .session(
            &porter_core::Need::Llm(porter_core::need::LlmNeed::new(
                [porter_core::capability::LlmFeature::Chat].into(),
                porter_core::Tokens(10),
            )),
            porter_core::DataClass::Prompt,
            porter_core::Tier::Balanced,
        )
        .await;
    assert!(refused.is_err(), "the launcher cannot Open a session");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setting_off_refuses_and_the_default_is_off() {
    assert_eq!(AgentEndpoint::default(), AgentEndpoint::Off);
    assert_eq!(
        inferd::settings::Settings::default().agent_endpoint,
        AgentEndpoint::Off
    );
    let mut off = plan(vec![anthropic_account()], say("x"), say("y"));
    off.agents = Some(AgentsPlan {
        endpoint: AgentEndpoint::Off,
    });
    let world = World::start(off).await;
    let refused = open_with(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "files",
        &["anthropic_messages"],
    )
    .await
    .expect_err("off");
    assert_eq!(
        error_name(&refused),
        "org.quire.Inference1.Error.EndpointOff"
    );
    // Turned on while the daemon runs (the next open reads the settings in force).
    let mut settings = (*world.served.settings()).clone();
    settings.agent_endpoint = AgentEndpoint::On;
    world.served.apply(settings);
    assert!(
        open_with(
            &world,
            &account_route("anthropic", &["claude-opus-5-5"]),
            "files",
            &["anthropic_messages"],
        )
        .await
        .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn outside_the_grant_the_bus_gets_the_cause_and_the_agent_gets_403() {
    let ungranted = FakeAccount {
        id: "anthropic",
        standing: Standing::Ask,
    };
    let world = World::start(plan(vec![ungranted], say("x"), say("y"))).await;
    let route = account_route("anthropic", &["claude-opus-5-5"]);
    let refused = open_with(&world, &route, "files", &["anthropic_messages"])
        .await
        .expect_err("not granted");
    assert_eq!(
        error_name(&refused),
        "org.quire.Inference1.Error.NotGranted"
    );

    // A grant that is withdrawn while the session is open: the next request is refused.
    let world = World::start(plan(vec![anthropic_account()], say("x"), say("y"))).await;
    let endpoint = open(&world, &route, "anthropic_messages").await;
    let body = anthropic_body("claude-opus-5-5", false);
    assert_eq!(messages(&endpoint, &body).await.status, 200);
    world
        .accountd
        .as_ref()
        .expect("accountd")
        .set_standing("anthropic", Standing::Denied);
    let reply = messages(&endpoint, &body).await;
    assert_eq!(reply.status, 403);
    assert_eq!(reply.json()["error"]["type"], "permission_error");
    let resolved = world.accountd.as_ref().expect("accountd").calls().resolved;
    assert_eq!(
        resolved.len(),
        1,
        "no key was fetched for the refused request"
    );
    assert_eq!(
        provider_requests(&world).len(),
        1,
        "and none reached the provider"
    );
    // The same through the OpenAI shape.
    let reply = completions(&endpoint, &openai_body("claude-opus-5-5", false)).await;
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert_eq!(reply.json()["error"]["code"], "permission_denied");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_policy_keeps_a_class_off_the_cloud_at_the_open_and_at_each_request() {
    let mut strict = plan(vec![anthropic_account()], say("x"), say("y"));
    strict.policy = Policy::proposed();
    let world = World::start(strict).await;
    let refused = open_with(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "files",
        &["anthropic_messages"],
    )
    .await
    .expect_err("local-only is on");
    assert_eq!(
        error_name(&refused),
        "org.quire.Inference1.Error.FloorRefused"
    );

    let world = World::start(plan(vec![anthropic_account()], say("x"), say("y"))).await;
    let endpoint = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let mut settings = (*world.served.settings()).clone();
    settings.policy = Policy::proposed();
    world.served.apply(settings);
    let reply = messages(&endpoint, &anthropic_body("claude-opus-5-5", false)).await;
    assert_eq!(reply.status, 403);
    assert!(provider_requests(&world).is_empty());
}

// ---- spend ---------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn over_the_spend_cap_the_agent_is_told_to_stop_and_nothing_is_sent_or_counted() {
    let mut capped = plan(
        vec![anthropic_account(), openai_account()],
        say("x"),
        say("y"),
    );
    // Five cents a day for the program.
    capped.spend = SpendLine {
        limits: inferd::settings::SpendLimits {
            app: inferd::settings::ScopeLimits {
                daily: inferd::settings::cents_to_limit(5),
                monthly: None,
            },
            account: inferd::settings::ScopeLimits::default(),
        },
        ..SpendLine::default()
    };
    let world = World::start(capped).await;
    let endpoint = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let body = anthropic_body("claude-opus-5-5", false);
    // Under the cap: served and counted.
    assert_eq!(messages(&endpoint, &body).await.status, 200);
    let cloud = world.served.cloud().expect("cloud");
    // The program has spent 40,000 micro-dollars of its 50,000 elsewhere today.
    cloud.ledger().record(
        &agent_app(),
        &porter_core::AccountId::parse("anthropic").expect("id"),
        porter_infer::TokenUsage {
            input: porter_core::Tokens(0),
            output: porter_core::Tokens(0),
            cached: porter_core::Tokens(0),
        },
        MicroUsd(39_940),
        cloud.now(),
    );
    let before = spent(&world, "anthropic").micro_usd;
    assert_eq!(before, 40_000);
    let reply = messages(&endpoint, &body).await;
    assert_eq!(reply.status, 429, "{}", reply.body);
    assert_eq!(reply.header("x-should-retry"), Some("false"));
    assert_eq!(reply.json()["error"]["type"], "rate_limit_error");
    assert_eq!(
        provider_requests(&world).len(),
        1,
        "the provider saw only the first"
    );
    assert_eq!(
        spent(&world, "anthropic").micro_usd,
        before,
        "a refused request counts nothing"
    );
    // The cap is the program's, whichever account pays: the same refusal in OpenAI's shape.
    cloud.ledger().record(
        &agent_app(),
        &porter_core::AccountId::parse("openai").expect("id"),
        porter_infer::TokenUsage {
            input: porter_core::Tokens(0),
            output: porter_core::Tokens(0),
            cached: porter_core::Tokens(0),
        },
        MicroUsd(9_900),
        cloud.now(),
    );
    let openai = open(
        &world,
        &account_route("openai", &["gpt-6-luna"]),
        "openai_compatible",
    )
    .await;
    let reply = completions(&openai, &openai_body("gpt-6-luna", false)).await;
    assert_eq!(reply.status, 429, "{}", reply.body);
    assert_eq!(reply.json()["error"]["code"], "spend_cap_reached");
    assert_eq!(provider_requests(&world).len(), 1);
}

// ---- models --------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn an_id_the_route_does_not_serve_is_an_error_and_never_a_substitute() {
    let world = World::start(plan(
        vec![anthropic_account(), openai_account()],
        say("x"),
        say("y"),
    ))
    .await;
    let endpoint = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let reply = messages(&endpoint, &anthropic_body("claude-sonnet-4", false)).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json()["error"]["type"], "not_found_error");
    assert!(provider_requests(&world).is_empty());
    let listed = send(
        endpoint.port,
        "GET",
        "/v1/models",
        &[
            ("x-api-key", &endpoint.token),
            ("anthropic-version", "2023-06-01"),
        ],
        "",
    )
    .await;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json()["data"][0]["id"], "claude-opus-5-5");
    assert_eq!(listed.json()["data"][0]["type"], "model");
    let one = send(
        endpoint.port,
        "GET",
        "/v1/models/claude-sonnet-4",
        &[("x-api-key", &endpoint.token)],
        "",
    )
    .await;
    assert_eq!(one.status, 404);

    // An any-model account route forwards what the agent names, if the provider prices it.
    let any = (
        "account".to_owned(),
        "openai".to_owned(),
        "any".to_owned(),
        Vec::new(),
    );
    let endpoint = open(&world, &any, "openai_compatible").await;
    let reply = completions(&endpoint, &openai_body("gpt-6-luna", false)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let unpriced = completions(&endpoint, &openai_body("gpt-nonesuch", false)).await;
    assert_eq!(
        unpriced.status, 404,
        "an id nobody prices cannot be metered"
    );
    let listed = send(
        endpoint.port,
        "GET",
        "/v1/models",
        &[("authorization", &format!("Bearer {}", endpoint.token))],
        "",
    )
    .await;
    assert_eq!(listed.json()["object"], "list");
    assert_eq!(listed.json()["data"][0]["id"], "gpt-6-luna");
    // The wrong protocol for the provider is a typed refusal at the open and a 400 per request.
    let refused = open_with(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "files",
        &["openai_compatible"],
    )
    .await
    .expect_err("protocol");
    assert_eq!(
        error_name(&refused),
        "org.quire.Inference1.Error.UnsupportedProtocol"
    );
    let wrong = completions(
        &open(
            &world,
            &account_route("anthropic", &["claude-opus-5-5"]),
            "anthropic_messages",
        )
        .await,
        &openai_body("claude-opus-5-5", false),
    )
    .await;
    assert_eq!(wrong.status, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn count_tokens_is_an_estimate_for_every_route() {
    let world = World::start(plan(vec![anthropic_account()], say("x"), say("y"))).await;
    let endpoint = open(
        &world,
        &account_route("anthropic", &["claude-opus-5-5"]),
        "anthropic_messages",
    )
    .await;
    let body = json!({ "model": "claude-opus-5-5", "messages": [{ "role": "user", "content": "x".repeat(400) }] });
    let reply = send(
        endpoint.port,
        "POST",
        "/v1/messages/count_tokens",
        &[("x-api-key", &endpoint.token)],
        &body.to_string(),
    )
    .await;
    assert_eq!(reply.status, 200);
    let tokens = reply.json()["input_tokens"].as_u64().expect("number");
    assert!((100..140).contains(&tokens), "{tokens}");
    assert!(
        provider_requests(&world).is_empty(),
        "the provider was not asked"
    );
}
