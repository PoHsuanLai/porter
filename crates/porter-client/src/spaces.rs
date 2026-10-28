//! The desktop-wide Spaces an app's own Space can link to (`org.quire.Spaces1` on accountd).
//!
//! An app keeps its own Spaces (quire's Spaces kit) and stores, per Space, an optional link: the
//! id of a desktop-wide Space from this list. It makes the `SpaceId` it hands memory and consent
//! with `porter_core::SpaceId::for_app`. When a desktop-wide Space is removed, the watch says so
//! and the app falls back to its own Space; that is the app's choice.
//!
//! ```ignore
//! let spaces = porter.spaces();
//! let mut changes = spaces.watch().await?;          // subscribe before reading the list
//! let list = spaces.list().await?;
//! let id = spaces.create(&SpaceName::parse("Work")?, &look).await?;
//! while let Some(change) = changes.next().await { /* (id, SpaceChange) */ }
//! ```

use crate::changes::Changes;
use crate::error::LinkError;
use crate::link::SpacesLink;
use porter_core::{DesktopSpace, DesktopSpaceRecord, SpaceChange, SpaceLook, SpaceName};
use std::sync::Arc;

/// Why a Spaces call failed. More reasons may be added: match with a wildcard. A Space that is
/// not there, and an argument accountd refuses, are both `Link(LinkError::Invalid)`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SpacesError {
    /// The app made too many Spaces within the last minute; try again later.
    #[error("too many new Spaces")]
    TooMany,
    /// The call did not complete.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// accountd's registry of desktop-wide Spaces, as an app sees it.
#[derive(Debug)]
pub struct Spaces<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for Spaces<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: SpacesLink> Spaces<T> {
    /// Every desktop-wide Space, in the order they were made.
    pub async fn list(&self) -> Result<Vec<DesktopSpaceRecord>, SpacesError> {
        self.link.list().await
    }

    /// Makes a desktop-wide Space and answers its id, which a rename keeps. accountd lets an app
    /// make only a few a minute (ten); more is [`SpacesError::TooMany`].
    pub async fn create(
        &self,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> Result<DesktopSpace, SpacesError> {
        self.link.create(name, look).await
    }

    /// Registers a Space whose id the caller minted (the compositor, per workspace); one already
    /// registered is left as it is. Only the compositor, Settings and the shell may.
    pub async fn register(
        &self,
        id: &DesktopSpace,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> Result<(), SpacesError> {
        self.link.register(id, name, look).await
    }

    /// Renames a Space. Only Settings and the shell may.
    pub async fn rename(&self, id: &DesktopSpace, name: &SpaceName) -> Result<(), SpacesError> {
        self.link.rename(id, name).await
    }

    /// Gives a Space a new look. Only Settings and the shell may.
    pub async fn set_look(&self, id: &DesktopSpace, look: &SpaceLook) -> Result<(), SpacesError> {
        self.link.set_look(id, look).await
    }

    /// Removes a Space. Only Settings and the shell may.
    pub async fn remove(&self, id: &DesktopSpace) -> Result<(), SpacesError> {
        self.link.remove(id).await
    }

    /// The changes to the Spaces, from now on: which Space, and what became of it. It subscribes,
    /// then calls `List` once so accountd counts this connection among those it tells.
    pub async fn watch(&self) -> Result<Changes<(DesktopSpace, SpaceChange)>, SpacesError> {
        self.link.watch().await
    }
}
