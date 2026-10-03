//! Who is calling: a bus sender's unique name to an [`AppId`].
//!
//! The caller is derived by the transport, never sent. The real table (the executable behind the
//! connection, through a caller table) is the same shape as inferd's `peers` and belongs in
//! `porter-dbus` when the second daemon needs it; here the seam is a trait and the one
//! implementation a host that knows its clients can fill.

use porter_core::AppId;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, PoisonError};

/// Says which app a bus connection is.
pub trait Callers: Send + Sync + 'static {
    /// The app behind the connection `sender` (its unique name), or `None` for nobody known.
    fn app_of(&self, sender: &str) -> impl Future<Output = Option<AppId>> + Send;
}

/// Callers from a map of unique names, for tests and for hosts that know their clients.
#[derive(Debug, Default)]
pub struct TableCallers(Mutex<BTreeMap<String, AppId>>);

impl TableCallers {
    /// Nobody is known.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says that the connection `unique_name` is `app`.
    pub fn introduce(&self, unique_name: &str, app: AppId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(unique_name.to_owned(), app);
    }
}

impl Callers for TableCallers {
    async fn app_of(&self, sender: &str) -> Option<AppId> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sender)
            .cloned()
    }
}

impl<T: Callers> Callers for std::sync::Arc<T> {
    async fn app_of(&self, sender: &str) -> Option<AppId> {
        (**self).app_of(sender).await
    }
}
