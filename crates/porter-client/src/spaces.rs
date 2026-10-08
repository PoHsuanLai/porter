//! The desktop-wide Spaces an app's own Space can link to (`org.quire.Spaces1` on accountd).
//!
//! An app keeps its own Spaces (quire's Spaces kit) and stores, per Space, an optional link: the
//! id of a desktop-wide Space from this list. It makes the `SpaceId` it hands memory and consent
//! with `porter_core::SpaceId::for_app`. When a desktop-wide Space is removed, [`SpaceChanges`]
//! says so and the app falls back to its own Space; that is the app's choice.
//!
//! ```ignore
//! let spaces = transport.spaces().await?;
//! let mut changes = spaces.watch().await?;          // subscribe before reading the list
//! let list = spaces.list().await?;
//! let id = spaces.create(&SpaceName::parse("Work")?, &look).await?;
//! while let Some(change) = changes.next().await { /* (id, SpaceChange) */ }
//! ```

use porter_core::{
    CoreError, DesktopSpace, DesktopSpaceRecord, SpaceChange, SpaceLook, SpaceName, UnixSeconds,
};
use porter_dbus::{
    BusConnection, BusError, BusFailure, BusStream, Details, SPACE_KEY_CREATED, SPACE_KEY_LOOK,
    SPACE_KEY_NAME, SpaceChanged, SpaceChangedStream, SpacesProxy, classify, is_invalid_args,
    is_limits_exceeded,
};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Why a Spaces call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpacesError {
    /// The app made too many Spaces within the last minute; try again later.
    #[error("too many new Spaces at once")]
    TooMany,
    /// accountd refused an argument (a name or look out of bounds, a Space that is not there).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// accountd does not know this caller, or the call needs Settings or the shell.
    #[error("refused by accountd: {0}")]
    Denied(String),
    /// accountd is not there.
    #[error("no account service reachable")]
    Unreachable,
    /// accountd sent something that is not porter's protocol, or could not save.
    #[error("malformed: {0}")]
    Malformed(String),
}

impl From<BusError> for SpacesError {
    fn from(error: BusError) -> Self {
        if is_limits_exceeded(&error) {
            return SpacesError::TooMany;
        }
        if is_invalid_args(&error) {
            return SpacesError::Invalid(error.to_string());
        }
        match classify(&error) {
            BusFailure::NoDaemon => SpacesError::Unreachable,
            BusFailure::Denied(why) => SpacesError::Denied(why),
            BusFailure::Other(why) => SpacesError::Malformed(why),
        }
    }
}

impl From<CoreError> for SpacesError {
    fn from(error: CoreError) -> Self {
        SpacesError::Malformed(error.to_string())
    }
}

/// accountd's registry of desktop-wide Spaces, as an app sees it.
#[derive(Debug, Clone)]
pub struct Spaces {
    proxy: SpacesProxy<'static>,
}

fn text(details: &Details, key: &str) -> Result<String, SpacesError> {
    details
        .get(key)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| String::try_from(v).ok())
        .ok_or_else(|| SpacesError::Malformed(format!("a Space without `{key}`")))
}

fn record(id: &str, details: &Details) -> Result<DesktopSpaceRecord, SpacesError> {
    let created = details
        .get(SPACE_KEY_CREATED)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| SpacesError::Malformed("a Space without `created`".into()))?;
    Ok(DesktopSpaceRecord {
        id: DesktopSpace::parse(id)?,
        name: SpaceName::parse(&text(details, SPACE_KEY_NAME)?)?,
        look: SpaceLook::parse(&text(details, SPACE_KEY_LOOK)?)?,
        created: UnixSeconds(created),
    })
}

impl Spaces {
    /// The registry over `connection`.
    pub async fn connect(connection: &BusConnection) -> Result<Self, SpacesError> {
        Ok(Self {
            proxy: SpacesProxy::new(connection).await?,
        })
    }

    /// Every desktop-wide Space, in the order they were made.
    pub async fn list(&self) -> Result<Vec<DesktopSpaceRecord>, SpacesError> {
        self.proxy
            .list()
            .await?
            .iter()
            .map(|(id, details)| record(id, details))
            .collect()
    }

    /// Makes a desktop-wide Space and answers its id, which a rename keeps. accountd lets an app
    /// make only a few a minute (ten); more is [`SpacesError::TooMany`].
    pub async fn create(
        &self,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> Result<DesktopSpace, SpacesError> {
        let id = self.proxy.create(name.as_str(), look.as_str()).await?;
        Ok(DesktopSpace::parse(&id)?)
    }

    /// The changes to the Spaces, from now on. It subscribes, then calls `List` once so accountd
    /// counts this connection among those it tells.
    pub async fn watch(&self) -> Result<SpaceChanges, SpacesError> {
        let stream = self.proxy.receive_changed().await?;
        self.proxy.list().await?;
        Ok(SpaceChanges(stream))
    }
}

/// The stream of changes: which Space, and what became of it. An item is an error when
/// accountd sent one that is not well formed.
#[derive(Debug)]
pub struct SpaceChanges(SpaceChangedStream);

fn change(signal: SpaceChanged) -> Result<(DesktopSpace, SpaceChange), SpacesError> {
    let args = signal.args()?;
    let what = SpaceChange::from_word(args.what)
        .ok_or_else(|| SpacesError::Malformed(format!("a change `{}`", args.what)))?;
    Ok((DesktopSpace::parse(args.id)?, what))
}

impl BusStream for SpaceChanges {
    type Item = Result<(DesktopSpace, SpaceChange), SpacesError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map(|next| next.map(change))
    }
}
