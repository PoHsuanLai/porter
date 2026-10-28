//! CalDAV (RFC 4791): a calendar is a collection under the account's calendar home, and an event a
//! resource in it holding the event's iCalendar text.
//!
//! An event put here is named for its UID (`<uid>.ics`, percent-encoded), so it is found again
//! with one `GET`. One the store got some other way (another client's, the server's own from an
//! invitation) is found by a `calendar-query` on its `UID` (§7.8). Every write is conditional: a
//! replacement carries the etag it was read at (`If-Match`), a new one `If-None-Match: *`, so a
//! change made elsewhere since is never overwritten blind (`412` is [`CalendarError::Changed`]).

use crate::CalendarStore;
use crate::error::CalendarError;
use crate::event::{CalendarRef, Event, Stored};
use crate::json::{encode, if_match, removed, url, with_header};
use dav_proto::discover::{Error as WalkError, Home, Next, Reach, Scope, Walk};
use dav_proto::names::{CALDAV, CALENDAR_DATA, GETETAG};
use dav_proto::{Depth, escape, parse_multistatus};
use porter_http::http::header::{ACCEPT, CONTENT_TYPE, IF_NONE_MATCH};
use porter_http::http::{Method, Request, Response};
use porter_http::{HeaderText, Http, HttpError, request, uri, webdav};
use porter_ids::WebUrl;

/// A CalDAV account's calendars, under its calendar home.
#[derive(Debug, Clone)]
pub struct CalDav<H> {
    http: H,
    home: WebUrl,
}

impl<H: Http> CalDav<H> {
    /// The calendars under `home`, the account's calendar home set.
    pub fn new(http: H, home: WebUrl) -> Self {
        Self { http, home }
    }

    /// The calendar home found from `start`, any URL on the server: dav-proto's discovery walk
    /// asks it for the principal (RFC 5397) and the principal's `calendar-home-set` (RFC 4791
    /// §6.2.1).
    pub async fn discover(http: H, start: WebUrl) -> Result<Self, CalendarError> {
        let found = walk(&http, &start, Reach::Homes).await?;
        let home = found
            .homes_of(Home::Calendar)
            .first()
            .map(|home| url(&home.to_string()))
            .transpose()?
            .ok_or_else(|| CalendarError::Malformed("no calendar-home-set".to_owned()))?;
        Ok(Self { http, home })
    }

    /// The calendar home.
    pub fn home(&self) -> &WebUrl {
        &self.home
    }

    /// The resource an event put here with `uid` is named by in `calendar`.
    fn resource(calendar: &str, uid: &str) -> Result<WebUrl, CalendarError> {
        let slash = if calendar.ends_with('/') { "" } else { "/" };
        url(&format!("{calendar}{slash}{}.ics", encode(uid)))
    }

    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, CalendarError> {
        Ok(self.http.send(request).await?)
    }

    /// The event at `at`, by `GET`; `None` when there is nothing there.
    async fn get(
        &self,
        calendar: &str,
        uid: &str,
        at: WebUrl,
    ) -> Result<Option<Stored>, CalendarError> {
        let asked = request(Method::GET, &at)
            .header(ACCEPT, "text/calendar")
            .body(Vec::new())
            .map_err(HttpError::from)?;
        let response = self.send(asked).await?;
        match response.status().as_u16() {
            200..=299 => Ok(Some(Stored {
                calendar: calendar.to_owned(),
                uid: uid.to_owned(),
                id: at.as_str().to_owned(),
                version: response.header("etag").map(str::to_owned),
                ics: Some(String::from_utf8_lossy(response.body()).into_owned()),
            })),
            404 | 410 => Ok(None),
            status => Err(CalendarError::of_status(status, "the GET of an event")),
        }
    }

