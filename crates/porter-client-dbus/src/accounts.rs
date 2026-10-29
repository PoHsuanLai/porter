//! accountd's calls over the bus: the two sheet methods it still has, `AddAccount` and
//! `Reauthenticate`. A refusal comes back as the error `org.quire.Accounts1.Error.<Refusal>`.
//!
//! The sheet methods return a Request object whose `Response` signal carries the answer
//! (`porter_dbus::accounts::sheet::reply_of` reads it: code 0 the answer, 1 `Dismissed`, 2 the
//! refusal named in the results). The signal may be sent before the method's reply arrives, so the
//! listener is subscribed first (`porter_dbus::accounts::Sheet`), and the call that waits closes
//! the sheet if it is dropped.

use crate::Bus;
use crate::failure::{accounts_error, link_error};
use futures_util::StreamExt;
use futures_util::stream::select;
use porter_client::{
    AccountChange, AccountRef, AccountView, AccountWatch, AccountsError, AccountsLink, Changes,
    LinkError, Malformed, NeedsReauth,
};
use porter_core::{AccountId, AccountLabel, AccountState, ProviderHint, ProviderId, ReauthReason};
use porter_dbus::accounts::account_path;
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{
    Abandon, AccountAdded, AccountProxy, AccountRemoved, ManagerProxy, Sheet, SheetError,
};
use porter_dbus::zvariant::OwnedObjectPath;
use porter_dbus::{BusError, Details};
use porter_ids::word::from_word;
use porter_wire::names::ACCOUNTS_PATH;
use porter_wire::{AccountsReply, ParentWindow};
use std::future::Future;

impl AccountsLink for Bus {
    async fn add(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> Result<AccountId, AccountsError> {
        let (hint, window) = (hint_text(hint), wire(window));
        let proxy: ManagerProxy<'static> = self
            .accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| link_error(&e))?;
        let reply = self
            .sheet(SheetKind::AddAccount, |options| async move {
                proxy.add_account(&hint, &window, &options).await
            })
            .await?;
        match reply {
            AccountsReply::Added(account) => Ok(account),
            AccountsReply::AlreadyAdded(account) => Err(AccountsError::AlreadyAdded(account)),
            AccountsReply::Refused(refusal) => Err(AccountsError::Refused(refusal)),
            _ => Err(LinkError::Malformed(Malformed::Reply).into()),
        }
    }

    async fn reauthenticate(
        &self,
        account: &AccountId,
        window: &ParentWindow,
    ) -> Result<(), AccountsError> {
        let window = wire(window);
        let builder = AccountProxy::builder(&self.connection)
            .path(account_path(account))
            .map_err(|e| link_error(&e))?;
        let proxy = self
            .accounts
            .aim(builder)
            .map_err(|e| link_error(&e))?
            .build()
            .await
            .map_err(|e| link_error(&e))?;
        let reply = self
            .sheet(SheetKind::Reauthenticate, |options| async move {
                proxy.reauthenticate(&window, &options).await
            })
            .await?;
        match reply {
            AccountsReply::Reauthenticated => Ok(()),
            AccountsReply::Refused(refusal) => Err(AccountsError::Refused(refusal)),
            _ => Err(LinkError::Malformed(Malformed::Reply).into()),
        }
    }
}

impl Bus {
    /// One sheet method: listen, call (naming the Request object through `handle_token`), then
    /// wait for the `Response` of the object the call returned. An error the call itself returns
    /// (a refusal before any sheet) is settled like an immediate call's.
    async fn sheet<F, Fut>(&self, kind: SheetKind, call: F) -> Result<AccountsReply, AccountsError>
    where
        F: FnOnce(Details) -> Fut,
        Fut: Future<Output = Result<OwnedObjectPath, BusError>>,
    {
        let mut listener = Sheet::subscribe(&self.connection)
            .await
            .map_err(|e| link_error(&e))?;
        // Armed before the call: a dropped call may already have made the object.
        let mut abandon = Abandon(listener.closer(None));
        let path = match call(listener.options()).await {
            Ok(path) => path,
            Err(error) => {
                abandon.answered();
                return Err(accounts_error(&error));
            }
        };
        abandon.0 = listener.closer(Some(path.clone()));
        let (code, results) = listener.response(&path).await.map_err(sheet_error)?;
        abandon.answered();
        reply_of(kind, code, results).map_err(|_| LinkError::Malformed(Malformed::Reply).into())
    }
}

/// The `parent_window` argument: no parent is the empty string.
fn wire(window: &ParentWindow) -> String {
    match window {
        ParentWindow::Handle(handle) => handle.clone(),
        // No parent, and a parent this build does not know, is no parent.
        _ => String::new(),
    }
}

/// The `provider_hint` argument: the provider list is the empty string.
fn hint_text(hint: &ProviderHint) -> String {
    match hint {
        ProviderHint::Provider(id) => id.as_str().to_owned(),
        _ => String::new(),
    }
}

fn sheet_error(error: SheetError) -> AccountsError {
    match error {
        SheetError::Bus(error) => accounts_error(&error),
        SheetError::Gone => LinkError::Closed.into(),
        SheetError::Unreadable => LinkError::Malformed(Malformed::Sheet).into(),
        // a failure a later porter-dbus adds
        _ => LinkError::Malformed(Malformed::UnknownFailure).into(),
    }
}

