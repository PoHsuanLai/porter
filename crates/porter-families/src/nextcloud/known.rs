//! The server and login name an existing account already holds, read back from its endpoints
//! (every endpoint of a Nextcloud account is under the server's root).

use porter_core::{EndpointUrl, LoginName, ServiceEndpoint};

/// Where a Nextcloud's own paths start, as a marker in an endpoint's URL.
const MARKERS: [&str; 2] = ["/remote.php/", "/index.php/"];

/// The server root and the login of the first endpoint that sits under a Nextcloud's own paths.
pub(super) fn server_of(endpoints: &[ServiceEndpoint]) -> Option<(EndpointUrl, LoginName)> {
    endpoints.iter().find_map(|endpoint| {
        let text = endpoint.url.as_str();
        let root = MARKERS
            .iter()
            .filter_map(|marker| text.find(marker))
            .min()
            .map(|at| &text[..at])?;
        let server = EndpointUrl::parse(root).ok()?;
        Some((server, endpoint.login.clone()))
    })
}

#[cfg(test)]
mod tests {
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
}