    /// The event with `uid` in `calendar` by a `calendar-query`, wherever the server keeps it. A
    /// server that does not answer the query (or not with a multistatus) has none to give.
    async fn query(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        let at = url(calendar)?;
        let asked = request(webdav::report(), &at)
            .header("Depth", Depth::One.as_str())
            .header(CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(calendar_query(uid).into_bytes())
            .map_err(HttpError::from)?;
        let response = self.send(asked).await?;
        if response.status().as_u16() != 207 {
            return match response.status().as_u16() {
                401 | 403 => Err(CalendarError::Unauthorized),
                _ => Ok(None),
            };
        }
        let status = parse_multistatus(&String::from_utf8_lossy(response.body()))
            .map_err(|e| CalendarError::Malformed(e.to_string()))?;
        let Some(found) = status.responses.iter().find(|r| {
            !r.gone()
                && r.found(CALENDAR_DATA)
                    .is_some_and(|ics| names_uid(ics, uid))
        }) else {
            return Ok(None);
        };
        Ok(Some(Stored {
            calendar: calendar.to_owned(),
            uid: uid.to_owned(),
            id: resolve(&at, &found.href)?.as_str().to_owned(),
            version: found.found(GETETAG).map(str::to_owned),
            ics: found.found(CALENDAR_DATA).map(str::to_owned),
        }))
    }
}

impl<H: Http> CalendarStore for CalDav<H> {
    async fn calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let found = walk(&self.http, &self.home, Reach::Collections).await?;
        found
            .collections
            .iter()
            .filter(|collection| collection.home == Home::Calendar)
            .map(|collection| {
                let id = url(&collection.url.to_string())?;
                let name = collection
                    .display_name
                    .clone()
                    .unwrap_or_else(|| last_segment(id.path()));
                Ok(CalendarRef {
                    id: id.as_str().to_owned(),
                    name,
                    color: collection.color.clone(),
                    writable: true,
                })
            })
            .collect()
    }

    async fn find(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        if let Some(found) = self
            .get(calendar, uid, Self::resource(calendar, uid)?)
            .await?
        {
            return Ok(Some(found));
        }
        self.query(calendar, uid).await
    }

    async fn put(&self, calendar: &str, event: &Event) -> Result<Stored, CalendarError> {
        let held = self.find(calendar, &event.uid).await?;
        let at = match &held {
            Some(held) => url(&held.id)?,
            None => Self::resource(calendar, &event.uid)?,
        };
        let asked = request(Method::PUT, &at)
            .header(CONTENT_TYPE, "text/calendar; charset=utf-8")
            .body(event.ics.clone().into_bytes())
            .map_err(HttpError::from)?;
        let asked = match held.as_ref().map(|held| held.version.as_deref()) {
            Some(Some(etag)) => if_match(asked, etag)?,
            Some(None) => asked,
            None => with_header(asked, IF_NONE_MATCH, "*")?,
        };
        let response = self.send(asked).await?;
        if !response.status().is_success() {
            return Err(CalendarError::of_status(
                response.status().as_u16(),
                "the PUT of an event",
            ));
        }
        Ok(Stored {
            calendar: calendar.to_owned(),
            uid: event.uid.clone(),
            id: at.as_str().to_owned(),
            version: response.header("etag").map(str::to_owned),
            ics: Some(event.ics.clone()),
        })
    }

    async fn remove(&self, calendar: &str, uid: &str) -> Result<bool, CalendarError> {
        let Some(held) = self.find(calendar, uid).await? else {
            return Ok(false);
        };
        let mut asked = request(Method::DELETE, &url(&held.id)?)
            .body(Vec::new())
            .map_err(HttpError::from)?;
        if let Some(etag) = &held.version {
            asked = if_match(asked, etag)?;
        }
        let response = self.send(asked).await?;
        removed(&response, "the DELETE of an event")
    }
}

/// What dav-proto's discovery walk finds from `start` as far as `reach`, each request sent
/// through the seam (any host the server points to: iCloud keeps its homes on another).
async fn walk<H: Http>(
    http: &H,
    start: &WebUrl,
    reach: Reach,
) -> Result<dav_proto::discover::Found, CalendarError> {
    let (mut walk, first) = Walk::start(uri(start)?, reach, &[Home::Calendar], Scope::Anywhere);
    let mut reply = http.send(first).await?;
    loop {
        match walk.advance(&reply).map_err(walk_error)? {
            Next::Done(found) => return Ok(found),
            Next::Ask(request) => reply = http.send(request).await?,
        }
    }
}

