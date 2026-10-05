//! The autoconfig, DNS and model-list fakes driven with a minimal client.

use porter_core::Family;
use porter_discover::{Dns, DnsFault, MxRecord, SrvRecord};
use porter_fake::FakeServer;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, send};
use porter_fake_servers::{
    FakeAutoconfig, FakeDns, FakeModels, ModelDef, Wire, autoconfig_xml, shipped,
};
use porter_provider::DomainName;

async fn get(base: &str, path: &str, bearer: Option<&str>) -> porter_fake_servers::Response {
    let (address, _) = split_loopback(base).expect("address");
    let mut request = Request::new("GET", path);
    if let Some(token) = bearer {
        request = request.with_header("Authorization", &format!("Bearer {token}"));
    }
    send(&address, Scheme::Http, &request).await.expect("get")
}

#[tokio::test]
async fn autoconfig_and_well_known_routes_are_a_table() {
    let server = FakeAutoconfig::start().await.expect("server");
    server.serve_autoconfig(&autoconfig_xml(
        "fake.test",
        ("imap.fake.test", 993),
        ("smtp.fake.test", 587),
    ));
    server.serve_redirect("/.well-known/caldav", "/dav/");
    server.serve_body("/.well-known/jmap", "application/json", "{}");

    let xml = get(
        server.base_url(),
        "/mail/config-v1.1.xml?emailaddress=a@fake.test",
        None,
    )
    .await;
    assert_eq!(
        (xml.status, xml.header("content-type")),
        (200, Some("text/xml"))
    );
    assert!(
        xml.text().contains("<hostname>imap.fake.test</hostname>")
            && xml.text().contains("<port>993</port>")
    );
    assert_eq!(
        get(
            server.base_url(),
            "/.well-known/autoconfig/mail/config-v1.1.xml",
            None
        )
        .await
        .status,
        200
    );
    let moved = get(server.base_url(), "/.well-known/caldav", None).await;
    assert_eq!(
        (moved.status, moved.header("location")),
        (301, Some("/dav/"))
    );
    assert_eq!(
        get(server.base_url(), "/.well-known/jmap", None)
            .await
            .text(),
        "{}"
    );
    assert_eq!(
        get(server.base_url(), "/.well-known/carddav", None)
            .await
            .status,
        404
    );
    assert_eq!(server.hits().len(), 5);
}

#[tokio::test]
async fn dns_answers_from_its_table_and_records_what_was_asked() {
    let imaps = SrvRecord {
        priority: 0,
        weight: 1,
        port: 993,
        target: DomainName::parse("imap.fake.test").expect("name"),
    };
    let mx = MxRecord {
        preference: 10,
        host: DomainName::parse("mx.fake.test").expect("name"),
    };
    let dns = FakeDns::new()
        .with_srv("_imaps._tcp.fake.test", vec![imaps.clone()])
        .with_mx("fake.test", vec![mx.clone()]);

    assert_eq!(dns.srv("_imaps._tcp.fake.test").await, Ok(vec![imaps]));
    assert_eq!(
        dns.srv("_imap._tcp.fake.test").await,
        Err(DnsFault::NoRecords)
    );
    let domain = DomainName::parse("FAKE.test").expect("name");
    assert_eq!(dns.mx(&domain).await, Ok(vec![mx]));
    assert_eq!(
        dns.mx(&DomainName::parse("other.test").expect("name"))
            .await,
        Err(DnsFault::NoRecords)
    );
    assert_eq!(
        dns.asked(),
        vec![
            "SRV _imaps._tcp.fake.test",
            "SRV _imap._tcp.fake.test",
            "MX fake.test",
            "MX other.test"
        ]
    );

    let dead = FakeDns::new().with_mx("fake.test", vec![]).unreachable();
    assert_eq!(dead.mx(&domain).await, Err(DnsFault::Unreachable));
}

#[tokio::test]
async fn ollama_lists_tags_and_shows_a_model() {
    let models = vec![
        ModelDef::chat("llama3.2:3b", 131_072),
        ModelDef::embedding("nomic-embed-text:latest", 2048),
    ];
    let server = FakeModels::start(Wire::Ollama, models, None)
        .await
        .expect("server");

    let tags = get(server.base_url(), "/api/tags", None)
        .await
        .json_body()
        .expect("json");
    let names: Vec<_> = tags["models"]
        .as_array()
        .expect("models")
        .iter()
        .map(|m| m["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["llama3.2:3b", "nomic-embed-text:latest"]);

    let (address, _) = split_loopback(server.base_url()).expect("address");
    let show = |name: &str| {
        let request =
            Request::new("POST", "/api/show").with_body(format!("{{\"model\":\"{name}\"}}"));
        let address = address.clone();
        async move { send(&address, Scheme::Http, &request).await.expect("show") }
    };
    let chat = show("llama3.2:3b").await.json_body().expect("json");
    assert_eq!(chat["model_info"]["llama.context_length"], 131_072);
    assert_eq!(
        chat["capabilities"],
        serde_json::json!(["completion", "tools"])
    );
    let embed = show("nomic-embed-text:latest")
        .await
        .json_body()
        .expect("json");
    assert_eq!(embed["capabilities"], serde_json::json!(["embedding"]));
    let missing = show("nope").await;
    assert_eq!(missing.status, 404);
    assert!(
        missing.json_body().expect("json")["error"]
            .as_str()
            .expect("error")
            .contains("not found")
    );
    // Not the OpenAI wire.
    assert_eq!(get(server.base_url(), "/v1/models", None).await.status, 404);
}

#[tokio::test]
async fn openai_compatible_models_list_wants_its_key() {
    let server = FakeModels::start(
        Wire::OpenAi,
        vec![ModelDef::chat("gpt-fake", 8192)],
        Some("sk-fake"),
    )
    .await
    .expect("server");
    assert_eq!(get(server.base_url(), "/v1/models", None).await.status, 401);
    assert_eq!(
        get(server.base_url(), "/v1/models", Some("sk-wrong"))
            .await
            .status,
        401
    );
    let list = get(server.base_url(), "/v1/models", Some("sk-fake"))
        .await
        .json_body()
        .expect("json");
    assert_eq!(
        (list["object"].as_str(), list["data"][0]["id"].as_str()),
        (Some("list"), Some("gpt-fake"))
    );
    // The hits record the Authorization header as received, for "no key leaked" assertions.
    let hits = server.hits();
    assert_eq!(hits[2].authorization.as_deref(), Some("Bearer sk-fake"));
    assert_eq!(hits[0].authorization, None);
}

#[tokio::test]
async fn rewrite_points_the_shipped_ollama_row_at_the_fake() {
    let fake = FakeModels::bind(Wire::Ollama, vec![], None)
        .await
        .expect("server");
    let base = fake.handle().base_url().to_owned();
    let rewritten = fake.rewrite(&shipped::ollama());
    let row = rewritten
        .capabilities
        .iter()
        .find(|r| r.family == Family::OllamaNative)
        .expect("row");
    assert_eq!(row.endpoint.as_ref().map(|e| e.0.clone()), Some(base));
}

#[test]
fn every_shipped_provider_file_is_readable_here() {
    let ids: Vec<_> = [
        shipped::nextcloud(),
        shipped::ollama(),
        shipped::google(),
        shipped::local(),
    ]
    .iter()
    .map(|s| s.id.clone())
    .collect();
    assert_eq!(ids.len(), 4);
}
