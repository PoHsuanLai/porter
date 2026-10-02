//! The label lattice (FIDES): integrity falls and confidentiality rises as data from more
//! sources is combined, and only a witness can reverse that.
//!
//! Frozen as types and signatures. The behaviour (`join`, the constructors, `zip`) is a
//! `todo!()` listed in `FINDINGS.md`; the type-level walls (`Quarantined`, `ReaderKey`) are
//! built.

use crate::ids::ClientName;
use porter_core::{AppName, DataClass, SpaceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Whether the content may steer the companion. Ordered `Untrusted < Trusted`; a join takes
/// the minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    /// Written by someone else (mail, web, files, the screen) or by a model reading them.
    Untrusted,
    /// Typed or chosen by the person, or authored by an app as metadata.
    Trusted,
}

/// Who may see it. `Public < Private(spaces) < Secret`; a join takes the maximum and unions
/// the Spaces, so data from two Spaces may flow only where both may.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Confidentiality {
    /// May go anywhere.
    Public,
    /// Stays inside these Spaces.
    Private(BTreeSet<SpaceId>),
    /// Never leaves the shell's trusted path (passwords, keys).
    Secret,
}

/// Where a value came from: what the confirmation sheet and the reviewer are told.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Source {
    /// Typed or chosen by the person (launcher, field, confirm sheet).
    User,
    /// App-authored metadata (labels, enum fields, ids, counts).
    App(AppName),
    /// Mail content written by others.
    Mail,
    /// A web page.
    Web,
    /// A file's content.
    File,
    /// What is on screen in this app.
    Screen {
        /// The app on screen.
        app: AppName,
    },
    /// The clipboard.
    Clipboard,
    /// Calendar content written by others.
    Calendar,
    /// Contacts content.
    Contacts,
    /// Notes content.
    Notes,
    /// Text a model produced.
    Model(ModelRole),
    /// An external MCP client's arguments.
    Mcp(ClientName),
}

/// Which model role produced a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// The planner.
    Planner,
    /// The quarantined reader.
    Reader,
    /// The action reviewer.
    Reviewer,
    /// The policy writer.
    PolicyWriter,
    /// The computer-use model.
    Cua,
    /// Nightly memory consolidation.
    Consolidator,
}

/// What is known about a value's trust, audience, data class and origin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Label {
    /// Whether it may steer the companion.
    pub integrity: Integrity,
    /// Who may see it.
    pub confidentiality: Confidentiality,
    /// The porter data classes it carries (the routing floor is the strictest).
    pub classes: BTreeSet<DataClass>,
    /// Everything it was derived from.
    pub sources: BTreeSet<Source>,
}

impl Label {
    /// The label of the person's own words.
    pub fn trusted_user() -> Label {
        todo!("Trusted, Public, no classes, sources {{User}}")
    }

    /// The label of content from `source` of class `class`, private to `space`.
    pub fn untrusted(source: Source, class: DataClass, space: SpaceId) -> Label {
        let _ = (source, class, space);
        todo!("Untrusted, Private({{space}}), classes {{class}}, sources {{source}}")
    }

    /// The label of anything derived from both: the only combiner.
    pub fn join(&self, other: &Label) -> Label {
        let _ = other;
        todo!("integrity min, confidentiality max (Private sets union), classes and sources union")
    }
}

/// A value and its label.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Labelled<T> {
    /// The value.
    pub value: T,
    /// Its label.
    pub label: Label,
}

impl<T> Labelled<T> {
    /// `value` with `label`.
    pub fn new(value: T, label: Label) -> Self {
        Self { value, label }
    }

    /// Transforms the value; the label is kept.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Labelled<U> {
        Labelled {
            value: f(self.value),
            label: self.label,
        }
    }

    /// Pairs two values; the label is the join of both.
    pub fn zip<U>(self, other: Labelled<U>) -> Labelled<(T, U)> {
        let _ = other;
        todo!("(value, other.value) labelled with label.join(&other.label)")
    }
}

// The value is the person's data: Debug shows the label and never the value.
impl<T> fmt::Debug for Labelled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Labelled")
            .field("value", &"<redacted>")
            .field("label", &self.label)
            .finish()
    }
}

/// How large a value is, for logs that must not show it.
pub trait Measured {
    /// Its size in bytes or elements.
    fn measure(&self) -> usize;
}

impl Measured for String {
    fn measure(&self) -> usize {
        self.len()
    }
}

impl<T> Measured for Vec<T> {
    fn measure(&self) -> usize {
        self.len()
    }
}

/// Untrusted content no planner may read. Type-level only: the real wall is the process
/// boundary around the reader.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantined<T>(Labelled<T>);

impl<T> Quarantined<T> {
    /// Locks `value` away from planners.
    pub fn new(value: Labelled<T>) -> Self {
        Self(value)
    }

    /// Its label, which is always readable.
    pub fn label(&self) -> &Label {
        &self.0.label
    }

    /// The content, for the reader host that minted the key.
    pub fn open(self, _key: &ReaderKey) -> Labelled<T> {
        self.0
    }
}

// Length and sources only, never the content.
impl<T: Measured> fmt::Debug for Quarantined<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Quarantined")
            .field("len", &self.0.value.measure())
            .field("sources", &self.0.label.sources)
            .finish()
    }
}

/// The right to open a [`Quarantined`] value. Only readerd's host creates one, in the reader
/// process; the planner's crates never call [`ReaderKey::for_reader_host`].
#[derive(Debug)]
pub struct ReaderKey(());

impl ReaderKey {
    /// The key, for the reader host's startup. Calling it anywhere else is a review finding.
    pub fn for_reader_host() -> Self {
        Self(())
    }
}
