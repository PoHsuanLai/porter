//! `org.quire.Accounts1.Broker` at `/org/quire/Accounts1`, on accountd's name: what the AI
//! broker (inferd) asks of the daemon that keeps the AI keys. inferd is the one that sends a key
//! to an AI company, so it asks accountd which accounts an app holds a permit on, and for the key
//! behind a permit. Callable only by a connection whose caller role is `AiBroker`; accountd
//! refuses every other sender `AccessDenied`.
//!
//! The app is named by the broker from its own connection, never by the app. A key leaves
//! accountd only on a sealed memfd (`h`), once per call, and is audited; it is in no body, no
//! string and no signal.
//!
//! - `Verdicts(app, class, usage)`: what the permits say for the app on each account that serves
//!   language models: the account's id, then a vardict of the [`LlmVerdict`] without it.
//! - `ReadKey(grant)`: the key behind an allow permit of the language-model kind. An allow-once
//!   permit is spent by a read that succeeded.
//! - `Accounts()`: the AI accounts, for the rows the broker adds to them (what each spent).
//! - `Servers()` and `ReadServerKey(account)`: the accounts that are servers of the person's own,
//!   and the key of one, for the one request that asks it for its models. No permit is needed.
//! - `ReportLocal(provider, claims, state)`: a probed local runtime as an account; `claims` is the
//!   JSON of the models as claims, `state` the account's state word (`ok`, `offline`).
//! - `Changed()`: an AI account, a permit or a switch changed; the broker asks again.

use crate::Details;
use crate::json_value::{bad, from_vardict, to_vardict};
use porter_core::{AiAccountRow, CoreError, LlmVerdict, OwnServerRow};
use porter_dbus_macros::bus_interface;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value as Json};
use zbus::zvariant::OwnedFd;

/// A row as its account's id and a vardict of the rest by name (`(sa{sv})`).
pub type RowArg = (String, Details);

/// The interface, written once. `#[bus_interface]` makes [`BrokerProxy`] for its callers from this
/// trait, [`BrokerServer`] for the daemon to implement and [`BrokerServed`] to register.
#[bus_interface(
    interface = "org.quire.Accounts1.Broker",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Broker {
    /// What the permits say for `app` (its reverse-DNS name and its isolation slug) on each
    /// account that serves language models, for data of `class` used as `usage` says.
    fn verdicts(
        &self,
        app: &(String, String),
        class: &str,
        usage: &str,
    ) -> zbus::Result<Vec<RowArg>>;
    /// The key behind an allow permit of the language-model kind, on a sealed memfd.
    fn read_key(&self, grant: &str) -> zbus::Result<OwnedFd>;
    /// The AI accounts: the account's id, then `provider` (`s`), `label` (`s`) and `network` (`b`).
    fn accounts(&self) -> zbus::Result<Vec<RowArg>>;
    /// The accounts that are servers of the person's own: the account's id, then `label` (`s`)
    /// and `endpoint` (the chat-completions endpoint, by name).
    fn servers(&self) -> zbus::Result<Vec<RowArg>>;
    /// The key of an own server, on a sealed memfd.
    fn read_server_key(&self, account: &str) -> zbus::Result<OwnedFd>;
    /// Reports a probed local runtime as the account of `provider`, its models as the JSON of
    /// the claims and its state (`ok` or `offline`); the account's id.
    fn report_local(&self, provider: &str, claims: &str, state: &str) -> zbus::Result<String>;
    /// An AI account, a permit or a switch changed. Sent to the broker.
    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
}

/// `row` as `(id, the rest by name)`; `id_field` is the field the id is taken from.
fn to_row<T: Serialize>(row: &T, id_field: &str) -> RowArg {
    let mut fields = match serde_json::to_value(row) {
        Ok(Json::Object(fields)) => fields,
        _ => Map::new(),
    };
    let id = match fields.remove(id_field) {
        Some(Json::String(id)) => id,
        _ => String::new(),
    };
    (id, to_vardict(&fields))
}

/// The row an `(id, the rest by name)` value carries, or why it is not one.
fn from_row<T: DeserializeOwned>(row: RowArg, id_field: &str, what: &str) -> Result<T, CoreError> {
    let (id, details) = row;
    let mut fields = from_vardict(&details)?;
    fields.insert(id_field.to_owned(), Json::String(id));
    serde_json::from_value(Json::Object(fields)).map_err(|e| bad(&format!("{what}: {e}")))
}

