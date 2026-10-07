//! The PIM side of the fake Google: Calendar, People and Tasks behind the same bearer rule as
//! the probes (an access token the issuer minted and has not revoked, with the service's scope;
//! or, for a test that has no issuer to sign in to, a token it was told to accept). The paths
//! are Google's own below each API's base (`/calendar/v3`, `/v1/people`, `/tasks/v1`), so a
//! provider file rewritten by `rewrite` keeps each row's path.
//!
//! What each API lists, how its changes are asked for and how a stale token is refused is in the
//! three modules; this one routes, authenticates and gives the test its levers.

use super::calendar::Calendars;
use super::people::People;
use super::tasks::Tasks;
use super::{GoogleHandle, Shared};
use crate::http::{Request, Response};
use crate::seen::lock;
use serde_json::{Value, json};

/// The scope of each API (any one of the listed is enough).
const CALENDAR: &[&str] = &["https://www.googleapis.com/auth/calendar"];
const CONTACTS: &[&str] = &["https://www.googleapis.com/auth/contacts"];
const TASKS: &[&str] = &["https://www.googleapis.com/auth/tasks"];

/// Everything the PIM APIs hold.
#[derive(Debug, Default)]
pub struct Pim {
    calendars: Calendars,
    people: People,
    tasks: Tasks,
}

/// A Google error body.
pub(super) fn error(status: u16, reason: &str) -> Response {
    Response::json(
        status,
        &json!({"error": {"code": status, "message": reason, "errors": [{"reason": reason}]}}),
    )
}

/// The numbers a token is made of: `prefix`, then numbers joined with `.`.
pub(super) fn numbers(token: &str, prefix: &str) -> Option<Vec<u64>> {
    let rest = token.strip_prefix(prefix)?;
    rest.split('.').map(|n| n.parse().ok()).collect()
}

/// The `updated` time of change number `seq`: one second after the last, from noon on a fixed
/// day, in Google's `YYYY-MM-DDTHH:MM:SS.sssZ` form (which orders as text).
pub(super) fn stamp(seq: u64) -> String {
    let at = 12 * 3_600 + seq;
    format!(
        "2026-10-05T{:02}:{:02}:{:02}.000Z",
        at / 3_600 % 24,
        at % 3_600 / 60,
        at % 60
    )
}

/// The scopes the API a path belongs to wants, and the path below its base.
fn api_of(path: &str) -> Option<(&'static [&'static str], &str, Api)> {
    if let Some(rest) = path.strip_prefix("/calendar/v3") {
        return Some((CALENDAR, rest, Api::Calendar));
    }
    if let Some(rest) = path.strip_prefix("/tasks/v1") {
        return Some((TASKS, rest, Api::Tasks));
    }
    path.strip_prefix("/v1")
        .filter(|rest| rest.starts_with("/people/"))
        .map(|rest| (CONTACTS, rest, Api::People))
}

#[derive(Debug, Clone, Copy)]
enum Api {
    Calendar,
    People,
    Tasks,
}

/// Answers a PIM request, or `None` when the path is not one (or is a probe this fake still
/// answers with its one calendar or list because none was set).
pub(super) fn answer(shared: &Shared, request: &Request) -> Option<Response> {
    let (scopes, rest, api) = api_of(request.path())?;
    let (token_ok, issued) = {
        let state = lock(&shared.state);
        let presented = request.bearer();
        let fixed = presented.is_some_and(|t| state.fixed.iter().any(|f| f == t));
        match (api, request.path()) {
            (Api::Calendar, _)
                if rest == "/users/me/calendarList" && state.pim.calendars.is_empty() =>
            {
                return None;
            }
            (Api::Tasks, _) if rest == "/users/@me/lists" && state.pim.tasks.is_empty() => {
                return None;
            }
            _ => {}
        }
        (
            fixed,
            presented
                .filter(|t| {
                    shared
                        .issuer
                        .as_ref()
                        .is_some_and(|issuer| issuer.access_is_live(t))
                })
                .is_some(),
        )
    };
    if !token_ok && !issued {
        return Some(Response::json(
            401,
            &json!({"error": {"code": 401, "status": "UNAUTHENTICATED"}}),
        ));
    }
    if !token_ok {
        let scoped_out = request
            .bearer()
            .and_then(|t| {
                shared
                    .issuer
                    .as_ref()
                    .and_then(|issuer| issuer.access_scope(t))
            })
            .is_some_and(|granted| !granted.split_whitespace().any(|s| scopes.contains(&s)));
        if scoped_out {
            return Some(error(403, "insufficientPermissions"));
        }
    }
    let state = lock(&shared.state);
    let response = match api {
        Api::Calendar => state.pim.calendars.answer(rest, request),
        Api::People => state.pim.people.answer(rest, request),
        Api::Tasks => state.pim.tasks.answer(rest, request),
    };
    Some(response.unwrap_or_else(|| error(404, "notFound")))
}

impl GoogleHandle {
    fn pim<R>(&self, with: impl FnOnce(&mut Pim) -> R) -> R {
        with(&mut lock(&self.shared.state).pim)
    }

