//! The format's reading, ported from `parsing` in mailo's `crates/mail-proto/tests/discover.rs`.

use super::*;
use crate::testing::{doc, server};

#[test]
fn every_server_is_read_in_document_order() {
    let xml = doc(&[
        server(
            "imap",
            "imap.example.test",
            993,
            "SSL",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "imap",
            "imap.example.test",
            143,
            "STARTTLS",
            "%EMAILADDRESS%",
            &["password-cleartext"],
        ),
        server(
            "pop3",
            "pop.example.test",
            995,
            "SSL",
            "%EMAILLOCALPART%",
            &["password-encrypted"],
        ),
        server(
            "smtp",
            "smtp.example.test",
            465,
            "SSL",
            "%EMAILADDRESS%",
            &["OAuth2", "password-cleartext"],
        ),
    ]
    .concat());
    let config = parse(&xml).expect("parses");
    assert_eq!(config.incoming.len(), 3);
    assert_eq!(config.outgoing.len(), 1);
    assert_eq!(config.incoming[0].protocol, ServerProtocol::Imap);
    assert_eq!(config.incoming[0].socket, SocketType::Ssl);
    assert_eq!(config.incoming[1].socket, SocketType::StartTls);
    assert_eq!(config.incoming[2].protocol, ServerProtocol::Pop3);
    assert_eq!(config.incoming[2].username, "%EMAILLOCALPART%");
    assert_eq!(config.incoming[2].auth, vec![AuthMethod::PasswordEncrypted]);
    assert_eq!(
        config.outgoing[0].auth,
        vec![AuthMethod::OAuth2, AuthMethod::PasswordCleartext]
    );
    assert_eq!(config.oauth_issuer, None);
}

#[test]
fn the_oauth_issuer_is_read_from_its_own_element() {
    let xml = doc(&server(
        "imap",
        "imap.example.test",
        993,
        "SSL",
        "",
        &["OAuth2"],
    ))
    .replace(
        "</clientConfig>",
        "<oAuth2><issuer>accounts.google.com</issuer><scope>x</scope></oAuth2></clientConfig>",
    );
    let config = parse(&xml).expect("parses");
    assert_eq!(config.oauth_issuer.as_deref(), Some("accounts.google.com"));
}

#[test]
fn a_server_with_no_host_or_a_bad_port_is_dropped_alone() {
    let xml = doc(&[
        server("imap", "", 993, "SSL", "", &[]),
        "<incomingServer type=\"imap\"><hostname>x.example.test</hostname><port>ninety</port></incomingServer>".to_owned(),
        "<incomingServer type=\"imap\"><hostname>y.example.test</hostname><port>0</port></incomingServer>".to_owned(),
        server("imap", "good.example.test", 993, "SSL", "", &[]),
    ]
    .concat());
    let config = parse(&xml).expect("parses");
    assert_eq!(config.incoming.len(), 1);
    assert_eq!(config.incoming[0].hostname, "good.example.test");
}

#[test]
fn a_missing_socket_type_means_no_tls() {
    let xml = doc(
        "<incomingServer type=\"imap\"><hostname>h.example.test</hostname><port>143</port></incomingServer>",
    );
    let config = parse(&xml).expect("parses");
    assert_eq!(config.incoming[0].socket, SocketType::Plain);
}

#[test]
fn malformed_documents_are_refused_with_a_reason() {
    assert!(matches!(
        parse("<clientConfig><emailProvider>"),
        Err(AutoconfigError::Xml(_))
    ));
    assert!(matches!(
        parse("<html><body>Not here</body></html>"),
        Err(AutoconfigError::NotClientConfig)
    ));
    assert!(matches!(
        parse("<clientConfig version=\"1.1\"/>"),
        Err(AutoconfigError::NoProvider)
    ));
    assert!(matches!(parse(""), Err(AutoconfigError::Xml(_))));
}

#[test]
fn a_document_type_declaration_is_refused_rather_than_expanded() {
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE clientConfig [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;">]>
<clientConfig><emailProvider><displayName>&b;</displayName></emailProvider></clientConfig>"#;
    assert!(matches!(parse(xml), Err(AutoconfigError::Xml(_))));
}

#[test]
fn a_default_namespace_does_not_hide_the_elements() {
    let xml = doc(&server("imap", "imap.example.test", 993, "SSL", "", &[]))
        .replace("<clientConfig ", "<clientConfig xmlns=\"urn:x\" ");
    assert_eq!(parse(&xml).expect("parses").incoming.len(), 1);
}

#[test]
fn the_recorded_ispdb_document_reads_whole() {
    let xml = include_str!("../../fixtures/ispdb-example.xml");
    let config = parse(xml).expect("parses");
    assert_eq!(config.incoming.len(), 3);
    assert_eq!(config.outgoing.len(), 1);
    assert_eq!(config.outgoing[0].port, 465);
}