/// A permit verdict as the account's id and the rest by name.
pub fn verdict_to_dbus(verdict: &LlmVerdict) -> RowArg {
    to_row(verdict, "account")
}

/// The verdict a D-Bus value carries, or why it is not one.
///
/// # Errors
/// The value is not a verdict.
pub fn verdict_from_dbus(row: RowArg) -> Result<LlmVerdict, CoreError> {
    from_row(row, "account", "verdict")
}

/// An AI account as its id and the rest by name.
pub fn ai_account_to_dbus(account: &AiAccountRow) -> RowArg {
    to_row(account, "account")
}

/// The AI account a D-Bus value carries, or why it is not one.
///
/// # Errors
/// The value is not an account row.
pub fn ai_account_from_dbus(row: RowArg) -> Result<AiAccountRow, CoreError> {
    from_row(row, "account", "account")
}

/// An own server as its account's id and the rest by name.
pub fn own_server_to_dbus(server: &OwnServerRow) -> RowArg {
    to_row(server, "account")
}

/// The own server a D-Bus value carries, or why it is not one.
///
/// # Errors
/// The value is not a server row.
pub fn own_server_from_dbus(row: RowArg) -> Result<OwnServerRow, CoreError> {
    from_row(row, "account", "server")
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::consent::Verdict;
    use porter_core::{AccountId, AccountState, GrantId, SessionToken};
    use porter_core::{Family, LoginName, ServiceEndpoint, Tls};

    fn id(text: &str) -> AccountId {
        AccountId::parse(text).expect("id")
    }

    #[test]
    fn a_verdict_survives_the_bus_shape_for_each_answer_and_scope() {
        let scopes = [
            porter_core::consent::GrantScope::Once,
            porter_core::consent::GrantScope::Always,
            porter_core::consent::GrantScope::Session(SessionToken::parse("s1").expect("id")),
        ];
        let mut verdicts = vec![Verdict::Denied, Verdict::Ask];
        verdicts.extend(scopes.into_iter().map(|scope| Verdict::Granted {
            grant: GrantId::parse("g.1").expect("id"),
            scope,
        }));
        for verdict in verdicts {
            let row = LlmVerdict::new(
                id("openrouter"),
                "openrouter".into(),
                "Work".into(),
                Some("OpenRouter".into()),
                AccountState::Ok,
                verdict,
            );
            let wire = verdict_to_dbus(&row);
            assert_eq!(wire.0, "openrouter");
            assert!(!wire.1.contains_key("account"));
            assert_eq!(verdict_from_dbus(wire).expect("back"), row);
        }
    }

    #[test]
    fn a_verdict_without_a_provider_label_leaves_the_key_out_and_reads_back() {
        let row = LlmVerdict::new(
            id("ollama"),
            "ollama".into(),
            "Ollama".into(),
            None,
            AccountState::Offline,
            Verdict::Ask,
        );
        let wire = verdict_to_dbus(&row);
        assert!(!wire.1.contains_key("provider_label"));
        assert_eq!(verdict_from_dbus(wire).expect("back"), row);
    }

    #[test]
    fn an_account_and_an_own_server_survive_the_bus_shape() {
        let account = AiAccountRow::new(id("a.b"), "ollama".into(), "Ollama".into(), false);
        assert_eq!(
            ai_account_from_dbus(ai_account_to_dbus(&account)).expect("back"),
            account
        );
        let endpoint = ServiceEndpoint {
            family: Family::ChatCompletions,
            url: porter_core::EndpointUrl::parse("http://127.0.0.1:8080/v1").expect("url"),
            tls: Tls::Plain,
            login: LoginName(String::new()),
        };
        let server = OwnServerRow::new(id("mine"), "Mine".into(), endpoint);
        assert_eq!(
            own_server_from_dbus(own_server_to_dbus(&server)).expect("back"),
            server
        );
    }

    #[test]
    fn a_value_that_is_not_a_row_is_refused() {
        assert!(verdict_from_dbus(("x".into(), Details::new())).is_err());
        assert!(ai_account_from_dbus(("x".into(), Details::new())).is_err());
        assert!(own_server_from_dbus(("x".into(), Details::new())).is_err());
    }
}
