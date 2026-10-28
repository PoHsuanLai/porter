//! The D-Bus shape every interface passes options and rows in: a vardict, as portals do.

use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

/// A vardict (`a{sv}`).
pub type Details = HashMap<String, OwnedValue>;
