//! What `porter-rig-client` does: one command as an app, over the session bus, answered as JSON
//! lines. Each command emits one or more objects with a `result` or an `event` key; a failure
//! is `{"result":"error", ...}` and the process exits 1.
//!
//! The commands are the app side of the scenarios: ask for a grant, open an AI session, open
//! an authenticated relay and read what comes through it, settle a sync conflict, watch for
//! them. The app's identity is the daemons' to find (see `fixture`).

use porter_client::{Accounts, ClientError, DbusTransport, Found, InferSession, OpenOptions};
use porter_core::capability::{Access, Delta, Offered, QuotaReport, StorageScope};
use porter_core::consent::Usage;
use porter_core::need::{LlmNeed, MailNeed, NotesNeed, PimNeed, StorageNeed};
use porter_core::wire::{ParentWindow, ProviderHint};
use porter_core::{AccountId, Candidate, DataClass, Need, ProviderId, Tier, Tokens};
use porter_dbus::{SyncProxy, from_vardict};
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRequest, Knob,
    MessagePart, Reasoning, ReplyShape, Role, ToolChoice, ToolParallelism,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::os::fd::OwnedFd;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use zbus::export::futures_core::Stream;

/// What the commands share: who asks and what for.
#[derive(Debug, Clone)]
pub struct Ask {
    /// The data class of the need (`files`, `photos`, `mail`, `public`, ...).
    pub class: DataClass,
    /// `interactive` or `background`.
    pub usage: Usage,
}

/// A word of a closed set as the wire spells it (`files`, `background`, `fast`).
pub fn word<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(text.to_owned()))
        .map_err(|_| format!("`{text}` is not one of the words"))
}

/// The need a command line names: a preset (`mail`, `storage`, `calendar`, `contacts`,
/// `notes`, `llm`), each asking for the least that is useful, or a JSON `Need` (porter-core's
/// serde form, `{"kind":"storage","v":{...}}`).
pub fn parse_need(text: &str) -> Result<Need, String> {
    if text.trim_start().starts_with('{') {
        return serde_json::from_str(text).map_err(|e| format!("not a Need: {e}"));
    }
    let pim = || PimNeed {
        access: Access::Read,
        delta: Delta::None,
    };
    Ok(match text {
        "mail" => Need::Mail(MailNeed {
            access: Access::Read,
            send: Offered::Absent,
            delta: Delta::None,
        }),
        "storage" => Need::Storage(StorageNeed {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            scope: StorageScope::AppFolder,
            quota: QuotaReport::Unreported,
        }),
        "calendar" => Need::Calendar(pim()),
        "contacts" => Need::Contacts(pim()),
        "notes" => Need::Notes(NotesNeed {
            access: Access::Read,
            delta: Delta::None,
        }),
        "llm" => Need::Llm(LlmNeed {
            features: Default::default(),
            context: Tokens(1_000),
        }),
        other => {
            return Err(format!(
                "`{other}` is not a need (mail, storage, calendar, contacts, notes, llm, or JSON)"
            ));
        }
    })
}

fn failure(why: impl std::fmt::Display) -> Value {
    json!({ "result": "error", "error": why.to_string() })
}

fn candidate_json(candidate: &Candidate) -> Value {
    serde_json::to_value(candidate).unwrap_or(Value::Null)
}

/// The commands.
#[derive(Debug, Clone)]
pub enum Command {
    /// Finds an account for the need, asking for a grant if the app has none.
    RequestGrant {
        /// The need.
        need: Need,
        /// Class and usage.
        ask: Ask,
    },
    /// Opens an AI session and, with a prompt, runs one chat turn on it.
    Open {
        /// The need.
        need: Need,
        /// Class and usage.
        ask: Ask,
        /// The tier.
        tier: Tier,
        /// What to say, if anything.
        prompt: Option<String>,
    },
    /// Opens an authenticated relay to one of a granted account's servers and reads what comes.
    OpenAuthenticated {
        /// The need.
        need: Need,
        /// Class and usage.
        ask: Ask,
        /// Which server: a family slug (`imap`, `graph`, `web_dav`, ...); the first when absent.
        family: Option<String>,
        /// The request path for an HTTP server.
        path: String,
    },
    /// Settles a stored sync conflict.
    SyncResolve {
        /// `<account>/<dataset>`.
        dataset: String,
        /// The conflict's number.
        number: i64,
        /// `keep_local` or `keep_remote`.
        how: String,
    },
    /// Prints the conflicts syncd tells.
    WatchConflicts {
        /// Stop after this many.
        count: Option<usize>,
        /// Stop after this long.
        timeout: Option<Duration>,
    },
    /// Opens accountd's add-account sheet (`Manager.AddAccount`) and waits for the Request's
    /// `Response`: the account added, or why not.
    AddAccount {
        /// The provider whose form opens first; none: the provider list.
        provider: Option<ProviderId>,
    },
    /// Opens the sheet that signs an account in again (`Account.Reauthenticate`) and waits for
    /// the `Response`.
    Reauthenticate {
        /// The account.
        account: AccountId,
    },
    /// The datasets syncd shows this app.
    Datasets,
    /// One dataset's status.
    Status {
        /// `<account>/<dataset>`.
        dataset: String,
    },
    /// The app's own grants.
    Grants,
}

