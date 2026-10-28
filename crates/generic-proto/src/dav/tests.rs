use super::*;

fn url(text: &str) -> EndpointUrl {
    EndpointUrl::parse(text).unwrap()
}

fn reply(status: u16, body: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(body.as_bytes().to_vec())
        .unwrap()
}

#[test]
fn an_address_with_a_path_is_the_root_and_a_bare_server_is_asked_where() {
    let [calendar, contacts] = SERVICES;
    assert_eq!(
        root(&url("https://dav.example.test/dav/"), &calendar),
        Root::Is(Some(url("https://dav.example.test/dav/")))
    );
    assert_eq!(
        root(&url("https://dav.example.test"), &calendar),
        Root::WellKnown(url("https://dav.example.test/.well-known/caldav"))
    );
    assert_eq!(
        root(&url("https://dav.example.test"), &contacts),
        Root::WellKnown(url("https://dav.example.test/.well-known/carddav"))
    );
}

#[test]
fn the_probe_asks_for_the_principal_with_the_credential() {
    let mut auth = HeaderValue::from_static("Basic abc");
    auth.set_sensitive(true);
    let (_, request) = probe(&url("https://dav.example.test/dav/"), auth).unwrap();
    assert_eq!(request.method(), "PROPFIND");
    assert!(request.headers()[AUTHORIZATION].is_sensitive());
    let body = String::from_utf8(request.body().clone()).unwrap();
    assert!(
        body.contains("current-user-principal") && !body.contains("home-set"),
        "{body}"
    );
    assert_eq!(
        probe(
            &url("http://dav.example.test"),
            HeaderValue::from_static("x")
        )
        .unwrap_err(),
        Error::Insecure
    );
}

#[test]
fn the_probe_is_accepted_by_any_success_refused_or_failed_by_its_status() {
    let answer = |status, body: &str| {
        let (mut walk, _) = probe(
            &url("https://d.example.test/"),
            HeaderValue::from_static("x"),
        )
        .unwrap();
        accepted(&mut walk, &reply(status, body))
    };
    assert_eq!(answer(207, "<d:multistatus xmlns:d=\"DAV:\"/>"), Ok(true));
    assert_eq!(answer(200, "<html/>"), Ok(true));
    assert_eq!(answer(401, ""), Err(Error::Refused));
    assert_eq!(answer(403, ""), Err(Error::Refused));
    assert_eq!(answer(503, ""), Err(Error::Unreachable));
    assert_eq!(answer(404, ""), Ok(false));
    assert_eq!(answer(301, ""), Ok(false));
}

#[test]
fn a_service_is_claimed_from_the_files_row_or_missing_from_the_server() {
    let spec =
        porter_provider::parse_provider(include_str!("../../../../providers/generic-dav.toml"))
            .expect("the shipped file");
    let present = claim(&spec, CapabilityKind::Calendar, true);
    assert!(matches!(present.offer, Offer::Present(_)));
    assert_eq!(present.provenance, Provenance::Discovered);
    let absent = claim(&spec, CapabilityKind::Calendar, false);
    assert!(matches!(
        absent.offer,
        Offer::Absent {
            reason: AbsentReason::NotOnServer,
            ..
        }
    ));
}
