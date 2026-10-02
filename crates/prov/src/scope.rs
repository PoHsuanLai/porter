//! How the `desktop` scope meets a Space: the join rule for confidentiality, where data may
//! flow, and what the desktop scope admits.
//!
//! The reserved `desktop` Space holds only facts the person stated themselves (preferences,
//! names, style; QUESTIONS Q9). It is a sub-scope of every Space: reading a `desktop` fact into
//! a `work` task must not widen the task's confidentiality to `{desktop, work}`, because that
//! would stop the task from ever flowing anywhere `work` data may flow. So in a join the
//! `desktop` member disappears whenever another Space is present.

use crate::label::{Confidentiality, Integrity, Label, Source};
use porter_core::SpaceId;
use std::collections::BTreeSet;

/// Whether data with some confidentiality may go into a Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flow {
    /// It may.
    Allowed,
    /// It may not.
    Denied,
}

/// Whether the desktop scope may hold a labelled value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesktopVerdict {
    /// The person said it, in the clear, for every Space.
    Admit,
    /// It is not secret-free.
    Secret,
    /// It is untrusted: something else wrote it, or a model derived it from that.
    Untrusted,
    /// Trusted, but not only the person's own words (an app's metadata, a model's inference).
    NotUserStated,
    /// It is private to a real Space, and the desktop scope would show it to all of them.
    SpaceBound,
}

/// `spaces` without `desktop` when any other Space is present.
fn normalise(spaces: BTreeSet<SpaceId>) -> BTreeSet<SpaceId> {
    let desktop = SpaceId::desktop();
    if spaces.len() > 1 {
        spaces.into_iter().filter(|s| s != &desktop).collect()
    } else {
        spaces
    }
}

impl Confidentiality {
    /// The confidentiality of anything derived from both: `Secret` if either is, else the
    /// other when one is `Public`, else the union of the Spaces with the `desktop` member
    /// dropped when another Space is present. Commutative, associative and idempotent on
    /// normalised values; a non-normalised `Private` is normalised on the way through.
    pub fn join(&self, other: &Confidentiality) -> Confidentiality {
        match (self, other) {
            (Confidentiality::Secret, _) | (_, Confidentiality::Secret) => Confidentiality::Secret,
            (Confidentiality::Public, Confidentiality::Public) => Confidentiality::Public,
            (Confidentiality::Public, Confidentiality::Private(s))
            | (Confidentiality::Private(s), Confidentiality::Public) => {
                Confidentiality::Private(normalise(s.clone()))
            }
            (Confidentiality::Private(a), Confidentiality::Private(b)) => {
                Confidentiality::Private(normalise(a.union(b).cloned().collect()))
            }
        }
    }

    /// Whether data this confidential may go into `space`: `Public` anywhere; `Private` where
    /// every named Space is `space` or `desktop`; `Secret` nowhere.
    pub fn flow_to(&self, space: &SpaceId) -> Flow {
        let desktop = SpaceId::desktop();
        match self {
            Confidentiality::Public => Flow::Allowed,
            Confidentiality::Private(spaces)
                if spaces.iter().all(|s| s == space || s == &desktop) =>
            {
                Flow::Allowed
            }
            Confidentiality::Private(_) | Confidentiality::Secret => Flow::Denied,
        }
    }
}

/// May the desktop scope hold a value with this label? Only what the person stated: trusted,
/// sourced from the person alone, not secret, and not private to a real Space.
pub fn desktop_admits(label: &Label) -> DesktopVerdict {
    let only_user = label.sources.len() == 1 && label.sources.contains(&Source::User);
    if label.confidentiality == Confidentiality::Secret {
        DesktopVerdict::Secret
    } else if label.integrity == Integrity::Untrusted {
        DesktopVerdict::Untrusted
    } else if !only_user {
        DesktopVerdict::NotUserStated
    } else if label.confidentiality.flow_to(&SpaceId::desktop()) == Flow::Denied {
        DesktopVerdict::SpaceBound
    } else {
        DesktopVerdict::Admit
    }
}
