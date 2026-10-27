//! The sheets open now: an app has one sheet of each kind open at a time.

use crate::errors::RefusedError;
use porter_core::AppId;
use porter_dbus::sheet::SheetKind;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

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
