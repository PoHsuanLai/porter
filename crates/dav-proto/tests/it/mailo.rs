//! The tests mailo's `mail-pim::dav` carries (same author, MIT OR Apache-2.0), over this
//! crate's model: matching by namespace, the typed CardDAV view and the request bodies.

use dav_proto::carddav::{Props, Resource};
use dav_proto::names::{CARDDAV, DAV};
use dav_proto::{Multiget, SyncLevel, multiget, parse_multistatus, propfind, sync_collection};

fn well_formed(xml: &str) -> roxmltree::Document<'_> {
    roxmltree::Document::parse(xml).unwrap_or_else(|e| panic!("{e}: {xml}"))
}

#[test]
fn elements_are_matched_by_namespace_whatever_the_prefix() {
    const CASES: &[&str] = &[
        r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/a</d:href></d:response></d:multistatus>"#,
        r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/a</D:href></D:response></D:multistatus>"#,
        r#"<multistatus xmlns="DAV:"><response><href>/a</href></response></multistatus>"#,
    ];
    for xml in CASES {
        let reply = parse_multistatus(xml).unwrap();
        assert_eq!(reply.responses.len(), 1, "{xml}");
        assert_eq!(reply.responses[0].href, "/a", "{xml}");
    }
}

#[test]
fn an_element_in_another_namespace_is_not_the_dav_one() {
    let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:x="urn:x"><x:response><d:href>/a</d:href></x:response></d:multistatus>"#;
    assert!(parse_multistatus(xml).unwrap().responses.is_empty());
}

#[test]
fn properties_from_a_failed_propstat_are_left_out() {
    let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">
      <d:response><d:href>/p/</d:href>
        <d:propstat><d:prop><d:displayname>Me</d:displayname>
          <d:resourcetype><d:collection/><c:addressbook/><x:other xmlns:x="urn:x"/></d:resourcetype>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
        <d:propstat><d:prop><d:getetag>"lie"</d:getetag></d:prop>
          <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
      </d:response></d:multistatus>"#;
    let props = Props::of(&parse_multistatus(xml).unwrap().responses[0]);
    assert_eq!(props.display_name.as_deref(), Some("Me"));
    assert_eq!(props.etag, None);
    assert_eq!(
        props.resource,
        [
            Resource::Collection,
            Resource::AddressBook,
            Resource::Other("other".into())
        ]
    );
    assert!(props.is_address_book());
}

#[test]
fn a_sync_report_carries_its_new_token_and_its_removals() {
    let xml = r#"<d:multistatus xmlns:d="DAV:">
      <d:response><d:href>/book/a.vcf</d:href>
        <d:propstat><d:prop><d:getetag>"1"</d:getetag></d:prop>
        <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
      <d:response><d:href>/book/b.vcf</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
      <d:sync-token>http://x.test/sync/2</d:sync-token>
    </d:multistatus>"#;
    let reply = parse_multistatus(xml).unwrap();
    assert_eq!(reply.sync_token.as_deref(), Some("http://x.test/sync/2"));
    assert_eq!(
        Props::of(&reply.responses[0]).etag.as_deref(),
        Some("\"1\"")
    );
    assert!(!reply.responses[0].gone());
    assert!(reply.responses[1].gone());
}

#[test]
fn principal_and_home_hrefs_are_read_from_inside_their_properties() {
    let xml = r#"<multistatus xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><response><href>/</href>
      <propstat><prop>
        <current-user-principal><href>/principals/me/</href></current-user-principal>
        <c:addressbook-home-set><href>/home/one/</href><href>/home/two/</href></c:addressbook-home-set>
      </prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    let props = Props::of(&parse_multistatus(xml).unwrap().responses[0]);
    assert_eq!(
        props.current_user_principal.as_deref(),
        Some("/principals/me/")
    );
    assert_eq!(props.addressbook_home_set, ["/home/one/", "/home/two/"]);
}

#[test]
fn address_data_in_cdata_is_read_as_text() {
    let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/a</d:href><d:propstat><d:prop><c:address-data><![CDATA[BEGIN:VCARD\r\nFN:A & B\r\nEND:VCARD]]></c:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
    let data = Props::of(&parse_multistatus(xml).unwrap().responses[0])
        .address_data
        .unwrap();
    assert!(data.contains("FN:A & B"), "{data}");
}

#[test]
fn a_reply_that_is_not_a_multistatus_is_an_error_not_an_empty_one() {
    const CASES: &[&str] = &[
        "<html><body>Sign in</body></html>",
        "not xml at all",
        r#"<!DOCTYPE x [<!ENTITY a "aaaa">]><d:multistatus xmlns:d="DAV:">&a;</d:multistatus>"#,
    ];
    for xml in CASES {
        assert!(parse_multistatus(xml).is_err(), "{xml}");
    }
}

#[test]
fn a_propfind_names_each_property_in_its_namespace() {
    let body = propfind(&[
        "DAV:current-user-principal",
        "urn:ietf:params:xml:ns:carddav:addressbook-home-set",
    ]);
    let doc = well_formed(&body);
    let asked: Vec<(Option<&str>, &str)> = doc
        .descendants()
        .filter(|n| n.parent().is_some_and(|p| p.has_tag_name((DAV, "prop"))))
        .map(|n| (n.tag_name().namespace(), n.tag_name().name()))
        .collect();
    assert_eq!(
        asked,
        [
            (Some(DAV), "current-user-principal"),
            (Some(CARDDAV), "addressbook-home-set")
        ]
    );
}

#[test]
fn a_sync_token_and_hrefs_are_escaped_into_the_body() {
    let body = sync_collection(
        "http://x.test/sync?a=1&b=<2>",
        SyncLevel::One,
        &["DAV:getetag"],
    );
    let doc = well_formed(&body);
    let token = doc
        .descendants()
        .find(|n| n.has_tag_name((DAV, "sync-token")))
        .and_then(|n| n.text());
    assert_eq!(token, Some("http://x.test/sync?a=1&b=<2>"));

    let body = multiget(Multiget::Addressbook, &["/book/a&b.vcf", "/book/c.vcf"]);
    let doc = well_formed(&body);
    assert!(
        doc.root_element()
            .has_tag_name((CARDDAV, "addressbook-multiget"))
    );
    let hrefs: Vec<&str> = doc
        .descendants()
        .filter(|n| n.has_tag_name((DAV, "href")))
        .filter_map(|n| n.text())
        .collect();
    assert_eq!(hrefs, ["/book/a&b.vcf", "/book/c.vcf"]);
    let asked: Vec<(Option<&str>, &str)> = doc
        .descendants()
        .filter(|n| n.parent().is_some_and(|p| p.has_tag_name((DAV, "prop"))))
        .map(|n| (n.tag_name().namespace(), n.tag_name().name()))
        .collect();
    assert_eq!(
        asked,
        [(Some(DAV), "getetag"), (Some(CARDDAV), "address-data")]
    );
}

#[test]
fn a_first_sync_sends_an_empty_token() {
    let body = sync_collection("", SyncLevel::One, &["DAV:getetag"]);
    assert!(body.contains("<d:sync-token></d:sync-token>"), "{body}");
    assert!(body.contains("<d:getetag/>"), "{body}");
}
