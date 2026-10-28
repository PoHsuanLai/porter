//! The sheet methods (`AddAccount`, `Reauthenticate`) over porter-sheets' Request objects.
//!
//! A sheet method returns the path of a Request object at once and runs the service call in a
//! task; the answer is the object's `Response` signal, sent to the caller alone, and the object
//! goes when it has been sent. The objects themselves (portal-shaped: `Close`, `Response`, the
//! `handle_token` option, the caller leaving the bus) are `porter_sheets::RequestObjects`, which
//! inferd and syncd serve the same way under their own roots; what stays here is accountd's
//! own rule, that an app has one sheet of each kind open at a time ([`OpenSheets`](crate::open_sheets::OpenSheets)), and which
//! service call each method runs.

use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::state::{Core, Standing};
use porter_dbus::Details;
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::callers::CallerRole;
use porter_wire::AccountsRequest;
use std::pin::Pin;
use std::sync::Arc;
use zbus::export::futures_core::Stream;
use zbus::fdo::NameOwnerChangedStream;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

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
