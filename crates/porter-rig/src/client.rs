//! What `porter-rig-client` does: one command as an app, over the session bus, answered as JSON
//! lines. Each command emits one or more objects with a `result` or an `event` key; a failure
//! is `{"result":"error", ...}` and the process exits 1.
//!
//! The commands are the app side of the scenarios: list the identities and continue as one,
//! ask for a permit to use the person's AI keys and add a key, run the agent launcher's calls,
//! open an AI session, settle a sync conflict, watch for them. No credential crosses a bus, so
//! there is nothing to ask a grant for or to open a relay to. The app's identity is the
//! daemons' to find (see `fixture`).

use porter_capability::Tier;
use porter_capability::{Access, AgentProgram, Delta, Offered, QuotaReport, StorageScope};
use porter_client::{
    Accounts, AiKeys, AiKeysError, ClientError, DbusTransport, Identities, IdentitiesError,
    InferSession, Launcher, LauncherError, OpenOptions,
};
use porter_core::LauncherSession;
use porter_core::ProviderHint;
use porter_core::audit::Handoff;
use porter_core::consent::Usage;
use porter_core::{AccountId, KeyPermitId, ProviderId};
use porter_core::{DataClass, IdentityId};
use porter_dbus::{SyncProxy, from_vardict};
use porter_ids::Tokens;
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRequest, MessagePart,
    Role, ToolParallelism,
};
use porter_match::Need;
use porter_match::{LlmNeed, MailNeed, NotesNeed, PimNeed, StorageNeed};
use porter_wire::ParentWindow;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;
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
    let pim = || PimNeed::new(Access::Read, Delta::None);
    Ok(match text {
        "mail" => Need::Mail(MailNeed::new(Access::Read, Offered::Absent, Delta::None)),
        "storage" => Need::Storage(StorageNeed::new(
            Access::ReadWrite,
            Delta::Poll,
            StorageScope::AppFolder,
            QuotaReport::Unreported,
        )),
        "calendar" => Need::Calendar(pim()),
        "contacts" => Need::Contacts(pim()),
        "notes" => Need::Notes(NotesNeed::new(Access::Read, Delta::None)),
        "llm" => Need::Llm(LlmNeed::new(Default::default(), Tokens(1_000))),
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

/// The commands.
#[derive(Debug, Clone)]
pub enum Command {
    /// Lists the identities signed in on this machine (`Identities1.List`).
    IdentitiesList,
    /// "Continue as": shows the identity picker (`Identities1.Offer`) and prints the pick, never a
    /// token.
    IdentitiesOffer {
        /// Limits the picker to one provider's identities; none: any.
        provider: Option<ProviderId>,
    },
    /// Says this app signed in with the identity (`Identities1.RegisterUse`).
    IdentitiesRegisterUse {
        /// The identity.
        identity: IdentityId,
    },
    /// Shows inferd's add-a-key sheet (`Inference1.Keys.AddAccount`) and waits for the answer.
    AiKeysAdd {
        /// The provider whose form opens first; none: the list.
        provider: Option<ProviderId>,
    },
    /// Asks inferd for a permit to use the person's AI keys (`Inference1.Keys.RequestKey`).
    AiKeysRequest {
        /// Class and usage.
        ask: Ask,
    },
    /// As the agent launcher: registers `program`, optionally opens `session`, and asks the person
    /// which of their AI keys the agent may use (`Launcher.RequestAgentKey`).
    LauncherRequestKey {
        /// The agent program.
        program: AgentProgram,
        /// The data class.
        class: DataClass,
        /// A session to open first, for "this session only".
        session: Option<LauncherSession>,
    },
    /// As the agent launcher: registers `program` and asks for a process credential under
    /// `permit` (`Launcher.IssueProcessCredential`). Prints its id, never the key.
    LauncherIssue {
        /// The permit.
        permit: KeyPermitId,
        /// The agent program.
        program: AgentProgram,
        /// How the key is handed over.
        target: Handoff,
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
        Command::IdentitiesList => identities_list(connection, emit).await,
        Command::IdentitiesOffer { provider } => identities_offer(connection, provider, emit).await,
        Command::IdentitiesRegisterUse { identity } => {
            identities_register_use(connection, &identity, emit).await
        }
        Command::AiKeysAdd { provider } => ai_keys_add(connection, provider, emit).await,
        Command::AiKeysRequest { ask } => ai_keys_request(connection, &ask, emit).await,
        Command::LauncherRequestKey {
            program,
            class,
            session,
        } => launcher_request_key(connection, &program, class, session.as_ref(), emit).await,
        Command::LauncherIssue {
            permit,
            program,
            target,
        } => launcher_issue(connection, &permit, &program, target, emit).await,
        Command::Open {
            need,
            ask,
            tier,
            prompt,
        } => open(&accounts, &need, &ask, tier, prompt, emit).await,
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
    }
}

/// A refusal as the rig prints it, `refused` with the word for it.
fn refused(refusal: porter_wire::Refusal, emit: &mut impl FnMut(Value)) -> bool {
    emit(json!({ "result": "refused", "refusal": format!("{refusal:?}") }));
    false
}

async fn identities_list(connection: &zbus::Connection, emit: &mut impl FnMut(Value)) -> bool {
    let listed = async { Identities::connect(connection).await?.list().await }.await;
    match listed {
        Ok(identities) => {
            let rows: Vec<Value> = identities
                .iter()
                .map(|identity| serde_json::to_value(identity).unwrap_or(Value::Null))
                .collect();
            emit(json!({ "result": "ok", "identities": rows }));
            true
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn identities_offer(
    connection: &zbus::Connection,
    provider: Option<ProviderId>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let hint = provider.map_or(ProviderHint::Any, ProviderHint::Provider);
    let picked = async {
        Identities::connect(connection)
            .await?
            .offer(&hint, &ParentWindow::Unparented)
            .await
    }
    .await;
    match picked {
        Ok(picked) => {
            emit(json!({
                "result": "picked",
                "identity": picked.id.as_str(),
                "address": picked.address.0,
                "provider": picked.provider.as_str(),
            }));
            true
        }
        Err(IdentitiesError::Cancelled) => {
            emit(json!({ "result": "refused", "refusal": "Cancelled" }));
            false
        }
        Err(IdentitiesError::NoIdentity) => {
            emit(json!({ "result": "none" }));
            false
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn identities_register_use(
    connection: &zbus::Connection,
    identity: &IdentityId,
    emit: &mut impl FnMut(Value),
) -> bool {
    let said = async {
        Identities::connect(connection)
            .await?
            .register_use(identity)
            .await
    }
    .await;
    match said {
        Ok(()) => {
            emit(json!({ "result": "registered", "identity": identity.as_str() }));
            true
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn ai_keys_add(
    connection: &zbus::Connection,
    provider: Option<ProviderId>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let hint = provider.map_or(ProviderHint::Any, ProviderHint::Provider);
    let added = async {
        AiKeys::connect(connection)
            .await?
            .add_account(hint, &ParentWindow::Unparented)
            .await
    }
    .await;
    match added {
        Ok(account) => {
            emit(json!({ "result": "added", "account": account.as_str() }));
            true
        }
        Err(AiKeysError::AlreadyAdded(account)) => {
            emit(json!({ "result": "already_added", "account": account.as_str() }));
            false
        }
        Err(AiKeysError::Refused(refusal)) => refused(refusal, emit),
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn ai_keys_request(
    connection: &zbus::Connection,
    ask: &Ask,
    emit: &mut impl FnMut(Value),
) -> bool {
    let permit = async {
        AiKeys::connect(connection)
            .await?
            .request_key(ask.class, ask.usage, &ParentWindow::Unparented)
            .await
    }
    .await;
    match permit {
        Ok(permit) => {
            emit(json!({ "result": "granted", "permit": permit.as_str() }));
            true
        }
        Err(AiKeysError::Refused(refusal)) => refused(refusal, emit),
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

/// The launcher, registered for `program`: only a registered connection may ask for its key.
async fn launcher_for(
    connection: &zbus::Connection,
    program: &AgentProgram,
) -> Result<Launcher, LauncherError> {
    let launcher = Launcher::connect(connection).await?;
    launcher.register(std::slice::from_ref(program)).await?;
    Ok(launcher)
}

async fn launcher_request_key(
    connection: &zbus::Connection,
    program: &AgentProgram,
    class: DataClass,
    session: Option<&LauncherSession>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let key = async {
        let launcher = launcher_for(connection, program).await?;
        if let Some(session) = session {
            launcher.begin_session(session).await?;
        }
        launcher.request_agent_key(program, class, session).await
    }
    .await;
    match key {
        Ok(key) => {
            emit(json!({
                "result": "granted",
                "permit": key.permit.as_str(),
                "account": key.account.as_str(),
            }));
            true
        }
        Err(LauncherError::Refused(refusal)) => refused(refusal, emit),
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn launcher_issue(
    connection: &zbus::Connection,
    permit: &KeyPermitId,
    program: &AgentProgram,
    target: Handoff,
    emit: &mut impl FnMut(Value),
) -> bool {
    let credential = async {
        let launcher = launcher_for(connection, program).await?;
        launcher.issue_credential(permit, program, target).await
    }
    .await;
    match credential {
        // The id and the way it was handed over; the key itself is never printed.
        Ok(credential) => {
            emit(json!({
                "result": "issued",
                "id": credential.id().as_str(),
                "handoff": target.word(),
            }));
            true
        }
        Err(LauncherError::Refused(refusal)) => refused(refusal, emit),
        Err(why) => {
            emit(failure(why));
            false
        }
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
        // The account was already here: nothing was added, and this is the one it is.
        Err(ClientError::AlreadyAdded(account)) => {
            emit(json!({ "result": "already_added", "account": account.as_str() }));
            false
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

fn chat(prompt: &str, ask: &Ask, tier: Tier) -> InferRequest {
    InferRequest::Chat(
        ChatRequest::new(
            vec![ChatMessage {
                role: Role::User,
                parts: vec![MessagePart::Text(prompt.to_owned())],
            }],
            tier,
            ask.class,
            ask.usage,
        )
        .with_control(ChatControl::new().with_tool_calls(ToolParallelism::Many)),
    )
}

async fn open(
    accounts: &Accounts<DbusTransport>,
    need: &Need,
    ask: &Ask,
    tier: Tier,
    prompt: Option<String>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let options = OpenOptions::default().with_usage(ask.usage);
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
