//! The D-Bus shapes of porter's values: a kind slug plus a vardict, as portals pass options.

use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// A vardict (`a{sv}`).
pub type Details = HashMap<String, OwnedValue>;

/// A need (`(sa{sv})`): the kind's slug and its fields by name.
pub type NeedArg = (String, Details);

/// A candidate (`(osa{sv})`): the account's object path, its label, and the rest by name.
pub type CandidateArg = (OwnedObjectPath, String, Details);

/// An issued token (`(ssx)`): kind slug, value, expiry in Unix seconds.
pub type TokenArg = (String, String, i64);

/// An app named to the Peer interface (`(ss)`): its reverse-DNS name and its isolation slug.
pub type AppArg = (String, String);

/// One account's verdict for an app (`(ssa{sv})`): the account id, `granted`, `denied` or `ask`,
/// and for a grant its `grant` id and `scope` by name.
pub type VerdictArg = (String, String, Details);