/// Runs `command` as the app on `connection`, handing every JSON line to `emit`; whether it
/// succeeded.
pub async fn run(
    connection: &zbus::Connection,
    command: Command,
    emit: &mut impl FnMut(Value),
) -> bool {
    let accounts = Accounts::over(DbusTransport::over(connection.clone()));
    match command {
        Command::RequestGrant { need, ask } => request_grant(&accounts, &need, &ask, emit).await,
        Command::Open {
            need,
            ask,
            tier,
            prompt,
        } => open(&accounts, &need, &ask, tier, prompt, emit).await,
        Command::OpenAuthenticated {
            need,
            ask,
            family,
            path,
        } => open_authenticated(&accounts, &need, &ask, family.as_deref(), &path, emit).await,
        Command::SyncResolve {
            dataset,
            number,
            how,
        } => sync_resolve(connection, &dataset, number, &how, emit).await,
        Command::WatchConflicts { count, timeout } => {
            watch_conflicts(connection, count, timeout, emit).await
        }
        Command::AddAccount { provider } => add_account(&accounts, provider, emit).await,
        Command::Reauthenticate { account } => reauthenticate(&accounts, &account, emit).await,
        Command::Datasets => match SyncProxy::new(connection).await {
            Ok(sync) => match sync.datasets().await {
                Ok(names) => {
                    emit(json!({ "result": "ok", "datasets": names }));
                    true
                }
                Err(why) => {
                    emit(failure(why));
                    false
                }
            },
            Err(why) => {
                emit(failure(why));
                false
            }
        },
        Command::Status { dataset } => status(connection, &dataset, emit).await,
        Command::Grants => match accounts.grants().await {
            Ok(grants) => {
                emit(json!({ "result": "ok", "grants": grants }));
                true
            }
            Err(why) => {
                emit(failure(why));
                false
            }
        },
    }
}

/// What a sheet that did not finish tells: `refused` with accountd's word for it (`Cancelled`
/// when the person closed the sheet, `Unavailable` when no sheet host answered, ...).
fn sheet_failure(why: ClientError, emit: &mut impl FnMut(Value)) -> bool {
    match why {
        ClientError::Refused(refusal) => {
            emit(json!({ "result": "refused", "refusal": format!("{refusal:?}") }));
        }
        other => emit(failure(other)),
    }
    false
}

async fn add_account(
    accounts: &Accounts<DbusTransport>,
    provider: Option<ProviderId>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let hint = provider.map_or(ProviderHint::Any, ProviderHint::Provider);
    match accounts.add_account(hint, &ParentWindow::Unparented).await {
        Ok(account) => {
            emit(json!({ "result": "added", "account": account.as_str() }));
            true
        }
        Err(why) => sheet_failure(why, emit),
    }
}

async fn reauthenticate(
    accounts: &Accounts<DbusTransport>,
    account: &AccountId,
    emit: &mut impl FnMut(Value),
) -> bool {
    match accounts
        .reauthenticate(account, &ParentWindow::Unparented)
        .await
    {
        Ok(()) => {
            emit(json!({ "result": "reauthenticated", "account": account.as_str() }));
            true
        }
        Err(why) => sheet_failure(why, emit),
    }
}

