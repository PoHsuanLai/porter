use super::*;
use porter_core::GrantId;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{EndpointUrl, Family, LoginName, Tls};

fn account(id: &str) -> AccountId {
    AccountId::parse(id).expect("id")
}

fn endpoint(url: &str, tls: Tls) -> ServiceEndpoint {
    ServiceEndpoint {
        family: Family::ChatCompletions,
        url: EndpointUrl::parse(url).expect("url"),
        tls,
        login: LoginName(String::new()),
    }
}

fn server(id: &str, url: &str, tls: Tls) -> OwnServer {
    OwnServer::of(account(id), id.to_owned(), &[endpoint(url, tls)]).expect("a server")
}

fn granted(id: &str) -> AccountVerdict {
    AccountVerdict {
        account: account(id),
        provider: Some(OWN_PROVIDER.to_owned()),
        label: Some("Studio".to_owned()),
        provider_label: None,
        state: None,
        verdict: Verdict::Granted {
            grant: GrantId::parse("grant-1").expect("id"),
            scope: GrantScope::Always,
        },
    }
}

#[test]
fn an_address_is_a_place_and_a_way_to_reach_it() {
    let cases = [
        (
            "https://llm.example.test/v1",
            Tls::Implicit,
            ("llm.example.test", 443, "/v1", Link::Tls),
        ),
        (
            "https://llm.example.test:8443/",
            Tls::Implicit,
            ("llm.example.test", 8443, "", Link::Tls),
        ),
        (
            "http://localhost:8080/v1/",
            Tls::Plain,
            ("localhost", 8080, "/v1", Link::PlainLoopback),
        ),
        (
            "http://studio:8080/v1",
            Tls::Plain,
            ("studio", 8080, "/v1", Link::Plain),
        ),
    ];
    for (url, tls, (host, port, base, link)) in cases {
        let place = Place::of(&endpoint(url, tls)).expect(url);
        assert_eq!(
            (
                place.door().host.as_str(),
                place.door().port,
                place.door().base.as_str(),
                place.link()
            ),
            (host, port, base, link),
            "{url}"
        );
    }
    assert_eq!(
        Place::of(&endpoint("imaps://mail.example.test", Tls::Implicit)),
        None
    );
}

#[test]
fn a_key_is_a_bearer_over_tls_or_this_computer_and_an_empty_key_is_none() {
    let key = |text: &str| SecretText::new(text);
    let auth = |url: &str, tls, key: &SecretText| {
        Place::of(&endpoint(url, tls))
            .expect(url)
            .endpoint(key, LIST_WAIT)
            .auth
    };
    let bearer = |text: &str| AuthHeader::Bearer(Secret(text.to_owned()));
    assert_eq!(
        auth("https://a.test/v1", Tls::Implicit, &key("k")),
        bearer("k")
    );
    assert_eq!(
        auth("http://127.0.0.1:1/v1", Tls::Plain, &key("k")),
        AuthHeader::LoopbackBearer(Secret("k".to_owned()))
    );
    assert_eq!(auth("http://studio/v1", Tls::Plain, &key("k")), bearer("k"));
    for url in ["https://a.test/v1", "http://127.0.0.1:1/v1"] {
        let tls = if url.starts_with("https") {
            Tls::Implicit
        } else {
            Tls::Plain
        };
        assert_eq!(auth(url, tls, &key("")), AuthHeader::None, "{url}");
    }
}

