//! Spaces as consent, memory and labels see them: a stable id, and a scope that is one Space or
//! any.
//!
//! Spaces are per app, but linkable (owner decision 2026-10-09). Each app keeps its own list of
//! Spaces (quire's Spaces kit numbers them, `ds_style::space::list::SpaceId(u64)`), and any of
//! them may be linked to a desktop-wide Space, whose registry accountd keeps
//! (`org.quire.Spaces1`). Only linked Spaces share agent memory and consent across apps; an
//! unlinked Space is its app's alone. porter owns the ids; quire's kit stores only an opaque
//! link per Space, which is a desktop-wide Space's id.
//!
//! One [`SpaceId`] names any of the three kinds ([`SpaceKind`]), as text:
//!
//! - `desktop`: outside any Space (reserved);
//! - a bare slug (`work`, `space-3`): a desktop-wide Space ([`DesktopSpace`]). Every id stored
//!   before the kinds existed is a slug, so it reads as a desktop-wide Space with no migration;
//! - `app:<app name>:<n>`: Space `n` of one app, unlinked. A slug never holds a `:` and an app
//!   name never does either (reverse DNS, `[A-Za-z0-9_-]` elements), so the form cannot collide
//!   with a slug and splits back unambiguously; `n` is written in canonical decimal.
//!
//! An app makes its ids with [`SpaceId::for_app`] from its own number and the link its kit
//! stores.

use crate::app_id::AppName;
use crate::error::CoreError;
use crate::id::well_formed;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What an app-scoped id starts with.
const APP_PREFIX: &str = "app:";

/// A Space's stable id: `desktop`, a desktop-wide Space's slug, or `app:<app name>:<n>`
/// (module docs). Its serde form is that text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SpaceId(String);

/// What a [`SpaceId`] names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SpaceKind {
    /// Outside any Space (`desktop`).
    Outside,
    /// A desktop-wide Space, shared by every app whose Space links to it.
    Linked(DesktopSpace),
    /// One app's own Space, linked to nothing: that app's alone.
    App {
        /// The app whose Space it is.
        app: AppName,
        /// The app's own number for it.
        local: LocalSpace,
    },
}

/// An app's own number for one of its Spaces (quire's kit `SpaceId(u64)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocalSpace(pub u64);

/// A desktop-wide Space's id: a slug of the id grammar, never `desktop`. On casement the
/// compositor mints it for each workspace (`s-` and 32 lowercase hex digits), sends it as the
/// ext-workspace id, keeps it across restarts and registers it with accountd's
/// `org.quire.Spaces1`; sill and apps read it from the workspace protocol. On a compositor
/// without stable workspace ids, accountd mints it when the Space is made. Never derived from
/// the name, so a rename keeps it; an id stored before Spaces were per app is kept as it is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DesktopSpace(String);

impl DesktopSpace {
    /// The id written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        match well_formed(text) && text != SpaceId::DESKTOP {
            true => Ok(Self(text.to_owned())),
            false => Err(CoreError::MalformedId {
                what: "desktop space id",
                text: text.to_owned(),
            }),
        }
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DesktopSpace {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<DesktopSpace> for String {
    fn from(id: DesktopSpace) -> String {
        id.0
    }
}

impl fmt::Display for DesktopSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The app name and number of an `app:<app name>:<n>` text, when it is one.
fn app_parts(text: &str) -> Option<(AppName, LocalSpace)> {
    let (app, n) = text.strip_prefix(APP_PREFIX)?.rsplit_once(':')?;
    let local: u64 = n.parse().ok()?;
    // Canonical decimal only, so one Space has one text: no sign, no leading zero.
    (local.to_string() == n).then_some(())?;
    Some((AppName::parse(app).ok()?, LocalSpace(local)))
}

impl SpaceId {
    /// The reserved id of "outside any Space".
    pub const DESKTOP: &'static str = "desktop";

    /// The reserved `desktop` Space.
    pub fn desktop() -> Self {
        Self(Self::DESKTOP.to_owned())
    }

    /// The id written as `text`, or why it is not one: `desktop`, a slug, or
    /// `app:<app name>:<n>`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        match well_formed(text) || app_parts(text).is_some() {
            true => Ok(Self(text.to_owned())),
            false => Err(CoreError::MalformedId {
                what: "space id",
                text: text.to_owned(),
            }),
        }
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id of a desktop-wide Space.
    pub fn linked(space: &DesktopSpace) -> Self {
        Self(space.0.clone())
    }

    /// The id of `app`'s own Space `local`, linked to nothing.
    pub fn app(app: &AppName, local: LocalSpace) -> Self {
        Self(format!("{APP_PREFIX}{app}:{}", local.0))
    }

    /// The id an app uses for its Space `local`, from the link its Spaces kit stores: a link
    /// names the desktop-wide Space (and a link that is not one is an error, never quietly the
    /// app's own Space); no link is the app's own Space.
    pub fn for_app(
        app: &AppName,
        local: LocalSpace,
        link: Option<&str>,
    ) -> Result<SpaceId, CoreError> {
        match link {
            Some(link) => DesktopSpace::parse(link).map(|space| Self::linked(&space)),
            None => Ok(Self::app(app, local)),
        }
    }

    /// What the id names.
    pub fn kind(&self) -> SpaceKind {
        if self.0 == Self::DESKTOP {
            return SpaceKind::Outside;
        }
        match app_parts(&self.0) {
            Some((app, local)) => SpaceKind::App { app, local },
            // A parsed id is a slug or the app form, so what is not the app form is a slug.
            None => SpaceKind::Linked(DesktopSpace(self.0.clone())),
        }
    }

    /// The app whose own Space this is; none for `desktop` and desktop-wide Spaces.
    pub fn owner(&self) -> Option<AppName> {
        match self.kind() {
            SpaceKind::App { app, .. } => Some(app),
            SpaceKind::Outside | SpaceKind::Linked(_) => None,
        }
    }
}

impl TryFrom<String> for SpaceId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<SpaceId> for String {
    fn from(id: SpaceId) -> String {
        id.0
    }
}

impl fmt::Display for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which Spaces a decision covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum SpaceScope {
    /// Every Space (and outside them).
    Any,
    /// This Space only.
    Only(SpaceId),
}

impl SpaceScope {
    /// Whether `app` may hold a decision over this scope: any app for `Any`, `desktop` and a
    /// desktop-wide Space; only its owner for an app's own Space. `app` is the caller's verified
    /// name (the bus's, the socket's), never one read from the id's text or a request.
    pub fn open_to(&self, app: &AppName) -> bool {
        match self {
            SpaceScope::Any => true,
            SpaceScope::Only(space) => space.owner().is_none_or(|owner| owner == *app),
        }
    }
}

impl fmt::Display for SpaceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpaceScope::Any => f.write_str("any"),
            SpaceScope::Only(space) => write!(f, "only {space}"),
        }
    }
}

#[cfg(test)]
mod tests;