impl Bus {
    async fn manager(&self) -> Result<ManagerProxy<'static>, LinkError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| link_error(&e))
    }

    /// The object of `account`.
    async fn account_proxy(
        &self,
        account: &AccountRef,
    ) -> Result<AccountProxy<'static>, LinkError> {
        let builder = AccountProxy::builder(&self.connection)
            .path(format!("{ACCOUNTS_PATH}/account/{}", account.segment()))
            .map_err(|e| link_error(&e))?;
        self.accounts
            .aim(builder)
            .map_err(|e| link_error(&e))?
            .build()
            .await
            .map_err(|e| link_error(&e))
    }
}

/// The interface whose `State` property a shell is told changed.
const ACCOUNT_INTERFACE: &str = "org.quire.Accounts1.Account";

/// What a `PropertiesChanged` heard is, when it says an account's `State` changed: the account
/// and the state it has now, read from its object. A signal about anything else is not a change;
/// an account that went before it could be read is not one either (its removal is told by
/// `AccountRemoved`).
async fn state_change(
    bus: &Bus,
    message: zbus::Result<zbus::Message>,
) -> Option<Result<AccountChange, Malformed>> {
    let message = message.ok()?;
    let account = AccountRef::of_object_path(message.header().path()?.as_str())?;
    let (interface, changed, invalidated) = message
        .body()
        .deserialize::<(
            String,
            std::collections::HashMap<String, porter_dbus::zvariant::OwnedValue>,
            Vec<String>,
        )>()
        .ok()?;
    let says_state = changed.contains_key("State") || invalidated.iter().any(|n| n == "State");
    if interface != ACCOUNT_INTERFACE || !says_state {
        return None;
    }
    let word = bus.account_proxy(&account).await.ok()?.state().await.ok()?;
    Some(
        from_word::<AccountState>(&word)
            .map(|state| AccountChange::State(account, state))
            .ok_or(Malformed::Row),
    )
}

fn account_ref(path: &str) -> Result<AccountRef, Malformed> {
    AccountRef::of_object_path(path).ok_or(Malformed::Row)
}

impl AccountWatch for Bus {
    async fn needing_reauth(&self) -> Result<Vec<NeedsReauth>, LinkError> {
        self.manager()
            .await?
            .needing_reauth()
            .await
            .map_err(|e| link_error(&e))?
            .into_iter()
            .map(|(path, label)| {
                account_ref(path.as_str())
                    .map(|account| NeedsReauth::new(account, AccountLabel(label)))
                    .map_err(LinkError::Malformed)
            })
            .collect()
    }

    async fn view(&self, account: &AccountRef) -> Result<AccountView, LinkError> {
        let proxy = self.account_proxy(account).await?;
        let read = |e: BusError| link_error(&e);
        let id = proxy.id().await.map_err(read)?;
        let provider = proxy.provider().await.map_err(read)?;
        let label = proxy.label().await.map_err(read)?;
        let state = proxy.state().await.map_err(read)?;
        let reason = proxy.reauth_reason().await.map_err(read)?;
        let malformed = |what| LinkError::Malformed(what);
        Ok(AccountView::new(
            AccountId::parse(&id).map_err(|_| malformed(Malformed::Id))?,
            ProviderId::parse(&provider).map_err(|_| malformed(Malformed::Id))?,
            AccountLabel(label),
            from_word::<AccountState>(&state).ok_or(malformed(Malformed::Row))?,
            match reason.as_str() {
                "" => None,
                word => Some(from_word::<ReauthReason>(word).ok_or(malformed(Malformed::Row))?),
            },
        ))
    }

    async fn reauthenticate_shown(
        &self,
        account: &AccountRef,
        window: &ParentWindow,
    ) -> Result<(), AccountsError> {
        let window = wire(window);
        let proxy = self.account_proxy(account).await?;
        let reply = self
            .sheet(SheetKind::Reauthenticate, |options| async move {
                proxy.reauthenticate(&window, &options).await
            })
            .await?;
        match reply {
            AccountsReply::Reauthenticated => Ok(()),
            AccountsReply::Refused(refusal) => Err(AccountsError::Refused(refusal)),
            _ => Err(LinkError::Malformed(Malformed::Reply).into()),
        }
    }

    async fn watch(&self) -> Result<Changes<AccountChange>, LinkError> {
        let manager = self.manager().await?;
        let added = manager
            .receive_account_added()
            .await
            .map_err(|e| link_error(&e))?
            .map(|signal: AccountAdded| {
                let args = signal.args().map_err(|_| Malformed::Signal)?;
                account_ref(args.account.as_str()).map(AccountChange::Added)
            });
        let removed = manager
            .receive_account_removed()
            .await
            .map_err(|e| link_error(&e))?
            .map(|signal: AccountRemoved| {
                let args = signal.args().map_err(|_| Malformed::Signal)?;
                account_ref(args.account.as_str()).map(AccountChange::Removed)
            });
        // A change of any account's state is a `PropertiesChanged` of its object, sent to the shell
        // alone; the new state is read from the object, which is where the shell may read it.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.DBus.Properties")
            .and_then(|rule| rule.member("PropertiesChanged"))
            .map_err(|e| link_error(&e))?
            .build();
        let bus = self.clone();
        let states = zbus::MessageStream::for_match_rule(rule, &self.connection, None)
            .await
            .map_err(|e| link_error(&e))?
            .filter_map(move |message| {
                let bus = bus.clone();
                async move { state_change(&bus, message).await }
            });
        // Asking is how accountd learns this connection, which it then sends state changes to.
        manager.needing_reauth().await.map_err(|e| link_error(&e))?;
        Ok(Changes::new(select(select(added, removed), states)))
    }
}