async fn request_grant(
    accounts: &Accounts<DbusTransport>,
    need: &Need,
    ask: &Ask,
    emit: &mut impl FnMut(Value),
) -> bool {
    let found = match accounts.find(need, ask.class, ask.usage).await {
        Ok(found) => found,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    match found {
        Found::One(candidate) => {
            emit(
                json!({ "result": "granted", "already": true, "candidate": candidate_json(&candidate) }),
            );
            true
        }
        Found::Several(several) => {
            let all: Vec<Value> = several.iter().map(candidate_json).collect();
            emit(json!({ "result": "several", "candidates": all }));
            true
        }
        Found::None(why) => {
            emit(json!({ "result": "none", "why": format!("{why:?}") }));
            false
        }
        Found::NeedsConsent(offer) => {
            match accounts
                .request_grant(&offer, &ParentWindow::Unparented)
                .await
            {
                Ok(candidate) => {
                    emit(
                        json!({ "result": "granted", "already": false, "candidate": candidate_json(&candidate) }),
                    );
                    true
                }
                Err(ClientError::Refused(refusal)) => {
                    emit(json!({ "result": "refused", "refusal": format!("{refusal:?}") }));
                    false
                }
                Err(why) => {
                    emit(failure(why));
                    false
                }
            }
        }
    }
}

fn chat(prompt: &str, ask: &Ask, tier: Tier) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts: vec![MessagePart::Text(prompt.to_owned())],
        }],
        shape: ReplyShape::Text,
        tier,
        class: ask.class,
        usage: ask.usage,
        tools: vec![],
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::Many,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
        },
    })
}

