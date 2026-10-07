//! The fake Google's Calendar, People and Tasks: what each lists, how its changes are asked for
//! and how a stale token is refused. These are the shapes the syncd Google sources read.

use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Response, Scheme, send};
use porter_fake_servers::{FakeGoogle, FakeIssuer, GoogleHandle, Running};
use serde_json::{Value, json};

const TOKEN: &str = "fixed-token";

struct Rig {
    _issuer: Running<porter_fake_servers::IssuerHandle>,
    google: Running<GoogleHandle>,
}

async fn rig() -> Rig {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let google = FakeGoogle::start(&issuer).await.expect("google");
    google.accept_bearer(TOKEN);
    Rig {
        _issuer: issuer,
        google,
    }
}

async fn get_as(rig: &Rig, target: &str, bearer: Option<&str>) -> Response {
    let (address, _) = split_loopback(rig.google.base_url()).expect("address");
    let mut request = Request::new("GET", target);
    if let Some(token) = bearer {
        request = request.with_header("Authorization", &format!("Bearer {token}"));
    }
    send(&address, Scheme::Http, &request).await.expect("get")
}

async fn get(rig: &Rig, target: &str) -> (u16, Value) {
    let response = get_as(rig, target, Some(TOKEN)).await;
    let body = response.json_body().unwrap_or(Value::Null);
    (response.status, body)
}

fn ids(items: &Value, key: &str) -> Vec<String> {
    items["items"]
        .as_array()
        .or_else(|| items[key].as_array())
        .map(|list| {
            list.iter()
                .map(|i| {
                    i["id"]
                        .as_str()
                        .or(i["resourceName"].as_str())
                        .unwrap_or("?")
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_request_without_an_accepted_bearer_is_unauthorized() {
    let rig = rig().await;
    rig.google.seed_calendars();
    for bearer in [None, Some("other")] {
        let response = get_as(&rig, "/calendar/v3/users/me/calendarList", bearer).await;
        assert_eq!(response.status, 401);
    }
}

#[tokio::test]
async fn calendars_list_with_names_and_colours_and_events_page_to_a_sync_token() {
    let rig = rig().await;
    rig.google.seed_calendars();
    let (status, list) = get(&rig, "/calendar/v3/users/me/calendarList").await;
    assert_eq!(status, 200);
    assert_eq!(ids(&list, "items"), ["cal-personal", "cal-work"]);
    assert_eq!(list["items"][0]["summary"], "Personal");
    assert_eq!(list["items"][0]["backgroundColor"], "#9fe1e7");

    let (_, first) = get(
        &rig,
        "/calendar/v3/calendars/cal-personal/events?maxResults=3",
    )
    .await;
    assert_eq!(ids(&first, "items").len(), 3);
    assert!(first.get("nextSyncToken").is_none());
    let page = first["nextPageToken"]
        .as_str()
        .expect("next page")
        .to_owned();
    let (_, second) = get(
        &rig,
        &format!("/calendar/v3/calendars/cal-personal/events?maxResults=3&pageToken={page}"),
    )
    .await;
    assert_eq!(ids(&second, "items").len(), 1);
    assert!(second.get("nextPageToken").is_none());
    assert!(second["nextSyncToken"].as_str().is_some());
}

#[tokio::test]
async fn a_sync_token_returns_what_changed_with_cancelled_stubs_and_goes_stale_with_410() {
    let rig = rig().await;
    rig.google.seed_calendars();
    let (_, all) = get(&rig, "/calendar/v3/calendars/cal-work/events").await;
    let token = all["nextSyncToken"].as_str().expect("token").to_owned();
    let incremental = |token: &str| {
        format!("/calendar/v3/calendars/cal-work/events?syncToken={token}&showDeleted=true")
    };

    let (_, none) = get(&rig, &incremental(&token)).await;
    assert!(ids(&none, "items").is_empty());
    rig.google.put_event(
        "cal-work",
        "ev-new",
        json!({"summary": "New", "start": {"date": "2026-10-09"}, "end": {"date": "2026-10-10"}}),
    );
    rig.google.remove_event("cal-work", "ev-review");
    let (_, changed) = get(&rig, &incremental(&token)).await;
    let mut changed_ids = ids(&changed, "items");
    changed_ids.sort();
    assert_eq!(changed_ids, ["ev-new", "ev-review"]);
    let stub = changed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|e| e["id"] == "ev-review")
        .expect("stub");
    assert_eq!(stub["status"], "cancelled");
    let live = rig.google.events("cal-work");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].1["iCalUID"], "ev-new@google.com");

    // A listing from the start has no stubs; asking for them gives the cancelled one.
    let (_, fresh) = get(&rig, "/calendar/v3/calendars/cal-work/events").await;
    assert_eq!(ids(&fresh, "items"), ["ev-new"]);
    let (_, with) = get(
        &rig,
        "/calendar/v3/calendars/cal-work/events?showDeleted=true",
    )
    .await;
    assert_eq!(ids(&with, "items").len(), 2);

    rig.google.expire_sync_tokens();
    let (status, body) = get(&rig, &incremental(&token)).await;
    assert_eq!(status, 410, "{body}");
    assert_eq!(body["error"]["errors"][0]["reason"], "fullSyncRequired");
    let (status, _) = get(
        &rig,
        "/calendar/v3/calendars/cal-work/events?syncToken=junk",
    )
    .await;
    assert_eq!(status, 410);
    // A new full listing hands out a token that works.
    let (_, again) = get(&rig, "/calendar/v3/calendars/cal-work/events").await;
    let fresh_token = again["nextSyncToken"].as_str().expect("token").to_owned();
    assert_eq!(get(&rig, &incremental(&fresh_token)).await.0, 200);
}

#[tokio::test]
async fn contacts_list_with_a_sync_token_and_a_stale_one_is_expired_sync_token() {
    let rig = rig().await;
    rig.google.seed_people();
    let all = "/v1/people/me/connections?personFields=names&requestSyncToken=true";
    let (status, first) = get(&rig, all).await;
    assert_eq!(status, 200);
    assert_eq!(ids(&first, "connections"), ["people/c1001", "people/c1002"]);
    let token = first["nextSyncToken"].as_str().expect("token").to_owned();
    let without = get(&rig, "/v1/people/me/connections?personFields=names")
        .await
        .1;
    assert!(without.get("nextSyncToken").is_none());

    rig.google.remove_person("people/c1002");
    rig.google
        .put_person("people/c1003", json!({"names": [{"displayName": "Linus"}]}));
    let (_, changed) = get(&rig, &format!("{all}&syncToken={token}")).await;
    let connections = changed["connections"].as_array().expect("connections");
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[0]["metadata"]["deleted"], true);
    assert_eq!(connections[1]["resourceName"], "people/c1003");
    assert_eq!(rig.google.people().len(), 2);
    let (status, one) = get(&rig, "/v1/people/c1003?personFields=names").await;
    assert_eq!(
        (status, one["names"][0]["displayName"].as_str()),
        (200, Some("Linus"))
    );

    rig.google.expire_sync_tokens();
    let (status, body) = get(&rig, &format!("{all}&syncToken={token}")).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["details"][0]["reason"], "EXPIRED_SYNC_TOKEN");
}

