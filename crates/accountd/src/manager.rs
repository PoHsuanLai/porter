//! `org.quire.Accounts1.Manager`: find, choose and add accounts.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing, hint, slug, window};
use crate::errors::RefusedError;
use porter_core::consent::Availability;
use porter_core::wire::Refusal;
use porter_core::{AccountState, AccountsReply, AccountsRequest};
use porter_dbus::{
    CallerRole, CandidateArg, Details, NeedArg, SheetKind, account_path, candidate_to_dbus,
    need_from_dbus,
};
use std::sync::Arc;
use zbus::Connection;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::ObjectPath;
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
        match self.0.answer(&header, Standing::Any, request).await? {
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
        match self.0.answer(&header, Standing::Any, request).await? {
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

    /// The accounts that need signing in again now, for the shell alone (`SheetHost`). Asking
    /// joins the caller to the roster, so it is then sent `NeedsReauth` and `State` changes for
    /// every account.
    async fn needing_reauth(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<(OwnedObjectPath, String)>, RefusedError> {
        let caller = self.0.identify(&header, Standing::Any).await?;
        if caller.role != CallerRole::SheetHost {
            return Err(RefusedError::of(Refusal::Denied));
        }
        self.0
            .host
            .registry()
            .accounts
            .iter()
            .filter(|account| account.state == AccountState::NeedsReauth)
            .map(|account| {
                OwnedObjectPath::try_from(account_path(&account.id))
                    .map(|path| (path, account.label.0.clone()))
                    .map_err(|e| RefusedError::failed(e.to_string()))
            })
            .collect()
    }

    #[zbus(signal)]
    pub(crate) async fn account_added(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn account_removed(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn capability_changed(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn needs_reauth(
        emitter: &SignalEmitter<'_>,
        account: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn grant_changed(emitter: &SignalEmitter<'_>, grant: &str)
    -> zbus::Result<()>;
}
