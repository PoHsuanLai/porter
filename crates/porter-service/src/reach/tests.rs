use super::*;
use porter_core::{LoginName, Tls};

fn endpoint(family: Family, url: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        family,
        url: EndpointUrl::parse(url).expect("url"),
        tls: Tls::Implicit,
        login: LoginName("ada".into()),
    }
}

fn any(prefix: &str) -> PathReach {
    PathReach::new(prefix.as_bytes().to_vec(), ReachMethods::Any)
}

fn properties(prefix: &str) -> PathReach {
    PathReach::new(prefix.as_bytes().to_vec(), ReachMethods::ReadProperties)
}

#[test]
fn each_family_reaches_its_own_paths() {
    type Case = (&'static str, Family, &'static str, Vec<PathReach>);
    let cases: Vec<Case> = vec![
        (
            "address books, with the principals beside them",
            Family::CardDav,
            "https://cloud.example.org/remote.php/dav/addressbooks/users/ada/",
            vec![
                any("/remote.php/dav/addressbooks/users/ada"),
                properties("/remote.php/dav/principals"),
            ],
        ),
        (
            "an escaped user name in the endpoint",
            Family::CardDav,
            "https://c.example/remote.php/dav/addressbooks/users/ada%40x.org/",
            vec![
                any("/remote.php/dav/addressbooks/users/ada@x.org"),
                properties("/remote.php/dav/principals"),
            ],
        ),
        (
            "a calendar home reads its principal",
            Family::CalDav,
            "https://dav.fastmail.example/dav/calendars/user/ada%40x.org/",
            vec![
                any("/dav/calendars/user/ada@x.org"),
                properties("/dav/principals"),
            ],
        ),
        (
            "files stay in the files, and reach no principal",
            Family::WebDav,
            "https://c.example/remote.php/dav/files/ada/",
            vec![any("/remote.php/dav/files/ada")],
        ),
        (
            "notes stay in the notes app",
            Family::NextcloudNotes,
            "https://c.example/index.php/apps/notes/api/v1/",
            vec![any("/index.php/apps/notes/api/v1")],
        ),
    ];
    for (name, family, url, want) in cases {
        let reach = reach_for(&endpoint(family, url));
        assert_eq!(reach.paths(), want.as_slice(), "{name}");
        assert!(!reach.is_origin(), "{name}");
    }
}

#[test]
fn a_family_that_is_per_token_or_names_its_own_urls_reaches_the_origin() {
    const CASES: &[(&str, Family, &str)] = &[
        (
            "google drive: uploads are on another path",
            Family::GoogleDrive,
            "https://www.googleapis.com/drive/v3",
        ),
        (
            "google photos: the picker's sessions",
            Family::GooglePhotosPicker,
            "https://photospicker.googleapis.com/v1/picker",
        ),
        (
            "graph is per token, not per path",
            Family::Graph,
            "https://graph.microsoft.com",
        ),
        (
            "jmap names its own urls on the origin",
            Family::Jmap,
            "https://api.fastmail.example/jmap/session",
        ),
        (
            "a dav server at its root",
            Family::CalDav,
            "https://caldav.icloud.example",
        ),
        (
            "a declared origin has its root as the endpoint",
            Family::Graph,
            "https://lh3.googleusercontent.com",
        ),
    ];
    for (name, family, url) in CASES {
        assert_eq!(
            reach_for(&endpoint(*family, url)),
            Reach::origin(),
            "{name}"
        );
    }
}
