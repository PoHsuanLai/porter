//! Request bodies: PROPFIND, the sync-collection REPORT and the multiget REPORTs. Built as text:
//! they are small and fixed in shape, and the variable parts (a sync token, property names,
//! hrefs) are escaped on the way in.

use crate::names::{CALDAV, CARDDAV, DAV, split};

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;

/// How deep a PROPFIND goes: the value of its `Depth` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Depth {
    /// The resource itself.
    Zero,
    /// The resource and its members.
    One,
}

impl Depth {
    /// The header's value: `0` or `1`.
    pub fn as_str(self) -> &'static str {
        match self {
            Depth::Zero => "0",
            Depth::One => "1",
        }
    }
}

/// How far below the collection a sync-collection report looks (RFC 6578 section 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncLevel {
    /// The collection's own members: calendars and address books.
    One,
    /// Everything below it: a file tree with folders.
    Infinite,
}

impl SyncLevel {
    fn as_str(self) -> &'static str {
        match self {
            SyncLevel::One => "1",
            SyncLevel::Infinite => "infinite",
        }
    }
}

/// Which multiget REPORT to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Multiget {
    /// `addressbook-multiget` (RFC 6352 section 8.7): each card's etag and `address-data`.
    Addressbook,
    /// `calendar-multiget` (RFC 4791 section 7.9): each object's etag and `calendar-data`.
    Calendar,
}

/// A PROPFIND body asking for `props` (namespace-qualified names, as in [`crate::names`]).
pub fn propfind(props: &[&str]) -> String {
    let asked = asked(props);
    format!(r#"{HEAD}<d:propfind xmlns:d="{DAV}"><d:prop>{asked}</d:prop></d:propfind>"#)
}

/// A sync-collection REPORT body (RFC 6578 section 3.2) asking for every change since `token`
/// at `level`, with `props` for each changed member. An empty `token` asks for everything: the
/// first listing.
pub fn sync_collection(token: &str, level: SyncLevel, props: &[&str]) -> String {
    let token = escape(token);
    let level = level.as_str();
    let asked = asked(props);
    format!(
        r#"{HEAD}<d:sync-collection xmlns:d="{DAV}"><d:sync-token>{token}</d:sync-token><d:sync-level>{level}</d:sync-level><d:prop>{asked}</d:prop></d:sync-collection>"#
    )
}

/// A multiget REPORT body for `hrefs`, asking for each one's etag and data.
pub fn multiget(kind: Multiget, hrefs: &[&str]) -> String {
    let (element, ns, data) = match kind {
        Multiget::Addressbook => ("addressbook-multiget", CARDDAV, "address-data"),
        Multiget::Calendar => ("calendar-multiget", CALDAV, "calendar-data"),
    };
    let hrefs: String = hrefs
        .iter()
        .map(|h| format!("<d:href>{}</d:href>", escape(h)))
        .collect();
    format!(
        r#"{HEAD}<c:{element} xmlns:d="{DAV}" xmlns:c="{ns}"><d:prop><d:getetag/><c:{data}/></d:prop>{hrefs}</c:{element}>"#
    )
}

/// XML character escaping for text and attribute content.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The elements of a `prop` that asks for `props`. A `DAV:` name uses the root's `d` prefix;
/// any other gets its own namespace declaration, since names are free-form and there is no
/// prefix table.
fn asked(props: &[&str]) -> String {
    props
        .iter()
        .map(|name| match split(name) {
            ("", local) => format!("<{}/>", escape(local)),
            (DAV, local) => format!("<d:{}/>", escape(local)),
            (ns, local) => format!(r#"<p:{} xmlns:p="{}"/>"#, escape(local), escape(ns)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::*;

    fn well_formed(xml: &str) -> roxmltree::Document<'_> {
        roxmltree::Document::parse(xml).unwrap_or_else(|e| panic!("{e}: {xml}"))
    }

    fn asked_names(xml: &str) -> Vec<(Option<String>, String)> {
        well_formed(xml)
            .descendants()
            .filter(|n| n.parent().is_some_and(|p| p.has_tag_name((DAV, "prop"))))
            .map(|n| {
                (
                    n.tag_name().namespace().map(str::to_owned),
                    n.tag_name().name().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn a_propfind_names_each_property_in_its_namespace() {
        let body = propfind(&[
            CURRENT_USER_PRINCIPAL,
            ADDRESSBOOK_HOME_SET,
            CALENDAR_HOME_SET,
            QUOTA_USED,
            "http://apple.com/ns/ical/calendar-color",
        ]);
        let want: Vec<(Option<String>, String)> = [
            (DAV, "current-user-principal"),
            (CARDDAV, "addressbook-home-set"),
            (CALDAV, "calendar-home-set"),
            (DAV, "quota-used-bytes"),
            ("http://apple.com/ns/ical/", "calendar-color"),
        ]
        .iter()
        .map(|(ns, local)| (Some((*ns).to_owned()), (*local).to_owned()))
        .collect();
        assert_eq!(asked_names(&body), want);
    }

    #[test]
    fn a_sync_collection_carries_its_level_and_props() {
        let body = sync_collection(
            "t&<1>",
            SyncLevel::Infinite,
            &[GETETAG, "DAV:getcontentlength"],
        );
        let doc = well_formed(&body);
        let text_of = |name: &str| {
            doc.descendants()
                .find(|n| n.has_tag_name((DAV, name)))
                .and_then(|n| n.text())
                .map(str::to_owned)
        };
        assert_eq!(text_of("sync-token").as_deref(), Some("t&<1>"));
        assert_eq!(text_of("sync-level").as_deref(), Some("infinite"));
        assert_eq!(asked_names(&body).len(), 2);
        let one = sync_collection("", SyncLevel::One, &[GETETAG]);
        assert!(one.contains("<d:sync-level>1</d:sync-level>"), "{one}");
    }

    #[test]
    fn a_calendar_multiget_asks_for_calendar_data() {
        let body = multiget(Multiget::Calendar, &["/cal/a.ics"]);
        let doc = well_formed(&body);
        assert!(
            doc.root_element()
                .has_tag_name((CALDAV, "calendar-multiget"))
        );
        assert!(
            doc.descendants()
                .any(|n| n.has_tag_name((CALDAV, "calendar-data")))
        );
    }

    #[test]
    fn the_depth_header_values() {
        assert_eq!(Depth::Zero.as_str(), "0");
        assert_eq!(Depth::One.as_str(), "1");
    }
}
