//! accountd's calls over the bus: the methods that answer at once (`Query`, `Availability`,
//! `Grants.List` and `Revoke`, `Tokens.IssueToken`). A refusal comes back as the error
//! `org.quire.Accounts1.Error.<Refusal>` and is returned as `AccountsReply::Refused`, as the
//! in-process carrier does. The sheet methods (`Choose`, `AddAccount`, `Reauthenticate`) return a
//! Request object whose `Response` signal carries the answer; that flow waits for accountd.

use super::dbus::{bus_error, slug};
use crate::error::TransportError;
use porter_core::consent::Availability;
use porter_core::{AccountsReply, AccountsRequest, Candidate, DataClass, Need};
use porter_dbus::{
    BusConnection, BusError, GrantsProxy, ManagerProxy, TokensProxy, candidate_from_dbus,
    grant_from_dbus, need_to_dbus, refusal_of, token_from_dbus,
};

/// A call that came back as a value, or as a refusal accountd gave, or not at all.
fn settled<T>(
    result: Result<T, BusError>,
    reply: impl FnOnce(T) -> Result<AccountsReply, TransportError>,
) -> Result<AccountsReply, TransportError> {
    match result {
        Ok(value) => reply(value),
        Err(error) => match refusal_of(&error) {
            Some(refusal) => Ok(AccountsReply::Refused(refusal)),
            None => Err(bus_error(&error)),
        },
    }
}

fn malformed(error: impl std::fmt::Display) -> TransportError {
    TransportError::Malformed(error.to_string())
}

fn candidates(list: Vec<porter_dbus::CandidateArg>) -> Result<Vec<Candidate>, TransportError> {
    list.into_iter()
        .map(|arg| candidate_from_dbus(arg).map_err(malformed))
        .collect()
}

fn availability(slug: &str) -> Result<Availability, TransportError> {
    serde_json::from_value(serde_json::Value::String(slug.to_owned())).map_err(malformed)
}

/// The sheet methods are not served yet.
fn sheet() -> TransportError {
    TransportError::Malformed(
        "sheet requests need accountd's Request objects, which are not served yet".to_owned(),
    )
}

async fn manager(connection: &BusConnection) -> Result<ManagerProxy<'_>, TransportError> {
    ManagerProxy::new(connection)
        .await
        .map_err(|e| bus_error(&e))
}

async fn query(
    connection: &BusConnection,
    need: &Need,
    class: DataClass,
    usage: &impl serde::Serialize,
) -> Result<AccountsReply, TransportError> {
    let proxy = manager(connection).await?;
    let result = proxy
        .query(&need_to_dbus(need), &slug(&class)?, &slug(usage)?)
        .await;
    settled(result, |list| {
        candidates(list).map(AccountsReply::Candidates)
    })
}

/// One request to accountd over the bus.
pub(super) async fn call(
    connection: &BusConnection,
    request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    match request {
        AccountsRequest::Query { need, class, usage } => {
            query(connection, &need, class, &usage).await
        }
        AccountsRequest::Availability { need, class, usage } => {
            let proxy = manager(connection).await?;
            let result = proxy
                .availability(&need_to_dbus(&need), &slug(&class)?, &slug(&usage)?)
                .await;
            settled(result, |text| {
                availability(&text).map(AccountsReply::Availability)
            })
        }
        AccountsRequest::ListGrants => {
            let proxy = GrantsProxy::new(connection)
                .await
                .map_err(|e| bus_error(&e))?;
            settled(proxy.list().await, |list| {
                list.into_iter()
                    .map(|arg| grant_from_dbus(arg).map_err(malformed))
                    .collect::<Result<Vec<_>, _>>()
                    .map(AccountsReply::Grants)
            })
        }
        AccountsRequest::Revoke { grant } => {
            let proxy = GrantsProxy::new(connection)
                .await
                .map_err(|e| bus_error(&e))?;
            settled(proxy.revoke(grant.as_str()).await, |()| {
                Ok(AccountsReply::Revoked)
            })
        }
        AccountsRequest::IssueToken { grant, audience } => {
            let proxy = TokensProxy::new(connection)
                .await
                .map_err(|e| bus_error(&e))?;
            settled(
                proxy.issue_token(grant.as_str(), &audience.0).await,
                |arg| {
                    token_from_dbus(arg)
                        .map(AccountsReply::Token)
                        .map_err(malformed)
                },
            )
        }
        AccountsRequest::Choose { .. }
        | AccountsRequest::AddAccount { .. }
        | AccountsRequest::Reauthenticate { .. } => Err(sheet()),
    }
}
