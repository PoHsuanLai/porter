//! accountd's `Keys` over the bus.

use crate::Bus;
use crate::failure::{ai_keys_error, link_error};
use porter_client::{AiKeysError, AiKeysLink, LinkError, Malformed};
use porter_core::consent::Usage;
use porter_core::{DataClass, GrantId};
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{Abandon, KeysProxy, ServiceRoot, Sheet, SheetError};
use porter_ids::word::word as word_of;
use porter_wire::{AccountsReply, ParentWindow};

fn malformed(what: Malformed) -> AiKeysError {
    AiKeysError::Link(LinkError::Malformed(what))
}

/// The `parent_window` argument: no parent is the empty string.
fn wire(window: &ParentWindow) -> String {
    match window {
        ParentWindow::Handle(handle) => handle.clone(),
        _ => String::new(),
    }
}

fn sheet_error(error: SheetError) -> AiKeysError {
    match error {
        SheetError::Bus(error) => ai_keys_error(&error),
        SheetError::Gone => AiKeysError::Link(LinkError::Unreachable),
        SheetError::Unreadable => malformed(Malformed::Sheet),
        // a failure a later porter-dbus adds
        _ => malformed(Malformed::UnknownFailure),
    }
}

impl AiKeysLink for Bus {
    async fn request_key(
        &self,
        class: DataClass,
        usage: Usage,
        window: &ParentWindow,
    ) -> Result<GrantId, AiKeysError> {
        let proxy: KeysProxy<'static> = self
            .accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| AiKeysError::Link(link_error(&e)))?;
        let (class, usage, window) = (word_of(&class), word_of(&usage), wire(window));
        let mut listener = Sheet::subscribe_on(&self.connection, ServiceRoot::Accounts)
            .await
            .map_err(|e| ai_keys_error(&e))?;
        // Armed before the call: a dropped call may already have made the sheet.
        let mut abandon = Abandon(listener.closer(None));
        let path = match proxy
            .request_key(class, usage, &window, &listener.options())
            .await
        {
            Ok(path) => path,
            Err(error) => {
                abandon.answered();
                return Err(ai_keys_error(&error));
            }
        };
        abandon.0 = listener.closer(Some(path.clone()));
        let (code, results) = listener.response(&path).await.map_err(sheet_error)?;
        abandon.answered();
        match reply_of(SheetKind::Choose, code, results).map_err(|_| malformed(Malformed::Reply))? {
            // The answer is the chosen key; its `grant` is the permit's id.
            AccountsReply::Chosen(candidate) => Ok(candidate.grant),
            AccountsReply::Refused(refusal) => Err(AiKeysError::Refused(refusal)),
            _ => Err(malformed(Malformed::Reply)),
        }
    }
}
