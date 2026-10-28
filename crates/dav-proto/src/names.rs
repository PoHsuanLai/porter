//! Namespaces and property names, namespace-qualified as [`crate::propfind`] takes them and
//! [`crate::Prop::name`] gives them back.

/// `DAV:`, the WebDAV namespace.
pub const DAV: &str = "DAV:";
/// The CalDAV namespace (RFC 4791 section 5.1).
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
/// The CardDAV namespace (RFC 6352 section 10.1).
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";

/// `DAV:getetag`.
pub const GETETAG: &str = "DAV:getetag";
/// `DAV:displayname`.
pub const DISPLAYNAME: &str = "DAV:displayname";
/// `DAV:resourcetype`.
pub const RESOURCETYPE: &str = "DAV:resourcetype";
/// `DAV:sync-token` as a property (RFC 6578 section 4).
pub const SYNC_TOKEN: &str = "DAV:sync-token";
/// `DAV:current-user-principal` (RFC 5397).
pub const CURRENT_USER_PRINCIPAL: &str = "DAV:current-user-principal";
/// `DAV:quota-used-bytes` (RFC 4331).
pub const QUOTA_USED: &str = "DAV:quota-used-bytes";
/// `DAV:quota-available-bytes` (RFC 4331).
pub const QUOTA_AVAILABLE: &str = "DAV:quota-available-bytes";
/// `addressbook-home-set` (RFC 6352).
pub const ADDRESSBOOK_HOME_SET: &str = "urn:ietf:params:xml:ns:carddav:addressbook-home-set";
/// `calendar-home-set` (RFC 4791).
pub const CALENDAR_HOME_SET: &str = "urn:ietf:params:xml:ns:caldav:calendar-home-set";
/// `address-data`, a card's vCard text (RFC 6352 section 10.4).
pub const ADDRESS_DATA: &str = "urn:ietf:params:xml:ns:carddav:address-data";
/// `calendar-data`, an object's iCalendar text (RFC 4791 section 9.6).
pub const CALENDAR_DATA: &str = "urn:ietf:params:xml:ns:caldav:calendar-data";

/// Apple's calendar colour property, which Nextcloud, iCloud and Radicale all serve.
pub const CALENDAR_COLOR: &str = "http://apple.com/ns/ical/calendar-color";

/// Splits `ns:local` or `ns/local` at the last separator: the namespace as XML writes it
/// (`DAV:` keeps its colon, `urn:...:carddav` loses the joining one) and the local name.
pub(crate) fn split(name: &str) -> (&str, &str) {
    match name.rfind([':', '/']) {
        None => ("", name),
        Some(at) => {
            let (ns, local) = (&name[..at + 1], &name[at + 1..]);
            match ns {
                DAV => (ns, local),
                _ => (ns.strip_suffix(':').unwrap_or(ns), local),
            }
        }
    }
}

/// The qualified spelling of a namespace and local name, the inverse of [`split`].
pub(crate) fn qualify(namespace: Option<&str>, local: &str) -> String {
    match namespace {
        None | Some("") => local.to_owned(),
        Some(ns) if ns.ends_with([':', '/']) => format!("{ns}{local}"),
        Some(ns) => format!("{ns}:{local}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_qualify_round_trip() {
        const CASES: &[(&str, &str, &str)] = &[
            ("DAV:getetag", "DAV:", "getetag"),
            (
                ADDRESSBOOK_HOME_SET,
                "urn:ietf:params:xml:ns:carddav",
                "addressbook-home-set",
            ),
            (
                "http://calendarserver.org/ns/getctag",
                "http://calendarserver.org/ns/",
                "getctag",
            ),
            ("plain", "", "plain"),
        ];
        for (name, ns, local) in CASES {
            assert_eq!(split(name), (*ns, *local), "{name}");
            assert_eq!(qualify(Some(ns), local), *name, "{name}");
        }
    }
}
