//! The store: what builds a [`ChunkedCredential`] for each entry.

use crate::credential::{ChunkedCredential, is_part};
use crate::{ChunkError, Limit};
use keyring_core::api::{CredentialPersistence, CredentialStoreApi};
use keyring_core::{CredentialStore, Entry, Result};
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A store that keeps a value too long for one entry of `S` in several of them.
///
/// Generic over the store it wraps: an `Arc<CredentialStore>` (the default, any store chosen at
/// run time) or a concrete one, such as keyring-core's mock in a test.
pub struct ChunkedStore<S: ?Sized = CredentialStore> {
    pub(crate) inner: Arc<S>,
    pub(crate) marker: Arc<str>,
    pub(crate) limit: Limit,
}

impl<S: ?Sized> Clone for ChunkedStore<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            marker: self.marker.clone(),
            limit: self.limit,
        }
    }
}

impl<S: ?Sized> fmt::Debug for ChunkedStore<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunkedStore")
            .field("marker", &self.marker)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl<S: CredentialStoreApi + Send + Sync + ?Sized + 'static> ChunkedStore<S> {
    /// Wraps `inner`, cutting any value over `limit`. A head entry holds `marker` and the part
    /// count; the marker is what tells a head from a value, so it is something no value the
    /// caller stores whole begins with (a value that does is stored in parts all the same).
    ///
    /// An empty marker is [`ChunkError::EmptyMarker`].
    pub fn new(inner: Arc<S>, marker: &str, limit: Limit) -> std::result::Result<Self, ChunkError> {
        if marker.is_empty() {
            return Err(ChunkError::EmptyMarker);
        }
        Ok(Self {
            inner,
            marker: marker.into(),
            limit,
        })
    }

    /// The store this one wraps.
    pub fn inner(&self) -> &Arc<S> {
        &self.inner
    }

    /// What a head entry holds before the part count.
    pub fn marker(&self) -> &str {
        &self.marker
    }

    /// How long one entry's value may be.
    pub fn limit(&self) -> Limit {
        self.limit
    }
}

impl<S: CredentialStoreApi + Send + Sync + ?Sized + 'static> CredentialStoreApi
    for ChunkedStore<S>
{
    fn vendor(&self) -> String {
        format!("keyring-chunked over {}", self.inner.vendor())
    }

    fn id(&self) -> String {
        format!(
            "keyring-chunked {} ({}) over {}",
            env!("CARGO_PKG_VERSION"),
            self.marker,
            self.inner.id()
        )
    }

    /// An entry for `service` and `user`; the modifiers are passed to every entry of the
    /// wrapped store it uses. Nothing is stored until a value is set.
    fn build(
        &self,
        service: &str,
        user: &str,
        modifiers: Option<&HashMap<&str, &str>>,
    ) -> Result<Entry> {
        // Building the head now reports a service, user or modifier the wrapped store refuses.
        self.inner.build(service, user, modifiers)?;
        let credential = ChunkedCredential::new(self.clone(), service, user, modifiers);
        Ok(Entry::new_with_credential(Arc::new(credential)))
    }

    /// The wrapped store's search, without the parts: each entry found that is not a part comes
    /// back as an entry of this store. One with no service and user is left out.
    fn search(&self, spec: &HashMap<&str, &str>) -> Result<Vec<Entry>> {
        let mut found = Vec::new();
        for entry in self.inner.search(spec)? {
            let Some((service, user)) = entry.get_specifiers() else {
                continue;
            };
            if !is_part(&user) {
                found.push(self.build(&service, &user, None)?);
            }
        }
        Ok(found)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn persistence(&self) -> CredentialPersistence {
        self.inner.persistence()
    }
}
