use super::*;
use porter_core::{Family, Tls};

fn endpoint(family: Family, url: &str, login: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        family,
        url: EndpointUrl::parse(url).expect("url"),
        tls: Tls::Implicit,
        login: LoginName(login.into()),
    }
}

/// A name, the endpoints, and the server and login expected.
type Case = (
    &'static str,
    Vec<ServiceEndpoint>,
    Option<(&'static str, &'static str)>,
);

#[test]
fn the_server_is_the_root_above_the_first_nextcloud_path() {
    let cases: &[Case] = &[
        (
            "webdav under the root",
            vec![endpoint(
                Family::WebDav,
                "https://cloud.example.org/remote.php/dav/files/ada/",
                "ada",
            )],
            Some(("https://cloud.example.org", "ada")),
        ),
        (
            "a server under a sub-path",
            vec![endpoint(
                Family::NextcloudNotes,
                "https://example.org/nextcloud/index.php/apps/notes/api/v1/",
                "ada",
            )],
            Some(("https://example.org/nextcloud", "ada")),
        ),
        (
            "a foreign endpoint is skipped",
            vec![
                endpoint(Family::WebDav, "https://files.example.org/dav/", "x"),
                endpoint(
                    Family::CalDav,
                    "https://cloud.example.org:8443/remote.php/dav/calendars/ada/",
                    "ada",
                ),
            ],
            Some(("https://cloud.example.org:8443", "ada")),
        ),
        ("no endpoints", vec![], None),
        (
            "none under a Nextcloud path",
            vec![endpoint(
                Family::WebDav,
                "https://files.example.org/dav/",
                "x",
            )],
            None,
        ),
    ];
    for (name, endpoints, want) in cases {
        let got = server_of(endpoints).map(|(s, l)| (s.to_string(), l.0));
        let want = want.map(|(s, l)| (s.to_owned(), l.to_owned()));
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn paths_are_under_the_server_root_and_a_user_id_is_encoded() {
    let server = EndpointUrl::parse("https://example.org/nextcloud").unwrap();
    let text = |url: Option<EndpointUrl>| url.map(|u| u.to_string());
    assert_eq!(
        text(dav_root(&server)).as_deref(),
        Some("https://example.org/nextcloud/remote.php/dav/")
    );
    assert_eq!(
        text(files(&server, "ada@example.org")).as_deref(),
        Some("https://example.org/nextcloud/remote.php/dav/files/ada%40example.org/")
    );
    assert_eq!(
        text(address_books(&server, "a b")).as_deref(),
        Some("https://example.org/nextcloud/remote.php/dav/addressbooks/users/a%20b/")
    );
    assert_eq!(
        text(calendars(&server, "ada")).as_deref(),
        Some("https://example.org/nextcloud/remote.php/dav/calendars/ada/")
    );
}

#[test]
fn the_user_is_the_last_segment_of_the_principal_decoded() {
    assert_eq!(
        user_of("/remote.php/dav/principals/users/ada/").as_deref(),
        Some("ada")
    );
    assert_eq!(
        user_of("/remote.php/dav/principals/users/ada%40example.org").as_deref(),
        Some("ada@example.org")
    );
    assert_eq!(
        user_of("/remote.php/dav/principals/users/a%20b/").as_deref(),
        Some("a b")
    );
    assert_eq!(user_of("/"), None);
    assert_eq!(user_of("/a/%zz/"), None);
}