async fn open(
    accounts: &Accounts<DbusTransport>,
    need: &Need,
    ask: &Ask,
    tier: Tier,
    prompt: Option<String>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let options = OpenOptions {
        traceparent: None,
        usage: Some(ask.usage),
    };
    let mut session = match accounts.session_with(need, ask.class, tier, &options).await {
        Ok(session) => session,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    emit(json!({ "result": "opened" }));
    let Some(prompt) = prompt else {
        return true;
    };
    if let Err(why) = session
        .send(ClientFrame::Request(chat(&prompt, ask, tier)))
        .await
    {
        emit(failure(format!("cannot send: {why}")));
        return false;
    }
    loop {
        match session.next().await {
            Ok(event) => {
                let finished = matches!(event, InferEvent::Finished(_));
                emit(json!({ "event": serde_json::to_value(&event).unwrap_or(Value::Null) }));
                if finished {
                    return true;
                }
            }
            Err(why) => {
                emit(failure(format!("the session ended: {why}")));
                return false;
            }
        }
    }
}

async fn open_authenticated(
    accounts: &Accounts<DbusTransport>,
    need: &Need,
    ask: &Ask,
    family: Option<&str>,
    path: &str,
    emit: &mut impl FnMut(Value),
) -> bool {
    let candidate = match accounts.find(need, ask.class, ask.usage).await {
        Ok(Found::One(candidate)) => candidate,
        Ok(Found::Several(several)) => match several.into_iter().next() {
            Some(first) => first,
            None => {
                emit(failure("no account"));
                return false;
            }
        },
        Ok(Found::NeedsConsent(_)) => {
            emit(failure(
                "the app holds no grant for this need: run request-grant first",
            ));
            return false;
        }
        Ok(Found::None(why)) => {
            emit(failure(format!("no account fits: {why:?}")));
            return false;
        }
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    let endpoint = candidate
        .endpoints
        .iter()
        .find(|e| family.is_none_or(|f| e.family.slug() == f));
    let Some(endpoint) = endpoint else {
        emit(failure(format!(
            "the account lists no such server: {:?}",
            candidate
                .endpoints
                .iter()
                .map(|e| e.family.slug())
                .collect::<Vec<_>>()
        )));
        return false;
    };
    let stream = match accounts
        .open_authenticated(&candidate.grant, &endpoint.url)
        .await
    {
        Ok(porter_client::AuthenticatedStream::Fd(fd)) => fd,
        Ok(_) => {
            emit(failure("the relay is not a descriptor"));
            return false;
        }
        Err(ClientError::Refused(refusal)) => {
            emit(json!({ "result": "refused", "refusal": format!("{refusal:?}") }));
            return false;
        }
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    let http = endpoint.family.relay_protocol() == Some(porter_core::EndpointProtocol::Http);
    let host = endpoint
        .url
        .as_str()
        .split("://")
        .nth(1)
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_owned();
    match talk(stream, http, &host, path).await {
        Ok(said) => {
            emit(
                json!({ "result": "ok", "family": endpoint.family.slug(), "endpoint": endpoint.url.as_str(), "said": said }),
            );
            true
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

/// Reads the greeting of a line protocol, or makes one `GET` of an HTTP server and returns its
/// status and the start of its answer.
async fn talk(fd: OwnedFd, http: bool, host: &str, path: &str) -> std::io::Result<Value> {
    let std_stream = std::os::unix::net::UnixStream::from(fd);
    std_stream.set_nonblocking(true)?;
    let mut stream = UnixStream::from_std(std_stream)?;
    if http {
        let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
    }
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    let wait = Duration::from_secs(10);
    while !complete(&seen, http) && seen.len() < 64 * 1024 {
        match tokio::time::timeout(wait, stream.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => seen.extend_from_slice(&buf[..n]),
            Ok(Err(why)) => return Err(why),
        }
    }
    let text = String::from_utf8_lossy(&seen).into_owned();
    let first = text.lines().next().unwrap_or_default().to_owned();
    Ok(match http {
        true => {
            let status = first.split(' ').nth(1).and_then(|s| s.parse::<u16>().ok());
            let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
            json!({ "status": status, "line": first, "body": body.chars().take(512).collect::<String>() })
        }
        false => json!({ "greeting": first }),
    })
}

/// Whether enough has been read: a first line for a line protocol, the whole head and, when
/// the answer says how long its body is, the body, for HTTP.
fn complete(seen: &[u8], http: bool) -> bool {
    match http {
        false => seen.contains(&b'\n'),
        true => {
            let text = String::from_utf8_lossy(seen);
            let Some((head, body)) = text.split_once("\r\n\r\n") else {
                return false;
            };
            let length = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().to_owned())
                })
                .and_then(|v| v.parse::<usize>().ok());
            length.is_some_and(|n| body.len() >= n)
        }
    }
}

async fn sync_resolve(
    connection: &zbus::Connection,
    dataset: &str,
    number: i64,
    how: &str,
    emit: &mut impl FnMut(Value),
) -> bool {
    let sync = match SyncProxy::new(connection).await {
        Ok(sync) => sync,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    match sync.resolve(dataset, number, how).await {
        Ok(()) => {
            emit(json!({ "result": "resolved", "dataset": dataset, "number": number, "how": how }));
            true
        }
        Err(zbus::Error::MethodError(name, text, _)) => {
            emit(
                json!({ "result": "error", "name": name.as_str(), "error": text.unwrap_or_default() }),
            );
            false
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn status(
    connection: &zbus::Connection,
    dataset: &str,
    emit: &mut impl FnMut(Value),
) -> bool {
    let answer = async {
        let sync = SyncProxy::new(connection).await?;
        sync.status(dataset).await
    }
    .await;
    match answer {
        Ok(details) => {
            let fields = from_vardict(&details)
                .map(Value::Object)
                .unwrap_or(Value::Null);
            emit(json!({ "result": "ok", "dataset": dataset, "status": fields }));
            true
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn watch_conflicts(
    connection: &zbus::Connection,
    count: Option<usize>,
    timeout: Option<Duration>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let sync = match SyncProxy::new(connection).await {
        Ok(sync) => sync,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    let mut conflicts = match sync.receive_conflict().await {
        Ok(stream) => stream,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    // Calling joins syncd's roster: it tells only the connections it has heard from.
    let names = match sync.datasets().await {
        Ok(names) => names,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    emit(json!({ "event": "watching", "datasets": names }));
    let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
    let mut seen = 0usize;
    loop {
        if count.is_some_and(|n| seen >= n) {
            return true;
        }
        let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut conflicts).poll_next(cx));
        let signal = match deadline {
            Some(at) => match tokio::time::timeout_at(at, next).await {
                Ok(signal) => signal,
                Err(_) => {
                    emit(json!({ "result": "timeout", "seen": seen }));
                    return seen > 0;
                }
            },
            None => next.await,
        };
        let Some(signal) = signal else {
            emit(failure("the signal stream ended"));
            return false;
        };
        let Ok(args) = signal.args() else { continue };
        let conflict = from_vardict(args.conflict())
            .map(Value::Object)
            .unwrap_or(Value::Null);
        emit(json!({ "event": "conflict", "dataset": args.dataset(), "conflict": conflict }));
        seen += 1;
    }
}
