//! `org.quire.Accounts1.Keys` at `/org/quire/Accounts1`: how an app gets a permit to use the
//! person's AI keys.
//!
//! `RequestKey(class, usage, parent_window, options)` returns a Request object
//! (`org.quire.Accounts1.Request`, under `/org/quire/Accounts1/request`) and answers on it when
//! the sheet ends, as a portal does: the answer carries the permit's id. The permit is held by
//! the app the caller is, never by a name it sends. An app that already holds a permit (Always,
//! session or not yet spent) is answered with it at once and no sheet is drawn; after a Don't
//! Allow it is refused as denied, again with no sheet.
//!
//! Who may call: any app, Settings and the shell. Every other role (an assistant of the desktop,
//! cuad, the terminal, the compositor, the agent launcher, the AI broker, and any role a newer
//! porter adds) is refused: the match fails closed. Settings lists and revokes permits through its
//! settings rows, so there is no method for it here. Adding a key is `Manager.AddAccount`.

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::state::{Core, Standing, window};
use porter_callers::CallerRole;
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_dbus::accounts::KeysServer;
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::{Call, Details};
use porter_match::llm_need;
use porter_wire::AccountsRequest;
use std::sync::Arc;
use zbus::zvariant::OwnedObjectPath;

/// The Keys object.
#[derive(Debug)]
pub(crate) struct KeysObject<H, C>(Arc<Core<H, C>>);

impl<H, C> KeysObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RefusedError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| RefusedError::invalid(format!("`{text}`: {e}")))
}

impl<H: Host, C: Callers> KeysServer for KeysObject<H, C> {
    type Error = RefusedError;

    async fn request_key(
        &self,
        Call { header, .. }: Call<'_>,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let caller = self.0.identify(&header, Standing::Any).await?;
        match caller.role {
            CallerRole::App | CallerRole::Settings | CallerRole::SheetHost => {}
            _ => {
                return Err(RefusedError::access_denied(
                    "this caller may not ask for the person's AI keys",
                ));
            }
        }
        let class: DataClass = slug(&class)?;
        let usage: Usage = slug(&usage)?;
        let request = AccountsRequest::Choose {
            need: llm_need(),
            class,
            usage,
            window: window(&parent_window),
            account: None,
        };
        self.0
            .sheet(&header, SheetKind::Choose, &options, request)
            .await
    }
}
