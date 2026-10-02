//! The label lattice (FIDES): integrity falls and confidentiality rises as data from more
//! sources is combined, and only a witness can reverse that.
//!
//! Frozen as types and signatures; the behaviour (`join`, the constructors, `zip`) and the
//! type-level walls (`Quarantined`, `ReaderKey`) are built.

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
        Label {
            integrity: Integrity::Trusted,
            confidentiality: Confidentiality::Public,
            classes: BTreeSet::new(),
            sources: BTreeSet::from([Source::User]),
        }
    }

    /// The label of content from `source` of class `class`, private to `space`.
    pub fn untrusted(source: Source, class: DataClass, space: SpaceId) -> Label {
        Label {
            integrity: Integrity::Untrusted,
            confidentiality: Confidentiality::Private(BTreeSet::from([space])),
            classes: BTreeSet::from([class]),
            sources: BTreeSet::from([source]),
        }
    }

    /// The label of anything derived from both: the only combiner.
    pub fn join(&self, other: &Label) -> Label {
        Label {
            integrity: self.integrity.min(other.integrity),
            confidentiality: self.confidentiality.join(&other.confidentiality),
            classes: self.classes.union(&other.classes).cloned().collect(),
            sources: self.sources.union(&other.sources).cloned().collect(),
        }
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
        let label = self.label.join(&other.label);
        Labelled {
            value: (self.value, other.value),
            label,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{ConfirmId, ConfirmReceipt, InputProof, Witness, declassify, endorse};
    use porter_core::{AppName, UnixSeconds};
    use proptest::prelude::*;

    fn space(id: &str) -> SpaceId {
        SpaceId::parse(id).expect("space id")
    }

    fn witness() -> Witness {
        Witness::UserConfirmed(ConfirmReceipt {
            id: ConfirmId::parse("c-1").expect("id"),
            input: InputProof::HardwareSeat,
            at: UnixSeconds(1),
        })
    }

    #[test]
    fn constructors_name_their_rows() {
        let user = Label::trusted_user();
        assert_eq!(user.integrity, Integrity::Trusted);
        assert_eq!(user.confidentiality, Confidentiality::Public);
        assert!(user.classes.is_empty());
        assert_eq!(user.sources, BTreeSet::from([Source::User]));

        let mail = Label::untrusted(Source::Mail, DataClass::Mail, space("work"));
        assert_eq!(mail.integrity, Integrity::Untrusted);
        assert_eq!(
            mail.confidentiality,
            Confidentiality::Private(BTreeSet::from([space("work")]))
        );
        assert_eq!(mail.classes, BTreeSet::from([DataClass::Mail]));
        assert_eq!(mail.sources, BTreeSet::from([Source::Mail]));
    }

    #[test]
    fn join_takes_the_worst_of_each_half() {
        let user = Label::trusted_user();
        let mail = Label::untrusted(Source::Mail, DataClass::Mail, space("work"));
        let web = Label::untrusted(Source::Web, DataClass::Public, space("home"));
        let joined = user.join(&mail);
        assert_eq!(joined.integrity, Integrity::Untrusted);
        assert_eq!(joined.sources, BTreeSet::from([Source::User, Source::Mail]));
        assert_eq!(joined.confidentiality, mail.confidentiality);
        let both = mail.join(&web);
        assert_eq!(
            both.confidentiality,
            Confidentiality::Private(BTreeSet::from([space("home"), space("work")]))
        );
        assert_eq!(
            both.classes,
            BTreeSet::from([DataClass::Mail, DataClass::Public])
        );
    }

    #[test]
    fn join_drops_desktop_beside_a_real_space() {
        let desktop = Label {
            confidentiality: Confidentiality::Private(BTreeSet::from([SpaceId::desktop()])),
            ..Label::trusted_user()
        };
        let work = Label::untrusted(Source::Mail, DataClass::Mail, space("work"));
        assert_eq!(
            desktop.join(&work).confidentiality,
            Confidentiality::Private(BTreeSet::from([space("work")]))
        );
    }

    #[test]
    fn zip_pairs_values_under_the_joined_label() {
        let pair = Labelled::new(1, Label::trusted_user()).zip(Labelled::new(
            "x",
            Label::untrusted(Source::Web, DataClass::Public, space("work")),
        ));
        assert_eq!(pair.value, (1, "x"));
        assert_eq!(pair.label.integrity, Integrity::Untrusted);
        assert_eq!(
            pair.label.sources,
            BTreeSet::from([Source::User, Source::Web])
        );
    }

    #[test]
    fn witnesses_raise_and_lower() {
        let mail = Labelled::new(
            (),
            Label::untrusted(Source::Mail, DataClass::Mail, space("work")),
        );
        let endorsed = endorse(mail, &witness());
        assert_eq!(endorsed.label.integrity, Integrity::Trusted);
        assert!(endorsed.label.sources.contains(&Source::User));
        assert!(endorsed.label.sources.contains(&Source::Mail));
        let opened = declassify(endorsed, Confidentiality::Public, &witness());
        assert_eq!(opened.label.confidentiality, Confidentiality::Public);
        assert_eq!(opened.label.integrity, Integrity::Trusted);
    }

    fn arb_space() -> impl Strategy<Value = SpaceId> {
        prop_oneof![Just("desktop"), Just("work"), Just("home"), Just("lab")].prop_map(|s| space(s))
    }

    fn arb_source() -> impl Strategy<Value = Source> {
        prop_oneof![
            Just(Source::User),
            Just(Source::Mail),
            Just(Source::Web),
            Just(Source::Clipboard),
            Just(Source::App(AppName::parse("org.quire.Mail").expect("app"))),
            Just(Source::Model(ModelRole::Reader)),
        ]
    }

    fn arb_class() -> impl Strategy<Value = DataClass> {
        prop_oneof![
            Just(DataClass::Mail),
            Just(DataClass::Files),
            Just(DataClass::Voice),
            Just(DataClass::Public),
        ]
    }

    fn arb_label() -> impl Strategy<Value = Label> {
        let confidentiality = prop_oneof![
            Just(Confidentiality::Public),
            Just(Confidentiality::Secret),
            proptest::collection::btree_set(arb_space(), 1..4).prop_map(Confidentiality::Private),
        ];
        (
            prop_oneof![Just(Integrity::Untrusted), Just(Integrity::Trusted)],
            confidentiality,
            proptest::collection::btree_set(arb_class(), 0..4),
            proptest::collection::btree_set(arb_source(), 0..4),
        )
            .prop_map(|(integrity, confidentiality, classes, sources)| Label {
                integrity,
                confidentiality,
                classes,
                sources,
            })
    }

    proptest! {
        #[test]
        fn join_is_commutative(a in arb_label(), b in arb_label()) {
            prop_assert_eq!(a.join(&b), b.join(&a));
        }

        #[test]
        fn join_is_associative(a in arb_label(), b in arb_label(), c in arb_label()) {
            prop_assert_eq!(a.join(&b).join(&c), a.join(&b.join(&c)));
        }

        #[test]
        fn join_is_idempotent_once_normalised(a in arb_label()) {
            let once = a.join(&a);
            prop_assert_eq!(once.join(&once), once.clone());
            prop_assert_eq!(once.join(&a), once);
        }

        #[test]
        fn join_never_raises_integrity_or_loses_a_source(a in arb_label(), b in arb_label()) {
            let j = a.join(&b);
            prop_assert!(j.integrity <= a.integrity && j.integrity <= b.integrity);
            prop_assert!(a.sources.is_subset(&j.sources) && b.sources.is_subset(&j.sources));
            prop_assert!(a.classes.is_subset(&j.classes) && b.classes.is_subset(&j.classes));
        }
    }
}
