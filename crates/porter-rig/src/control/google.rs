//! The control routes of the fake Google's Calendar, People and Tasks (`/google/...`): seeds and
//! remote edits, as another device of the account would make them. The bodies of the put routes
//! are the API's own JSON (a Google event, a Person, a Task).

use super::{Levers, absent, ok, refused};
use porter_fake_servers::GoogleHandle;
use porter_fake_servers::http::{Request, Response};
use serde_json::{Value, json};

fn json_body(request: &Request) -> Result<Value, Response> {
    serde_json::from_slice(&request.body).map_err(|_| refused(400, "the body is not JSON"))
}

/// The value of query parameter `name`, or the `400` that says it is missing.
fn need(request: &Request, name: &str) -> Result<String, Response> {
    request
        .query_value(name)
        .ok_or_else(|| refused(400, &format!("{name} is required")))
}

fn run(google: &GoogleHandle, request: &Request) -> Result<Response, Response> {
    let q = |name: &str| need(request, name);
    Ok(match (request.method.as_str(), request.path()) {
        ("POST", "/google/seed") => {
            let what = request.query_value("what").unwrap_or_else(|| "all".into());
            let all = what == "all";
            let known = ["calendars", "people", "tasks"];
            if !all && !known.contains(&what.as_str()) {
                return Err(refused(400, "what is calendars, people, tasks or all"));
            }
            if all || what == "calendars" {
                google.seed_calendars();
            }
            if all || what == "people" {
                google.seed_people();
            }
            if all || what == "tasks" {
                google.seed_tasks();
            }
            ok(json!({ "seeded": what }))
        }
        ("POST", "/google/calendar") => {
            let id = q("id")?;
            let name = request.query_value("name").unwrap_or_else(|| id.clone());
            google.set_calendar(
                &id,
                &name,
                &request.query_value("color").unwrap_or_default(),
            );
            ok(json!({ "id": id }))
        }
        ("POST", "/google/calendar-remove") => {
            let id = q("id")?;
            google.remove_calendar(&id);
            ok(json!({ "id": id }))
        }
        ("POST", "/google/event") => {
            let (calendar, id) = (q("calendar")?, q("id")?);
            google.put_event(&calendar, &id, json_body(request)?);
            ok(json!({ "calendar": calendar, "id": id }))
        }
        ("POST", "/google/event-remove") => {
            let (calendar, id) = (q("calendar")?, q("id")?);
            google.remove_event(&calendar, &id);
            ok(json!({ "calendar": calendar, "id": id }))
        }
        ("POST", "/google/person") => {
            let resource = q("resource")?;
            google.put_person(&resource, json_body(request)?);
            ok(json!({ "resource": resource }))
        }
        ("POST", "/google/person-remove") => {
            let resource = q("resource")?;
            google.remove_person(&resource);
            ok(json!({ "resource": resource }))
        }
        ("POST", "/google/task-list") => {
            let id = q("id")?;
            let title = request.query_value("title").unwrap_or_else(|| id.clone());
            google.set_task_list(&id, &title);
            ok(json!({ "id": id }))
        }
        ("POST", "/google/task-list-remove") => {
            let id = q("id")?;
            google.remove_task_list(&id);
            ok(json!({ "id": id }))
        }
        ("POST", "/google/task") => {
            let (list, id) = (q("list")?, q("id")?);
            google.put_task(&list, &id, json_body(request)?);
            ok(json!({ "list": list, "id": id }))
        }
        ("POST", "/google/task-remove") => {
            let (list, id) = (q("list")?, q("id")?);
            google.remove_task(&list, &id);
            ok(json!({ "list": list, "id": id }))
        }
        ("POST", "/google/expire-sync") => {
            google.expire_sync_tokens();
            ok(json!({}))
        }
        ("GET", "/google/hits") => {
            let hits: Vec<Value> = google
                .hits()
                .iter()
                .map(|h| {
                    json!({
                        "method": h.method, "target": h.target, "status": h.status,
                        "bearer": h.authorization.is_some(),
                    })
                })
                .collect();
            ok(json!({ "hits": hits }))
        }
        _ => refused(404, "no such lever"),
    })
}

impl Levers {
    /// Answers a `/google/...` request.
    pub(super) fn google_lever(&self, request: &Request) -> Response {
        match self.google.as_ref() {
            None => absent("the fake Google"),
            Some(google) => run(google, request).unwrap_or_else(|refusal| refusal),
        }
    }
}
