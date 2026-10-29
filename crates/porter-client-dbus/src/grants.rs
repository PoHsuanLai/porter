//! `Grants1` over the bus: who may use which account, and the relays a grant opens. The caller is
//! the connection's. `RequestGrant` returns a Request object whose `Response` carries the
//! candidate the person granted (the same wait as the sheet methods of the accounts domain), and
//! a relayed stream arrives as a descriptor: a Unix stream socket whose other end accountd's relay
//! holds.

use crate::Bus;
use crate::failure::{grants_error, link_error};
use porter_capability::CapabilityKind;
use porter_client::{
    AccountsError, AuthenticatedStream, ConsentOffer, GrantsError, GrantsLink, LinkError,
    ListedAccount, Malformed,
};
use porter_core::consent::{Availability, Grant, Usage};
use porter_core::{Account, AccountId, Candidate, DataClass, EndpointUrl, GrantId, ProviderId};
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::accounts::{ACCOUNT_OPTION, GrantsProxy, candidate_from_dbus, from_text, to_text};
use porter_ids::word::{from_word, word};
use porter_match::Need;
use porter_wire::{AccountsReply, ParentWindow};
use zbus::zvariant::{OwnedValue, Value};

impl Bus {
    async fn grants_proxy(&self) -> Result<GrantsProxy<'static>, GrantsError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| GrantsError::Link(link_error(&e)))
    }
}

fn malformed(what: Malformed) -> GrantsError {
    GrantsError::Link(LinkError::Malformed(what))
}

/// The sheet wait's failure as the grants domain's.
fn of_sheet(error: AccountsError) -> GrantsError {
    match error {
        AccountsError::Refused(refusal) => GrantsError::Refused(refusal),
        AccountsError::Busy => GrantsError::Busy,
        AccountsError::Link(link) => GrantsError::Link(link),
        // A reason an add-account sheet gives and a grant sheet does not.
        _ => malformed(Malformed::Reply),
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

impl GrantsLink for Bus {
    async fn candidates(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Vec<Candidate>, GrantsError> {
        self.grants_proxy()
            .await?
            .candidates(&to_text(need), word(&class), word(&usage))
            .await
            .map_err(|e| grants_error(&e))?
            .into_iter()
            .map(|row| candidate_from_dbus(row).map_err(|_| malformed(Malformed::Row)))
            .collect()
    }

    async fn availability(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Availability, GrantsError> {
        let text = self
            .grants_proxy()
            .await?
            .availability(&to_text(need), word(&class), word(&usage))
            .await
            .map_err(|e| grants_error(&e))?;
        from_word(&text).ok_or_else(|| malformed(Malformed::Reply))
    }

    async fn grants(&self) -> Result<Vec<Grant>, GrantsError> {
        let text = self
            .grants_proxy()
            .await?
            .grants()
            .await
            .map_err(|e| grants_error(&e))?;
        from_text(&text).map_err(|_| malformed(Malformed::Row))
    }

    async fn request_grant(
        &self,
        offer: &ConsentOffer,
        window: &ParentWindow,
    ) -> Result<Candidate, GrantsError> {
        let proxy = self.grants_proxy().await?;
        let (need, window) = (to_text(&offer.need), wire(window));
        let (class, usage) = (word(&offer.class), word(&offer.usage));
        let account = offer.account.as_ref().map(|id| id.as_str().to_owned());
        let reply = self
            .sheet(SheetKind::Choose, |mut options| async move {
                if let Some(account) = account {
                    let named = OwnedValue::try_from(Value::from(account))?;
                    options.insert(ACCOUNT_OPTION.to_owned(), named);
                }
                proxy
                    .request_grant(&need, class, usage, &window, &options)
                    .await
            })
            .await
            .map_err(of_sheet)?;
        match reply {
            AccountsReply::Chosen(candidate) => Ok(candidate),
            AccountsReply::Refused(refusal) => Err(GrantsError::Refused(refusal)),
            _ => Err(malformed(Malformed::Reply)),
        }
    }

    async fn revoke(&self, grant: &GrantId) -> Result<(), GrantsError> {
        self.grants_proxy()
            .await?
            .revoke(grant.as_str())
            .await
            .map_err(|e| grants_error(&e))
    }

    async fn accounts(&self) -> Result<Vec<ListedAccount>, GrantsError> {
        let text = self
            .grants_proxy()
            .await?
            .accounts()
            .await
            .map_err(|e| grants_error(&e))?;
        let rows: Vec<(Account, String)> =
            from_text(&text).map_err(|_| malformed(Malformed::Row))?;
        Ok(rows
            .into_iter()
            .map(|(account, label)| {
                ListedAccount::new(account, (!label.is_empty()).then_some(label))
            })
            .collect())
    }

    async fn extend_access(
        &self,
        account: &AccountId,
        kinds: &[CapabilityKind],
        window: &ParentWindow,
    ) -> Result<(), GrantsError> {
        let words: Vec<&str> = kinds.iter().map(word).collect();
        self.grants_proxy()
            .await?
            .extend_access(account.as_str(), &words, &wire(window))
            .await
            .map_err(|e| grants_error(&e))
    }

    async fn asks_on_use(
        &self,
        provider: &ProviderId,
        kind: CapabilityKind,
    ) -> Result<bool, GrantsError> {
        self.grants_proxy()
            .await?
            .asks_on_use(provider.as_str(), word(&kind))
            .await
            .map_err(|e| grants_error(&e))
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<AuthenticatedStream, GrantsError> {
        let fd = self
            .grants_proxy()
            .await?
            .open_authenticated(grant.as_str(), endpoint.as_str())
            .await
            .map_err(|e| grants_error(&e))?;
        Ok(AuthenticatedStream::Fd(std::os::fd::OwnedFd::from(fd)))
    }

    async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<AuthenticatedStream, GrantsError> {
        let fd = self
            .grants_proxy()
            .await?
            .open_linked(grant.as_str(), origin.as_str())
            .await
            .map_err(|e| grants_error(&e))?;
        Ok(AuthenticatedStream::Fd(std::os::fd::OwnedFd::from(fd)))
    }
}