    /// Drops every sync token handed out so far: Calendar answers `410`, People `400
    /// EXPIRED_SYNC_TOKEN`. Tasks has no token to drop.
    pub fn expire_sync_tokens(&self) {
        self.pim(|pim| {
            pim.calendars.expire();
            pim.people.expire();
        });
    }

    /// Fills the calendars with a small set (see [`Calendars::seed`]).
    pub fn seed_calendars(&self) {
        self.pim(|pim| pim.calendars.seed());
    }

    /// Adds the calendar `id`, or renames and recolours it.
    pub fn set_calendar(&self, id: &str, summary: &str, color: &str) {
        self.pim(|pim| pim.calendars.set_calendar(id, summary, color));
    }

    /// Removes a calendar and its events.
    pub fn remove_calendar(&self, id: &str) {
        self.pim(|pim| pim.calendars.remove_calendar(id));
    }

    /// Creates or replaces the event `id` of a calendar with this Google event JSON (the fake adds
    /// `id`, `etag`, `updated`, `status`, `iCalUID`), as another device of the account would.
    pub fn put_event(&self, calendar: &str, id: &str, event: Value) {
        self.pim(|pim| pim.calendars.put_event(calendar, id, event));
    }

    /// Deletes an event; the next incremental listing reports it cancelled.
    pub fn remove_event(&self, calendar: &str, id: &str) {
        self.pim(|pim| pim.calendars.remove_event(calendar, id));
    }

    /// The live events of a calendar: id and Google JSON.
    pub fn events(&self, calendar: &str) -> Vec<(String, Value)> {
        self.pim(|pim| pim.calendars.events(calendar))
    }

    /// Fills the contacts with two people (see [`People::seed`]).
    pub fn seed_people(&self) {
        self.pim(|pim| pim.people.seed());
    }

    /// Creates or replaces the contact `resource` (`people/c1001`) with this Person JSON.
    pub fn put_person(&self, resource: &str, person: Value) {
        self.pim(|pim| pim.people.put(resource, person));
    }

    /// Deletes a contact; the next incremental listing reports it `metadata.deleted`.
    pub fn remove_person(&self, resource: &str) {
        self.pim(|pim| pim.people.remove(resource));
    }

    /// The live contacts: resource name and Person JSON.
    pub fn people(&self) -> Vec<(String, Value)> {
        self.pim(|pim| pim.people.all())
    }

    /// Fills the task lists with two lists and three tasks (see [`Tasks::seed`]).
    pub fn seed_tasks(&self) {
        self.pim(|pim| pim.tasks.seed());
    }

    /// Adds the task list `id`, or renames it.
    pub fn set_task_list(&self, id: &str, title: &str) {
        self.pim(|pim| pim.tasks.set_list(id, title));
    }

    /// Removes a task list and its tasks.
    pub fn remove_task_list(&self, id: &str) {
        self.pim(|pim| pim.tasks.remove_list(id));
    }

    /// Creates or replaces the task `id` of a list with this Task JSON (the fake adds `id`,
    /// `etag`, `updated`, `status`).
    pub fn put_task(&self, list: &str, id: &str, task: Value) {
        self.pim(|pim| pim.tasks.put(list, id, task));
    }

    /// Deletes a task; it stays listable with `showDeleted`, `deleted: true`.
    pub fn remove_task(&self, list: &str, id: &str) {
        self.pim(|pim| pim.tasks.remove(list, id));
    }

    /// The live tasks of a list: id and Task JSON.
    pub fn tasks(&self, list: &str) -> Vec<(String, Value)> {
        self.pim(|pim| pim.tasks.tasks(list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_a_prefix_and_dotted_numbers() {
        assert_eq!(numbers("ep0.5.250.1", "ep"), Some(vec![0, 5, 250, 1]));
        assert_eq!(numbers("sync-12", "sync-"), Some(vec![12]));
        assert_eq!(numbers("sync-x", "sync-"), None);
        assert_eq!(numbers("psync-3", "pp"), None);
        assert_eq!(numbers("", "ep"), None);
    }

    #[test]
    fn a_stamp_orders_as_text_and_reads_as_rfc3339() {
        assert_eq!(stamp(0), "2026-10-05T12:00:00.000Z");
        assert_eq!(stamp(61), "2026-10-05T12:01:01.000Z");
        assert!(stamp(9) < stamp(10) && stamp(59) < stamp(60) && stamp(3_599) < stamp(3_600));
    }

    #[test]
    fn a_path_belongs_to_one_api() {
        assert!(matches!(
            api_of("/calendar/v3/users/me/calendarList"),
            Some((_, "/users/me/calendarList", Api::Calendar))
        ));
        assert!(matches!(
            api_of("/v1/people/me/connections"),
            Some((_, "/people/me/connections", Api::People))
        ));
        assert!(matches!(
            api_of("/tasks/v1/users/@me/lists"),
            Some((_, "/users/@me/lists", Api::Tasks))
        ));
        assert!(api_of("/v1/contactGroups").is_none() && api_of("/drive/v3/files").is_none());
        assert!(api_of("/v1/userinfo").is_none());
    }
}