#[tokio::test]
async fn task_lists_list_and_tasks_filter_by_updated_min_and_show_deleted() {
    let rig = rig().await;
    rig.google.seed_tasks();
    let (_, lists) = get(&rig, "/tasks/v1/users/@me/lists").await;
    assert_eq!(ids(&lists, "items"), ["list-home", "list-work"]);
    assert_eq!(lists["items"][0]["title"], "Home");

    let (_, all) = get(&rig, "/tasks/v1/lists/list-home/tasks?showHidden=true").await;
    assert_eq!(ids(&all, "items"), ["t-milk", "t-eggs"]);
    let newest = all["items"][1]["updated"]
        .as_str()
        .expect("updated")
        .to_owned();
    assert!(newest.ends_with("Z") && all["items"][1]["parent"] == "t-milk");

    // `updatedMin` is inclusive: the newest comes again, nothing older does.
    let since = format!(
        "/tasks/v1/lists/list-home/tasks?showDeleted=true&showHidden=true&updatedMin={newest}"
    );
    assert_eq!(ids(&get(&rig, &since).await.1, "items"), ["t-eggs"]);
    rig.google
        .put_task("list-home", "t-milk", json!({"title": "Buy oat milk"}));
    rig.google.remove_task("list-home", "t-eggs");
    let (_, changed) = get(&rig, &since).await;
    let got: Vec<(String, Value)> = changed["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap_or("").to_owned(),
                t["deleted"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("t-milk".to_owned(), Value::Null),
            ("t-eggs".to_owned(), json!(true))
        ]
    );
    // Without `showDeleted` the deleted one is not listed.
    let (_, live) = get(&rig, "/tasks/v1/lists/list-home/tasks").await;
    assert_eq!(ids(&live, "items"), ["t-milk"]);
    assert_eq!(rig.google.tasks("list-home").len(), 1);
    let (status, one) = get(&rig, "/tasks/v1/lists/list-home/tasks/t-milk").await;
    assert_eq!((status, one["title"].as_str()), (200, Some("Buy oat milk")));

    // Hidden tasks need `showHidden`; completed ones can be left out.
    rig.google.put_task(
        "list-work",
        "t-old",
        json!({"title": "Old", "status": "completed", "hidden": true}),
    );
    let (_, plain) = get(&rig, "/tasks/v1/lists/list-work/tasks").await;
    assert_eq!(ids(&plain, "items"), ["t-report"]);
    let (_, hidden) = get(&rig, "/tasks/v1/lists/list-work/tasks?showHidden=true").await;
    assert_eq!(ids(&hidden, "items"), ["t-report", "t-old"]);
    let (_, open) = get(&rig, "/tasks/v1/lists/list-work/tasks?showCompleted=false").await;
    assert!(ids(&open, "items").is_empty());
    let (page, _) = get(&rig, "/tasks/v1/lists/nope/tasks").await;
    assert_eq!(page, 404);
}

#[tokio::test]
async fn with_no_content_set_the_probe_paths_stay_the_issuers_to_admit() {
    let rig = rig().await;
    for path in [
        "/calendar/v3/users/me/calendarList",
        "/tasks/v1/users/@me/lists",
    ] {
        let response = get_as(&rig, path, Some(TOKEN)).await;
        // The probe answers a token the issuer minted; a bare fixed token is no issuer's.
        assert_eq!(response.status, 401, "{path}");
    }
}
