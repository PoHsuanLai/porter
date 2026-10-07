//! What the agent endpoint tests share: the accounts and plan of a world with agent endpoints, the
//! launcher's calls over the bus, and a plain HTTP client that plays the agent.

use super::accountd::{FakeAccount, Standing};
use super::engine::{Chat, Script};
use super::entries;
use super::rig::{AgentsPlan, Hosted, Plan, Trust, World};
use inferd::peers::Role;
use inferd::settings::AgentEndpoint;
use porter_core::{AppId, AppName, Isolation};
use porter_dbus::{AgentsProxy, Details, EndpointArg};
use porter_infer::{LocalOnly, Period, Policy, SpendScope};
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const KEY_ANTHROPIC: &str = "sk-ant-api03-REAL-SECRET-KEY-0123456789";
pub const KEY_OPENAI: &str = "sk-proj-OTHER-REAL-SECRET-9876543210";
pub const PROGRAM: &str = "claude-code";
pub const APP: &str = "org.quire.Agent.claude-code";

pub fn agent_app() -> AppId {
    AppId {
        name: AppName::parse(APP).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

pub fn anthropic_account() -> FakeAccount {
    FakeAccount {
        id: "anthropic",
        standing: Standing::Granted {
            to: vec![APP],
            grant: "grant-anthropic",
            key: Some(KEY_ANTHROPIC),
        },
    }
}

pub fn openai_account() -> FakeAccount {
    FakeAccount {
        id: "openai",
        standing: Standing::Granted {
            to: vec![APP],
            grant: "grant-openai",
            key: Some(KEY_OPENAI),
        },
    }
}

pub fn open_policy() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        floors: Vec::new(),
    }
}

pub fn plan(accounts: Vec<FakeAccount>, cloud: Chat, local: Chat) -> Plan {
    Plan {
        catalog: vec![
            ("a-claude-opus-5.5.toml", entries::claude()),
            ("b-gpt-6-luna.toml", entries::luna()),
            ("tiny-chat.toml", entries::chat()),
        ],
        scripts: vec![(
            "tiny-chat",
            Script {
                chat: vec![local],
                dims: 0,
            },
        )],
        policy: open_policy(),
        role: Role::AgentLauncher,
        hosted: Some(Hosted {
            accounts,
            chat: Script {
                chat: vec![cloud],
                dims: 0,
            },
            trust: Trust::ScratchCa,
        }),
        agents: Some(AgentsPlan {
            endpoint: AgentEndpoint::On,
        }),
        ..Plan::default()
    }
}

pub fn say(text: &'static str) -> Chat {
    Chat::Say(vec![text])
}

pub type Route = (String, String, String, Vec<String>);

pub fn account_route(id: &str, models: &[&str]) -> Route {
    (
        "account".into(),
        id.into(),
        "listed".into(),
        models.iter().map(|m| (*m).to_owned()).collect(),
    )
}

pub fn model_route(id: &str, mode: &str, models: &[&str]) -> Route {
    (
        "model".into(),
        id.into(),
        mode.into(),
        models.iter().map(|m| (*m).to_owned()).collect(),
    )
}

pub async fn open_with(
    world: &World,
    route: &Route,
    class: &str,
    protocols: &[&str],
) -> zbus::Result<EndpointArg> {
    AgentsProxy::new(&world.client)
        .await?
        .open_endpoint(PROGRAM, route, class, protocols, &Details::new())
        .await
}

pub async fn open(world: &World, route: &Route, protocol: &str) -> Endpoint {
    let arg = open_with(world, route, "files", &[protocol])
        .await
        .expect("an endpoint");
    Endpoint::of(arg)
}

/// What the launcher is told.
pub struct Endpoint {
    pub session: String,
    pub base_url: String,
    pub port: u16,
    pub token: String,
    pub protocol: String,
}

impl Endpoint {
    pub fn of(arg: EndpointArg) -> Self {
        let (session, scheme, host, port, base_url, token, protocol) = arg;
        assert_eq!((scheme.as_str(), host.as_str()), ("http", "127.0.0.1"));
        assert_eq!(base_url.split("://").next(), Some("http"));
        assert!(
            base_url.contains(&format!("127.0.0.1:{port}")),
            "{base_url}"
        );
        Self {
            session,
            base_url,
            port,
            token,
            protocol,
        }
    }
}

