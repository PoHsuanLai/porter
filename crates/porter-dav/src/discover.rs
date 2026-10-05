//! Discovery and quota, read from a parsed [`Multistatus`]: `current-user-principal` (RFC 5397),
//! the calendar and addressbook home sets, and `quota-used-bytes`/`quota-available-bytes`
//! (RFC 4331). The request side is [`crate::propfind`] with the names in [`crate::names`].
//!
//! The discovery walk (principal, then home set, then the home's children) follows mailo
//! `crates/mail-pim/src/dav/mod.rs` (same author, MIT OR Apache-2.0).

use crate::multistatus::{Multistatus, Response};
use crate::names::{
    ADDRESSBOOK_HOME_SET, CALENDAR_HOME_SET, CURRENT_USER_PRINCIPAL, QUOTA_AVAILABLE, QUOTA_USED,
    RESOURCETYPE,
};

/// Which home set to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// `calendar-home-set` (CalDAV).
    Calendar,
    /// `addressbook-home-set` (CardDAV).
    Addressbook,
}

impl Home {
    fn property(self) -> &'static str {
        match self {
            Home::Calendar => CALENDAR_HOME_SET,
            Home::Addressbook => ADDRESSBOOK_HOME_SET,
        }
    }

    fn collection_type(self) -> &'static str {
        match self {
            Home::Calendar => "urn:ietf:params:xml:ns:caldav:calendar",
            Home::Addressbook => "urn:ietf:params:xml:ns:carddav:addressbook",
        }
    }
}

/// The principal URL, as written (a path or a URL).
pub fn current_user_principal(status: &Multistatus) -> Option<String> {
    first_found(status, CURRENT_USER_PRINCIPAL).map(first_word)
}

/// The home-set hrefs of `home`, in order.
pub fn home_set(status: &Multistatus, home: Home) -> Vec<String> {
    first_found(status, home.property())
        .map(|v| v.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The hrefs among the responses whose `resourcetype` is a `home` collection (a calendar or an
/// address book): what a PROPFIND at depth 1 on the home lists.
pub fn collections(status: &Multistatus, home: Home) -> Vec<String> {
    status
        .responses
        .iter()
        .filter(|r| {
            r.found(RESOURCETYPE)
                .is_some_and(|t| t.split_whitespace().any(|k| k == home.collection_type()))
        })
        .map(|r| r.href.clone())
        .collect()
}

/// A collection's quota; a field is `None` when the server did not give it (or gave no number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Quota {
    /// `quota-used-bytes`.
    pub used: Option<u64>,
    /// `quota-available-bytes`: what is left, `None` also for a server without a limit.
    pub available: Option<u64>,
}

/// The quota the first response reports.
pub fn quota(status: &Multistatus) -> Quota {
    let number = |name| first_found(status, name).and_then(|v| v.trim().parse().ok());
    Quota {
        used: number(QUOTA_USED),
        available: number(QUOTA_AVAILABLE),
    }
}

fn first_found<'a>(status: &'a Multistatus, name: &str) -> Option<&'a str> {
    status
        .responses
        .iter()
        .find_map(|r: &Response| r.found(name))
        .filter(|v| !v.is_empty())
}

fn first_word(value: &str) -> String {
    value
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}
