//! `org.quire.Accounts1.Manager`: find, choose and add accounts.

use crate::callers::Callers;
use crate::core::{Core, Host, hint, slug, window};
use crate::errors::RefusedError;
use porter_core::consent::Availability;
use porter_core::{AccountsReply, AccountsRequest};
use porter_dbus::{CandidateArg, Details, NeedArg, SheetKind, candidate_to_dbus, need_from_dbus};
use std::sync::Arc;
use zbus::Connection;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

/// The manager object at the accountd path.
#[derive(Debug)]
pub(crate) struct Manager<H, C>(Arc<Core<H, C>>);

impl<H, C> Manager<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

fn mismatched(reply: AccountsReply) -> RefusedError {
    RefusedError::failed(format!("the service answered another request: {reply:?}"))
}

fn availability_slug(availability: Availability) -> Result<String, RefusedError> {
    match serde_json::to_value(availability) {
        Ok(serde_json::Value::String(slug)) => Ok(slug),
        _ => Err(RefusedError::failed("an availability with no slug")),
    }
}

#[zbus::interface(name = "org.quire.Accounts1.Manager")]
impl<H: Host, C: Callers> Manager<H, C> {
    async fn query(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> Result<Vec<CandidateArg>, RefusedError> {
        let request = AccountsRequest::Query {
            need: need_from_dbus(need).map_err(RefusedError::invalid)?,
            class: slug(&class)?,
            usage: slug(&usage)?,
        };
        match self.0.answer(&header, request).await? {
            AccountsReply::Candidates(list) => Ok(list.iter().map(candidate_to_dbus).collect()),
            other => Err(mismatched(other)),
        }
    }

    async fn availability(
        &self,
        #[zbus(header)] header: Header<'_>,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> Result<String, RefusedError> {
        let request = AccountsRequest::Availability {
            need: need_from_dbus(need).map_err(RefusedError::invalid)?,
            class: slug(&class)?,
            usage: slug(&usage)?,
        };
        match self.0.answer(&header, request).await? {
            AccountsReply::Availability(availability) => availability_slug(availability),
            other => Err(mismatched(other)),
        }
    }

    // The signature is the interface's: five arguments, and the caller's header and connection.
    #[allow(clippy::too_many_arguments)]
    async fn choose(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        need: NeedArg,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let request = AccountsRequest::Choose {
            need: need_from_dbus(need).map_err(RefusedError::invalid)?,
            class: slug(&class)?,
            usage: slug(&usage)?,
            window: window(&parent_window),
        };
        self.0
            .sheet(&header, connection, SheetKind::Choose, &options, request)
            .await
    }

    async fn add_account(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        provider_hint: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let request = AccountsRequest::AddAccount {
            hint: hint(&provider_hint)?,
            window: window(&parent_window),
        };
        self.0
            .sheet(
                &header,
                connection,
                SheetKind::AddAccount,
                &options,
                request,
            )
            .await
    }
}