pub fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => format!("{other:?}"),
    }
}

/// One HTTP response.
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(have, _)| have.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("json {e}: {}", self.body))
    }
}

pub fn unchunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(at) = rest.windows(2).position(|w| w == b"\r\n") {
        let size =
            usize::from_str_radix(std::str::from_utf8(&rest[..at]).expect("size").trim(), 16)
                .expect("hex size");
        rest = &rest[at + 2..];
        if size == 0 {
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
    out
}

pub fn parse_reply(raw: Vec<u8>) -> Reply {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a response head");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .expect("a status");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let body = &raw[split + 4..];
    let chunked = headers
        .iter()
        .any(|(n, v)| n == "transfer-encoding" && v == "chunked");
    let body = if chunked {
        unchunk(body)
    } else {
        body.to_vec()
    };
    Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

pub async fn send(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    parse_reply(raw)
}

/// An Anthropic Messages request as the agent sends it.
pub async fn messages(endpoint: &Endpoint, body: &Value) -> Reply {
    send(
        endpoint.port,
        "POST",
        "/v1/messages",
        &[
            ("x-api-key", &endpoint.token),
            ("anthropic-version", "2023-06-01"),
            ("content-type", "application/json"),
        ],
        &body.to_string(),
    )
    .await
}

/// A chat-completions request as the agent sends it.
pub async fn completions(endpoint: &Endpoint, body: &Value) -> Reply {
    send(
        endpoint.port,
        "POST",
        "/v1/chat/completions",
        &[
            ("authorization", &format!("Bearer {}", endpoint.token)),
            ("content-type", "application/json"),
        ],
        &body.to_string(),
    )
    .await
}

pub fn anthropic_body(model: &str, stream: bool) -> Value {
    json!({
        "model": model, "max_tokens": 256, "stream": stream,
        "system": [{ "type": "text", "text": "you are an agent", "cache_control": { "type": "ephemeral" } }],
        "tools": [{ "name": "read_file", "description": "reads",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string" } } } }],
        "messages": [
            { "role": "user", "content": "open main.rs" },
            { "role": "assistant", "content": [
                { "type": "text", "text": "reading it" },
                { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": { "path": "main.rs" } },
            ]},
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}" },
            ]},
        ],
    })
}

pub fn openai_body(model: &str, stream: bool) -> Value {
    json!({
        "model": model, "stream": stream,
        "tools": [{ "type": "function", "function": { "name": "read_file", "description": "reads",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } } } } }],
        "messages": [
            { "role": "user", "content": "open main.rs" },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "call_1", "type": "function",
                  "function": { "name": "read_file", "arguments": "{\"path\":\"main.rs\"}" } }] },
            { "role": "tool", "tool_call_id": "call_1", "content": "fn main() {}" },
        ],
    })
}

pub fn provider_requests(world: &World) -> Vec<super::engine::Seen> {
    world.provider.as_ref().expect("provider").requests()
}

pub fn spent(world: &World, account: &str) -> inferd::cloud::spend::Spent {
    let cloud = world.served.cloud().expect("cloud");
    let _ = account;
    cloud
        .ledger()
        .spent(&SpendScope::App(agent_app()), Period::Daily, cloud.now())
}

pub fn contains(haystack: &[u8], needle: &str) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
}

/// Every message on the bus from the moment the tap is set, as a monitor sees them.
pub struct Tap(zbus::MessageStream);

impl Tap {
    pub async fn start(bus: &super::bus::PrivateBus) -> Self {
        let monitor = bus.connect().await;
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    pub async fn drain(&mut self) -> Vec<Vec<u8>> {
        use zbus::export::futures_core::Stream;
        let mut seen = Vec::new();
        loop {
            let next = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.0).poll_next(cx)),
            )
            .await;
            match next {
                Ok(Some(Ok(message))) => seen.push(message.data().to_vec()),
                _ => return seen,
            }
        }
    }
}

pub fn files_holding(dir: &Path, needle: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found.extend(files_holding(&path, needle));
        } else if kind.is_file() && std::fs::read(&path).is_ok_and(|bytes| contains(&bytes, needle))
        {
            found.push(path);
        }
    }
    found
}

pub async fn refused_to_connect(port: u16) -> bool {
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}
