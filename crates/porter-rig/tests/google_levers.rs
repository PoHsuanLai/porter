//! The control endpoint's `/google/...` levers act on the fake Google's Calendar, People and
//! Tasks, and say so plainly when a parameter is missing or the fake was not started.

use porter_fake_servers::http::Request;
use porter_fake_servers::{FakeGoogle, FakeIssuer};
use porter_rig::control::Levers;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

fn levers(google: Option<porter_fake_servers::GoogleHandle>) -> Levers {
    Levers {
        issuer: None,
        graph: None,
        google,
        imap: None,
        smtp: None,
        pop3: None,
        ollama: None,
        rig: Arc::new(Mutex::new(String::new())),
        stop: Arc::new(Notify::new()),
    }
}

fn post(levers: &Levers, target: &str, body: Value) -> (u16, Value) {
    let request = Request::new("POST", target).with_body(body.to_string());
    let answer = levers.answer(&request);
    (answer.status, answer.json_body().unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_levers_seed_edit_and_remove_what_the_fake_google_holds() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let api = FakeGoogle::start(&issuer).await.expect("google");
    let levers = levers(Some((*api).clone()));

    let (status, body) = post(&levers, "/google/seed?what=all", Value::Null);
    assert_eq!((status, &body["ok"]), (200, &json!(true)), "{body}");
    assert_eq!(api.events("cal-personal").len(), 4);
    assert_eq!(api.people().len(), 2);
    assert_eq!(api.tasks("list-home").len(), 2);

    let event =
        json!({"summary": "Lunch", "start": {"date": "2026-10-09"}, "end": {"date": "2026-10-10"}});
    assert_eq!(
        post(
            &levers,
            "/google/event?calendar=cal-work&id=ev-lunch",
            event
        )
        .0,
        200
    );
    assert_eq!(api.events("cal-work").len(), 2);
    assert_eq!(
        post(
            &levers,
            "/google/event-remove?calendar=cal-work&id=ev-lunch",
            Value::Null
        )
        .0,
        200
    );
    assert_eq!(api.events("cal-work").len(), 1);
    assert_eq!(
        post(
            &levers,
            "/google/calendar?id=cal-new&name=New&color=%23ff0000",
            Value::Null
        )
        .0,
        200
    );
    assert_eq!(
        post(&levers, "/google/calendar-remove?id=cal-work", Value::Null).0,
        200
    );
    assert!(api.events("cal-work").is_empty());

    let person = json!({"names": [{"displayName": "Linus"}]});
    assert_eq!(
        post(&levers, "/google/person?resource=people/c9", person).0,
        200
    );
    assert_eq!(api.people().len(), 3);
    assert_eq!(
        post(
            &levers,
            "/google/person-remove?resource=people/c9",
            Value::Null
        )
        .0,
        200
    );
    assert_eq!(api.people().len(), 2);

    assert_eq!(
        post(
            &levers,
            "/google/task-list?id=list-new&title=New",
            Value::Null
        )
        .0,
        200
    );
    let task = json!({"title": "Call"});
    assert_eq!(
        post(&levers, "/google/task?list=list-new&id=t1", task).0,
        200
    );
    assert_eq!(api.tasks("list-new").len(), 1);
    assert_eq!(
        post(
            &levers,
            "/google/task-remove?list=list-new&id=t1",
            Value::Null
        )
        .0,
        200
    );
    assert!(api.tasks("list-new").is_empty());
    assert_eq!(
        post(&levers, "/google/task-list-remove?id=list-new", Value::Null).0,
        200
    );
    assert_eq!(post(&levers, "/google/expire-sync", Value::Null).0, 200);

    let hits = levers.answer(&Request::new("GET", "/google/hits"));
    assert_eq!(hits.status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lever_with_a_missing_parameter_or_a_bad_body_or_no_fake_says_why() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let api = FakeGoogle::start(&issuer).await.expect("google");
    let levers = levers(Some((*api).clone()));
    for (target, body, want) in [
        ("/google/event?calendar=c", json!({}), 400),
        (
            "/google/event?calendar=c&id=e",
            Value::String("not json".into()),
            400,
        ),
        ("/google/person", json!({}), 400),
        ("/google/task?list=l", json!({}), 400),
        ("/google/seed?what=everything", Value::Null, 400),
        ("/google/nothing", Value::Null, 404),
    ] {
        let request = Request::new("POST", target).with_body(match &body {
            Value::String(text) => text.clone().into_bytes(),
            other => other.to_string().into_bytes(),
        });
        assert_eq!(levers.answer(&request).status, want, "{target}");
    }
    let none = self::levers(None);
    assert_eq!(post(&none, "/google/seed", Value::Null).0, 409);
}
