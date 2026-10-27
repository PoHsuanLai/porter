//! The sheet methods (`AddAccount`, `Reauthenticate`) over porter-sheets' Request objects.
//!
//! A sheet method returns the path of a Request object at once and runs the service call in a
//! task; the answer is the object's `Response` signal, sent to the caller alone, and the object
//! goes when it has been sent. The objects themselves (portal-shaped: `Close`, `Response`, the
//! `handle_token` option, the caller leaving the bus) are `porter_sheets::RequestObjects`, which
//! inferd and syncd serve the same way under their own roots; what stays here is accountd's
//! own rule, that an app has one sheet of each kind open at a time ([`OpenSheets`]), and which
//! service call each method runs.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing};
use crate::errors::RefusedError;
use porter_core::{AccountsRequest, AppId};
use porter_dbus::Details;
use porter_dbus::callers::CallerRole;
use porter_dbus::sheet::SheetKind;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use zbus::export::futures_core::Stream;
use zbus::fdo::NameOwnerChangedStream;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

/// The sheets open now, by the app that asked and the kind of sheet: an app has one of each
/// kind at a time, so it cannot stack sheets on the person (rel-11). Keyed by the app, not the
/// connection, so a second connection of the same app is the same asker.
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenSheets(Arc<Mutex<HashSet<(AppId, SheetKind)>>>);

/// One open sheet, held by its task; dropping it (the sheet answered, was closed, or its caller
/// left) frees the app to ask again.
#[derive(Debug)]
pub(crate) struct SheetClaim {
    open: OpenSheets,
    key: (AppId, SheetKind),
}

impl OpenSheets {
    /// A claim on a `kind` sheet for `app`, or `LimitsExceeded` while it has one open: no
    /// `Refusal` says "you have one open already" (each is about the account or the person), so
    /// the method answers the bus's own error and no sheet or Request object is made.
    pub(crate) fn claim(&self, app: &AppId, kind: SheetKind) -> Result<SheetClaim, RefusedError> {
        let key = (app.clone(), kind);
        match self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone())
        {
            true => Ok(SheetClaim {
                open: self.clone(),
                key,
            }),
            false => Err(RefusedError::busy(
                "this app already has a sheet of this kind open",
            )),
        }
    }
}

impl Drop for SheetClaim {
    fn drop(&mut self) {
        self.open
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Starts a sheet for the caller of `header`: registers its Request object and runs
    /// `request` for the app behind the sender. Returns the object's path.
    pub(crate) async fn sheet(
        self: &Arc<Self>,
        header: &Header<'_>,
        kind: SheetKind,
        options: &Details,
        request: AccountsRequest,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let caller = self.identify(header, Standing::Acting).await?;
        let app = caller.app.clone();
        let shell = matches!(caller.role, CallerRole::SheetHost | CallerRole::Settings);
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_owned();
        let claim = self.open_sheets.claim(&app, kind)?;
        let core = Arc::clone(self);
        let run = async move {
            // Held until the sheet ends, however it ends (answered, closed, its caller gone).
            let _claim = claim;
            let reply = match request {
                // The shell and Settings sign any account in again; the service refuses any
                // other app, which holds no grant for an account.
                AccountsRequest::Reauthenticate { account, window } if shell => {
                    core.host.reauthenticate_any(&app, &account, window).await
                }
                request => core.host.handle(&app, request).await,
            };
            core.publish().await;
            reply
        };
        let path = self
            .requests
            .path(&sender, options)
            .map_err(RefusedError::from_request)?;
        self.requests
            .start(sender, path, kind, run)
            .await
            .map_err(RefusedError::from_request)
    }
}

/// Resolves when the caller's connection leaves the bus.
pub(crate) async fn departed(mut owners: NameOwnerChangedStream) {
    loop {
        let next = std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await;
        match next {
            Some(signal) => {
                let gone = signal
                    .args()
                    .map(|args| args.new_owner().is_none())
                    .unwrap_or(false);
                if gone {
                    return;
                }
            }
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{AppName, Isolation};

    fn app(name: &str) -> AppId {
        AppId {
            name: AppName::parse(name).expect("app name"),
            isolation: Isolation::Flatpak,
        }
    }

    #[test]
    fn an_app_has_one_open_sheet_of_each_kind_until_it_ends() {
        let open = OpenSheets::default();
        let mail = app("org.quire.Mail");
        let first = open.claim(&mail, SheetKind::AddAccount).expect("first");
        let again = open
            .claim(&mail, SheetKind::AddAccount)
            .expect_err("a second while the first is open");
        assert_eq!(
            again.error_name(),
            "org.freedesktop.DBus.Error.LimitsExceeded"
        );
        // Another kind, or another app, is its own.
        let _reauth = open
            .claim(&mail, SheetKind::Reauthenticate)
            .expect("another kind");
        let _other = open
            .claim(&app("org.quire.Photos"), SheetKind::AddAccount)
            .expect("another app");
        drop(first);
        let _next = open
            .claim(&mail, SheetKind::AddAccount)
            .expect("free once the first ended");
    }
}
