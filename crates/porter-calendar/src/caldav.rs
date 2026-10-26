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
use crate::json::{encode, url};
use dav_proto::names::{CALDAV, CALENDAR_DATA, DISPLAYNAME, GETETAG, RESOURCETYPE};
use dav_proto::{Depth, Multistatus, escape, parse_multistatus};
use porter_core::WebUrl;
use porter_dav::{Home, collections, current_user_principal, home_set, propfind};
use porter_http::{Http, HttpRequest, HttpResponse, Method};

/// Apple's calendar colour, which most servers keep (`#0082c9FF`).
const CALENDAR_COLOR: &str = "http://apple.com/ns/ical/calendar-color";

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

    /// The calendar home found from `start`, any URL on the server the principal's
    /// `current-user-principal` is asked at (RFC 5397), then the principal's
    /// `calendar-home-set` (RFC 4791 §6.2.1).
    pub async fn discover(http: H, start: WebUrl) -> Result<Self, CalendarError> {
        let at = propfind_status(
            &http,
            start.clone(),
            &[dav_proto::names::CURRENT_USER_PRINCIPAL],
        )
        .await?;
        let principal = current_user_principal(&at)
            .map(|href| resolve(&start, &href))
            .transpose()?
            .unwrap_or(start);
        let at = propfind_status(
            &http,
            principal.clone(),
            &[dav_proto::names::CALENDAR_HOME_SET],
        )
        .await?;
        let home = home_set(&at, Home::Calendar)
            .first()
            .map(|href| resolve(&principal, href))
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

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, CalendarError> {
        Ok(self.http.send(request).await?)
    }

    /// The event at `at`, by `GET`; `None` when there is nothing there.
    async fn get(
        &self,
        calendar: &str,
        uid: &str,
        at: WebUrl,
    ) -> Result<Option<Stored>, CalendarError> {
        let response = self
            .send(HttpRequest::new(Method::Get, at.clone()).with_header("Accept", "text/calendar"))
            .await?;
        match response.status.0 {
            200..=299 => Ok(Some(Stored {
                calendar: calendar.to_owned(),
                uid: uid.to_owned(),
                id: at.as_str().to_owned(),
                version: response.header("etag").map(str::to_owned),
                ics: Some(String::from_utf8_lossy(&response.body).into_owned()),
            })),
            404 | 410 => Ok(None),
            status => Err(CalendarError::of_status(status, "the GET of an event")),
        }
    }

    /// The event with `uid` in `calendar` by a `calendar-query`, wherever the server keeps it. A
    /// server that does not answer the query (or not with a multistatus) has none to give.
    async fn query(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        let at = url(calendar)?;
        let request = HttpRequest::new(Method::Report, at.clone())
            .with_header("Depth", Depth::One.as_str())
            .with_header("Content-Type", "application/xml; charset=utf-8")
            .with_body(calendar_query(uid));
        let response = self.send(request).await?;
        if response.status.0 != 207 {
            return match response.status.0 {
                401 | 403 => Err(CalendarError::Unauthorized),
                _ => Ok(None),
            };
        }
        let status = parse_multistatus(&String::from_utf8_lossy(&response.body))
            .map_err(|e| CalendarError::Malformed(e.to_string()))?;
        let Some(found) = status
            .responses
            .iter()
            .find(|r| !r.gone() && r.found(CALENDAR_DATA).is_some())
        else {
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
        let request = propfind(
            self.home.clone(),
            Depth::One,
            &[RESOURCETYPE, DISPLAYNAME, CALENDAR_COLOR],
        );
        let response = self.send(request).await?;
        let status = multistatus(&response, "the PROPFIND of the calendar home")?;
        let hrefs = collections(&status, Home::Calendar);
        let mut out = Vec::new();
        for response in status.responses.iter().filter(|r| hrefs.contains(&r.href)) {
            let id = resolve(&self.home, &response.href)?;
            let name = response
                .found(DISPLAYNAME)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| last_segment(id.path()));
            out.push(CalendarRef {
                id: id.as_str().to_owned(),
                name,
                color: response
                    .found(CALENDAR_COLOR)
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned),
                writable: true,
            });
        }
        Ok(out)
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
        let (at, condition) = match &held {
            Some(held) => (
                url(&held.id)?,
                held.version.as_ref().map(|etag| ("If-Match", etag.clone())),
            ),
            None => (
                Self::resource(calendar, &event.uid)?,
                Some(("If-None-Match", "*".to_owned())),
            ),
        };
        let mut request = HttpRequest::new(Method::Put, at.clone())
            .with_header("Content-Type", "text/calendar; charset=utf-8")
            .with_body(event.ics.clone());
        if let Some((name, value)) = condition {
            request = request.with_header(name, value);
        }
        let response = self.send(request).await?;
        if !response.status.is_success() {
            return Err(CalendarError::of_status(
                response.status.0,
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
        let mut request = HttpRequest::new(Method::Delete, url(&held.id)?);
        if let Some(etag) = &held.version {
            request = request.with_header("If-Match", etag.clone());
        }
        let response = self.send(request).await?;
        match response.status.0 {
            200..=299 => Ok(true),
            404 | 410 => Ok(false),
            status => Err(CalendarError::of_status(status, "the DELETE of an event")),
        }
    }
}

/// A PROPFIND of `props` at depth 0, read.
async fn propfind_status<H: Http>(
    http: &H,
    at: WebUrl,
    props: &[&str],
) -> Result<Multistatus, CalendarError> {
    let response = http.send(propfind(at, Depth::Zero, props)).await?;
    multistatus(&response, "a PROPFIND")
}

/// A `207` response's multistatus.
fn multistatus(response: &HttpResponse, what: &'static str) -> Result<Multistatus, CalendarError> {
    if response.status.0 != 207 {
        return Err(CalendarError::of_status(response.status.0, what));
    }
    parse_multistatus(&String::from_utf8_lossy(&response.body))
        .map_err(|e| CalendarError::Malformed(e.to_string()))
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
    fn the_query_names_the_uid_escaped() {
        let body = calendar_query("a<b>&c");
        assert!(body.contains("<c:prop-filter name=\"UID\">"), "{body}");
        assert!(body.contains(">a&lt;b&gt;&amp;c<"), "{body}");
    }

    /// A seam nothing is sent through: the naming test needs a type, not a server.
    #[derive(Debug)]
    struct NoHttp;

    impl Http for NoHttp {
        async fn send(
            &self,
            _request: HttpRequest,
        ) -> Result<HttpResponse, porter_http::HttpError> {
            Err(porter_http::HttpError::Unreachable)
        }
    }
}
