//! What `porter-rig-client` does: one command as an app, over the session bus, answered as JSON
//! lines. Each command emits one or more objects with a `result` or an `event` key; a failure
//! is `{"result":"error", ...}` and the process exits 1.
//!
//! The commands are the app side of the scenarios: list the identities and continue as one,
//! ask for a permit to use the person's AI keys and add a key, run the agent launcher's calls.
//! No credential crosses a bus, so there is nothing to ask a grant for or to open a relay to. The
//! app's identity is the daemons' to find (see `fixture`). The sync commands (settle a conflict,
//! watch for them) are `ferry-rig-client`'s.

use clap::Subcommand;
use porter_capability::AgentProgram;
use porter_client::{Accounts, AccountsError, AiKeysError, Identities, IdentitiesError, Porter};
use porter_client_dbus::{Bus, Launcher, LauncherError};
use porter_core::ProviderHint;
use porter_core::SessionToken;
use porter_core::consent::Usage;
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{DataClass, IdentityId};
use porter_wire::ParentWindow;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

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

fn failure(why: impl std::fmt::Display) -> Value {
    json!({ "result": "error", "error": why.to_string() })
}

/// The commands: the command line of `porter-rig-client`, each word and id parsed where it enters
/// (a word that is none is a `{"result":"error"}` line and exit 1, as a bus failure is).
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// The identities signed in on this machine, and "Continue as".
    #[command(subcommand)]
    Identities(IdentitiesCommand),
    /// The person's AI keys: ask for a permit to use them (adding one is `add-account`).
    #[command(subcommand)]
    AiKeys(AiKeysCommand),
    /// The agent launcher's calls to accountd.
    #[command(subcommand)]
    Launcher(LauncherCommand),
    /// Opens accountd's add-account sheet (`Manager.AddAccount`) and waits for the Request's
    /// `Response`: the account added, or why not.
    AddAccount {
        /// The provider's file id (`microsoft`, `nextcloud`, ...); the list when absent.
        #[arg(value_parser = provider)]
        provider: Option<ProviderId>,
    },
    /// Opens the sheet that signs an account in again (`Account.Reauthenticate`) and waits for
    /// the `Response`.
    Reauthenticate {
        /// The account.
        #[arg(value_parser = account)]
        account: AccountId,
    },
    /// Only writes the identity fixture for process `pid` (it is kept); not a bus command.
    WriteIdentity {
        /// The process the fixture names.
        #[arg(long)]
        pid: u32,
    },
}

/// The identities signed in on this machine, and "Continue as".
#[derive(Debug, Clone, Subcommand)]
pub enum IdentitiesCommand {
    /// Lists the identities signed in on this machine (`Identities1.List`).
    List,
    /// "Continue as": shows the identity picker (`Identities1.Offer`) and prints the pick, never a
    /// token.
    Offer {
        /// Limits the picker to one provider's identities (its file id); none: any.
        #[arg(value_parser = provider)]
        provider: Option<ProviderId>,
    },
    /// Says this app signed in with the identity (`Identities1.RegisterUse`).
    RegisterUse {
        /// The identity.
        #[arg(value_parser = identity)]
        identity: IdentityId,
    },
}

/// The person's AI keys: ask for a permit to use them (adding one is `add-account`).
#[derive(Debug, Clone, Subcommand)]
pub enum AiKeysCommand {
    /// Asks accountd for a permit to use the person's AI keys (`Accounts1.Keys.RequestKey`).
    Request {
        /// The data class.
        #[arg(long, default_value = "files", value_parser = word::<DataClass>)]
        class: DataClass,
        /// `interactive` or `background`.
        #[arg(long, default_value = "interactive", value_parser = word::<Usage>)]
        usage: Usage,
    },
}

/// The agent launcher's calls to accountd.
#[derive(Debug, Clone, Subcommand)]
pub enum LauncherCommand {
    /// As the agent launcher: registers `program`, optionally opens `session`, and asks the person
    /// which of their AI keys the agent may use (`Launcher.RequestAgentKey`).
    RequestKey {
        /// The agent program.
        #[arg(value_parser = program)]
        program: AgentProgram,
        /// The data class.
        #[arg(long, default_value = "files", value_parser = word::<DataClass>)]
        class: DataClass,
        /// A session to open first, for "this session only".
        #[arg(long, value_parser = session)]
        session: Option<SessionToken>,
    },
    /// As the agent launcher: registers `program` and asks for a process credential under
    /// `permit` (`Launcher.IssueProcessCredential`, on a memfd). Prints its id, never the key.
    Issue {
        /// The permit.
        #[arg(value_parser = permit)]
        permit: GrantId,
        /// The agent program.
        #[arg(value_parser = program)]
        program: AgentProgram,
    },
}

fn provider(id: &str) -> Result<ProviderId, String> {
    ProviderId::parse(id).map_err(|e| format!("`{id}` is not a provider: {e}"))
}

fn program(name: &str) -> Result<AgentProgram, String> {
    AgentProgram::parse(name).map_err(|e| format!("`{name}` is not an agent program: {e}"))
}