#[test]
fn the_names_a_server_lists_are_read_once_each() {
    let cases: &[(&str, Option<&[&str]>)] = &[
        (
            r#"{"object":"list","data":[{"id":"Llama-3.2:3B"},{"id":"qwen"},{"id":"qwen"},{"id":"  "}]}"#,
            Some(&["Llama-3.2:3B", "qwen"]),
        ),
        (r#"{"data":[]}"#, Some(&[])),
        (r#"{"models":[{"name":"m"}]}"#, None),
        ("<html>captive portal</html>", None),
        ("", None),
    ];
    for (body, want) in cases {
        let got = names_in(body.as_bytes());
        let want = want.map(|names| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>());
        assert_eq!(got, want, "{body}");
    }
}

#[test]
fn a_listed_model_is_a_hosted_entry_with_no_limit_known_and_no_price() {
    let listed = Listed::new(&account("my-llm"), "Llama-3.2:3B").expect("an entry");
    assert_eq!(listed.id().as_str(), "llama-3.2-3b");
    assert_eq!(listed.entry.label, "Llama-3.2:3B");
    assert_eq!(listed.reach.provider.0, "my-llm");
    assert_eq!(listed.reach.model.0, "Llama-3.2:3B");
    assert_eq!(
        (
            listed.reach.price.input_per_mtok.0,
            listed.reach.price.output_per_mtok.0
        ),
        (0, 0)
    );
    let text = listed
        .entry
        .capabilities
        .text_out
        .as_ref()
        .expect("text out");
    assert_eq!((text.context.0, text.max_output.0), (0, 0));
    // A name that makes no id is not a model.
    assert!(Listed::new(&account("my-llm"), "///").is_none());
}

#[test]
fn what_a_server_listed_is_kept_until_it_moves_and_a_failed_look_is_asked_again_later() {
    let book = OwnBook::default();
    let here = server("my-llm", "https://llm.example.test/v1", Tls::Implicit);
    let moved = server("my-llm", "https://llm.example.test/v2", Tls::Implicit);
    let at = |s| UnixSeconds(1_700_000_000 + s);
    assert!(!book.is_current(&here, at(0)));

    book.record(&here, Some(vec!["a".into(), "b".into()]), at(0));
    assert!(
        book.is_current(&here, at(100_000)),
        "a list stands until Rescan"
    );
    assert!(
        !book.is_current(&moved, at(1)),
        "a new address is looked at again"
    );
    assert!(book.holds(&account("my-llm"), &id_of("a").expect("id")));
    assert!(!book.holds(&account("my-llm"), &id_of("c").expect("id")));

    book.record(&here, None, at(10));
    assert!(book.is_current(&here, at(10 + FAILED_RETRY - 1)));
    assert!(!book.is_current(&here, at(10 + FAILED_RETRY)));
    assert!(!book.holds(&account("my-llm"), &id_of("a").expect("id")));

    book.keep(&[]);
    assert_eq!(book.place_of(&account("my-llm")), None);
}

#[test]
fn the_models_of_a_server_are_hosted_models_labelled_with_their_account() {
    let book = OwnBook::default();
    let here = server("my-llm", "https://llm.example.test/v1", Tls::Implicit);
    book.record(&here, Some(vec!["Llama-3.2:3B".into()]), UnixSeconds(1));

    let models = book.models(&[granted("my-llm"), granted("elsewhere")]);
    assert_eq!(models.len(), 1);
    let model = &models[0];
    assert_eq!(model.card.account.as_str(), "my-llm");
    assert_eq!(model.card.model.as_str(), "llama-3.2-3b");
    assert_eq!(model.reach.provider.0, "my-llm");
    assert!(model.grant().is_some());
    // A longest reply nobody stated is the default, not no tokens; the request names the model
    // as the server does.
    let target = crate::cloud::turn::remote_target(model);
    assert_eq!(target.max_output.0, 16_384);
    assert_eq!(target.name.0, "Llama-3.2:3B");

    let choices = book.choices(&[granted("my-llm")]);
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].value, "my-llm/llama-3.2-3b");
    assert_eq!(choices[0].label, "Llama-3.2:3B on Studio");
    assert_eq!(choices[0].company, OWN_COMPANY);
    assert_eq!(choices[0].available, Availability::Reachable);

    // A server that did not answer offers nothing.
    book.record(&here, None, UnixSeconds(2));
    assert!(book.models(&[granted("my-llm")]).is_empty());
    assert!(book.choices(&[granted("my-llm")]).is_empty());
}
