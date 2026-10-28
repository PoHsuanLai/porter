use super::*;
use porter_capability::{Capability, Offer, PimCap};

fn reply(status: u16) -> Response<Vec<u8>> {
    Response::builder().status(status).body(Vec::new()).unwrap()
}

#[test]
fn a_reply_is_a_refusal_a_server_failure_another_failure_or_a_success() {
    assert_eq!(judged(&reply(401)).unwrap_err(), Error::Refused);
    assert_eq!(judged(&reply(403)).unwrap_err(), Error::Refused);
    assert_eq!(judged(&reply(502)).unwrap_err(), Error::Unreachable);
    assert_eq!(judged(&reply(404)).unwrap_err(), Error::Unreadable);
    assert_eq!(judged(&reply(207)).unwrap().status(), 207);
}

fn claims(kinds: &[CapabilityKind]) -> Vec<Claim> {
    let json =
        r#"{"ocs":{"meta":{"statuscode":200},"data":{"capabilities":{"files":{},"dav":{}}}}}"#;
    ocs::parse_capabilities(json)
        .unwrap()
        .into_iter()
        .filter(|c| kinds.contains(&c.offer.kind()))
        .collect()
}

#[test]
fn the_endpoints_are_the_files_root_and_each_app_the_server_offers() {
    let server = EndpointUrl::parse("https://cloud.example.org").unwrap();
    let login = LoginName("ada".into());
    let offered = claims(&[CapabilityKind::Calendar, CapabilityKind::Contacts]);
    assert!(matches!(
        &offered[0].offer,
        Offer::Present(Capability::Calendar(PimCap { .. }))
    ));
    let got = endpoints(&server, "ada", &login, &offered, None, None).unwrap();
    let urls: Vec<_> = got.iter().map(|e| (e.family, e.url.to_string())).collect();
    assert_eq!(
        urls,
        [
            (
                Family::WebDav,
                "https://cloud.example.org/remote.php/dav/files/ada/".to_owned()
            ),
            (
                Family::CalDav,
                "https://cloud.example.org/remote.php/dav/calendars/ada/".to_owned()
            ),
            (
                Family::CardDav,
                "https://cloud.example.org/remote.php/dav/addressbooks/users/ada/".to_owned()
            ),
        ]
    );
}

#[test]
fn a_home_the_server_named_replaces_the_conventional_path() {
    let server = EndpointUrl::parse("https://cloud.example.org").unwrap();
    let login = LoginName("ada".into());
    let home = EndpointUrl::parse("https://cloud.example.org/dav/cal/ada/").unwrap();
    let offered = claims(&[CapabilityKind::Calendar]);
    let got = endpoints(&server, "ada", &login, &offered, Some(home.clone()), None).unwrap();
    assert_eq!(got[1].url, home);
    assert_eq!(got.len(), 2);
}