fn identity(id: &str) -> Result<IdentityId, String> {
    IdentityId::parse(id).map_err(|e| format!("`{id}` is not an identity id: {e}"))
}

fn account(id: &str) -> Result<AccountId, String> {
    AccountId::parse(id).map_err(|e| format!("`{id}` is not an account id: {e}"))
}

fn permit(id: &str) -> Result<GrantId, String> {
    GrantId::parse(id).map_err(|e| format!("`{id}` is not a permit id: {e}"))
}

fn session(text: &str) -> Result<SessionToken, String> {
    SessionToken::parse(text).map_err(|e| format!("`{text}` is not a session: {e}"))
}

/// Runs `command` as the app on `connection`, handing every JSON line to `emit`; whether it
/// succeeded.
pub async fn run(
    connection: &zbus::Connection,
    command: Command,
    emit: &mut impl FnMut(Value),
) -> bool {
    let accounts = Porter::over(Bus::over(connection.clone())).accounts();
    match command {
        Command::Identities(IdentitiesCommand::List) => identities_list(connection, emit).await,
        Command::Identities(IdentitiesCommand::Offer { provider }) => {
            identities_offer(connection, provider, emit).await
        }
        Command::Identities(IdentitiesCommand::RegisterUse { identity }) => {
            identities_register_use(connection, &identity, emit).await
        }
        Command::AiKeys(AiKeysCommand::Request { class, usage }) => {
            ai_keys_request(connection, &Ask { class, usage }, emit).await
        }
        Command::Launcher(LauncherCommand::RequestKey {
            program,
            class,
            session,
        }) => launcher_request_key(connection, &program, class, session.as_ref(), emit).await,
        Command::Launcher(LauncherCommand::Issue { permit, program }) => {
            launcher_issue(connection, &permit, &program, emit).await
        }
        Command::AddAccount { provider } => add_account(&accounts, provider, emit).await,
        Command::Reauthenticate { account } => reauthenticate(&accounts, &account, emit).await,
        Command::WriteIdentity { .. } => {
            emit(failure("write-identity is not a bus command"));
            false
        }
    }
}

/// A refusal as the rig prints it, `refused` with the word for it.
fn refused(refusal: porter_wire::Refusal, emit: &mut impl FnMut(Value)) -> bool {
    emit(json!({ "result": "refused", "refusal": format!("{refusal:?}") }));
    false
}

/// The Identities of this app on `connection`.
fn identities_of(connection: &zbus::Connection) -> Identities<Bus> {
    Porter::over(Bus::over(connection.clone())).identities()
}

async fn identities_list(connection: &zbus::Connection, emit: &mut impl FnMut(Value)) -> bool {
    let listed = identities_of(connection).list().await;
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
    let picked = identities_of(connection)
        .offer(&hint, &ParentWindow::Unparented)
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
    let said = identities_of(connection).register_use(identity).await;
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

async fn ai_keys_request(
    connection: &zbus::Connection,
    ask: &Ask,
    emit: &mut impl FnMut(Value),
) -> bool {
    let permit = Porter::over(Bus::over(connection.clone()))
        .ai_keys()
        .request_key(ask.class, ask.usage, &ParentWindow::Unparented)
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
    let launcher = Bus::over(connection.clone()).launcher();
    launcher.register(std::slice::from_ref(program)).await?;
    Ok(launcher)
}

async fn launcher_request_key(
    connection: &zbus::Connection,
    program: &AgentProgram,
    class: DataClass,
    session: Option<&SessionToken>,
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
    permit: &GrantId,
    program: &AgentProgram,
    emit: &mut impl FnMut(Value),
) -> bool {
    let credential = async {
        let launcher = launcher_for(connection, program).await?;
        launcher.issue_credential(permit, program).await
    }
    .await;
    match credential {
        // The id; the key itself is never printed.
        Ok(credential) => {
            emit(json!({
                "result": "issued",
                "id": credential.id().as_str(),
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
fn sheet_failure(why: AccountsError, emit: &mut impl FnMut(Value)) -> bool {
    match why {
        AccountsError::Refused(refusal) => {
            emit(json!({ "result": "refused", "refusal": format!("{refusal:?}") }));
        }
        other => emit(failure(other)),
    }
    false
}

async fn add_account(
    accounts: &Accounts<Bus>,
    provider: Option<ProviderId>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let hint = provider.map_or(ProviderHint::Any, ProviderHint::Provider);
    match accounts.add(hint, &ParentWindow::Unparented).await {
        Ok(account) => {
            emit(json!({ "result": "added", "account": account.as_str() }));
            true
        }
        // The account was already here: nothing was added, and this is the one it is.
        Err(AccountsError::AlreadyAdded(account)) => {
            emit(json!({ "result": "already_added", "account": account.as_str() }));
            false
        }
        Err(why) => sheet_failure(why, emit),
    }
}

async fn reauthenticate(
    accounts: &Accounts<Bus>,
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