/// What a reply that ended the walk means.
fn walk_error(error: WalkError) -> CalendarError {
    match error {
        WalkError::Unauthorized => CalendarError::Unauthorized,
        WalkError::Status(status) => CalendarError::of_status(status, "the discovery of calendars"),
        other => CalendarError::Malformed(other.to_string()),
    }
}

/// An href from a reply, as a URL: a path against `base`'s origin, a full URL as it is.
fn resolve(base: &WebUrl, href: &str) -> Result<WebUrl, CalendarError> {
    if href.starts_with("http://") || href.starts_with("https://") {
        return url(href);
    }
    let whole = base.as_str();
    let after = whole.find("://").map_or(0, |at| at + 3);
    let origin = &whole[..whole[after..]
        .find('/')
        .map_or(whole.len(), |at| after + at)];
    let path = if href.starts_with('/') {
        href.to_owned()
    } else {
        let dir = base.path().rsplit_once('/').map_or("", |(dir, _)| dir);
        format!("{dir}/{href}")
    };
    url(&format!("{origin}{path}"))
}

/// Whether the iCalendar text has a `UID` property equal to `uid`. A `text-match` is a substring
/// match (RFC 4791 §9.7.5), so the server's answer to a query for `a1` includes the event `a1b`,
/// and only an exact reading of what comes back says which one is the event asked for.
fn names_uid(ics: &str, uid: &str) -> bool {
    ics.replace("\r\n ", "")
        .replace("\r\n\t", "")
        .lines()
        .any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.split(';')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("UID"))
                    && value == uid
            })
        })
}

/// The last segment of a collection's path, for a calendar with no display name.
fn last_segment(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A `calendar-query` for the event whose `UID` is `uid`, compared octet by octet (RFC 4791
/// §7.8, §9.7.5), asking for its etag and text.
fn calendar_query(uid: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="{CALDAV}"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:prop-filter name="UID"><c:text-match collation="i;octet">{}</c:text-match></c:prop-filter></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#,
        escape(uid)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_href_is_resolved_against_the_url_it_came_from() {
        let base = url("https://dav.example.test/cal/home/").unwrap();
        let cases = [
            ("/cal/work/", "https://dav.example.test/cal/work/"),
            ("work/", "https://dav.example.test/cal/home/work/"),
            (
                "https://other.example.test/x/",
                "https://other.example.test/x/",
            ),
        ];
        for (href, want) in cases {
            assert_eq!(resolve(&base, href).unwrap().as_str(), want, "{href}");
        }
    }

    #[test]
    fn an_event_is_named_for_its_uid() {
        let at =
            CalDav::<NoHttp>::resource("https://dav.example.test/cal/work/", "a b@host").unwrap();
        assert_eq!(
            at.as_str(),
            "https://dav.example.test/cal/work/a%20b%40host.ics"
        );
        let at = CalDav::<NoHttp>::resource("https://dav.example.test/cal/work", "x").unwrap();
        assert_eq!(at.as_str(), "https://dav.example.test/cal/work/x.ics");
    }

    #[test]
    fn only_a_uid_property_that_is_equal_names_the_event() {
        let ics = "BEGIN:VEVENT\r\nUID:a1b@host\r\nSUMMARY:UID:a1@host\r\nEND:VEVENT\r\n";
        assert!(names_uid(ics, "a1b@host"));
        assert!(
            !names_uid(ics, "a1@host"),
            "a longer UID, and text that only looks like one"
        );
        assert!(
            names_uid("UID;X-A=b:long\r\n er\r\n", "longer"),
            "folded, with a parameter"
        );
    }

    #[test]
    fn the_query_names_the_uid_escaped() {
        let body = calendar_query("a<b>&c");
        assert!(body.contains("<c:prop-filter name=\"UID\">"), "{body}");
        assert!(body.contains(">a&lt;b&gt;&amp;c<"), "{body}");
    }

    /// A seam nothing is sent through: the naming test needs a type, not a server.
    #[derive(Debug)]
    struct NoHttp;

    impl Http for NoHttp {
        async fn send(&self, _request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
            Err(HttpError::Unreachable)
        }
    }
}
