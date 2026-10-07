//! [`PhotosPicker`]: the Google Photos Picker API as a typed API for an app. Google Photos does
//! not let an app read the library; the person picks, in a page Google draws, and the app gets
//! what they picked:
//!
//! 1. [`PhotosPicker::start`] creates a session and answers the `pickerUri` the app opens in the
//!    browser, and how often to poll;
//! 2. [`PhotosPicker::poll`] says whether the person has finished (`mediaItemsSet`);
//! 3. [`PhotosPicker::import`] lists the picked items, downloads each into
//!    `<picked>/<session>/` and deletes the session.
//!
//! A picked item's `baseUrl` is fetched with `=d` (a photo's bytes) or `=dv` (a video's) added.
//! The names and the session id come from Google and are never trusted as paths: a session id is
//! a plain token or refused, a file name is cut to its last component and made unique.

use crate::datasets::photos::cas;
use porter_core::WebUrl;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

/// A Picker session's id: `[A-Za-z0-9_-]+`, the only thing that becomes a directory name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// The id, if it is one.
    pub fn parse(text: &str) -> Option<Self> {
        let ok = !text.is_empty()
            && text.len() <= 128
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        ok.then(|| Self(text.to_owned()))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A session just created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerSession {
    /// The session.
    pub id: SessionId,
    /// The page the app opens in the browser, where the person picks.
    pub picker_uri: String,
    /// How often to poll.
    pub poll_every: Duration,
    /// How long Google keeps asking before the session times out.
    pub times_out_in: Duration,
}

/// Whether the person has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerState {
    /// Still picking.
    Waiting,
    /// Done: the items can be listed.
    Picked,
}

/// What became of the session after an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// Deleted at Google.
    Deleted,
    /// The delete did not go through; Google drops it when it expires.
    Left,
}

/// What an import fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// The folder the files are in: `<picked>/<session>`.
    pub dir: PathBuf,
    /// The files, in the order the person's picks were listed.
    pub files: Vec<PathBuf>,
    /// The session's end.
    pub session: SessionEnd,
}

/// Why a Picker call did not give what it was for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PickerError {
    /// No answer.
    #[error("the Picker API was not reached")]
    Unreached,
    /// An HTTP status that was not a success.
    #[error("the Picker API answered {0}")]
    Refused(u16),
    /// An answer that was not the shape it should be.
    #[error("the Picker API answered something unreadable")]
    Unreadable,
    /// The person has not finished picking.
    #[error("the person has not finished picking")]
    NotYet,
    /// Google named a session that is not a plain token.
    #[error("the session id is not a plain token")]
    BadSession,
    /// A file could not be kept.
    #[error("cannot keep a picked file: {0}")]
    Disk(String),
}

#[derive(Deserialize)]
struct SessionWire {
    id: String,
    #[serde(rename = "pickerUri")]
    picker_uri: Option<String>,
    #[serde(rename = "pollingConfig")]
    polling: Option<Polling>,
    #[serde(default, rename = "mediaItemsSet")]
    set: bool,
}

#[derive(Deserialize)]
struct Polling {
    #[serde(rename = "pollInterval")]
    every: Option<String>,
    #[serde(rename = "timeoutIn")]
    timeout: Option<String>,
}

#[derive(Deserialize)]
struct ListWire {
    #[serde(default, rename = "mediaItems")]
    items: Vec<ItemWire>,
    #[serde(rename = "nextPageToken")]
    next: Option<String>,
}

#[derive(Deserialize)]
struct ItemWire {
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "mediaFile")]
    file: Option<FileWire>,
}

#[derive(Deserialize)]
struct FileWire {
    #[serde(rename = "baseUrl")]
    base: String,
    filename: Option<String>,
}

/// A query value with everything but unreserved characters percent-encoded.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `"1s"`, `"600.5s"` as a duration; anything else is `None`.
fn seconds(text: &str) -> Option<Duration> {
    let value: f64 = text.strip_suffix('s')?.parse().ok()?;
    (value.is_finite() && (0.0..=86_400.0).contains(&value)).then(|| Duration::from_secs_f64(value))
}

/// The name a picked file is kept under: its last path component without leading dots, or
/// `item`.
fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let last = last.trim_start_matches('.');
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect();
    match cleaned.is_empty() || cleaned.len() > 200 {
        true => "item".to_owned(),
        false => cleaned,
    }
}

/// `name`, or `name` with `-2`, `-3`... before its extension, whichever `taken` does not hold.
fn unique(name: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == name) {
        return name.to_owned();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2u32..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_default()
}

/// The Picker API at one origin, and where picked files go.
#[derive(Debug)]
pub struct PhotosPicker<H> {
    http: H,
    origin: String,
    picked: PathBuf,
}

impl<H: Http> PhotosPicker<H> {
    /// The Picker API served at the origin of `base` (`https://photospicker.googleapis.com/v1`),
    /// reached through `http` (which also fetches the items' `baseUrl`s), keeping what is picked
    /// below `picked`.
    pub fn new(http: H, base: &WebUrl, picked: PathBuf) -> Self {
        let text = base.as_str();
        let after = text.find("://").map_or(0, |at| at + 3);
        let end = text[after..]
            .find(['/', '?'])
            .map_or(text.len(), |at| after + at);
        Self {
            http,
            origin: text[..end].to_owned(),
            picked,
        }
    }

    fn url(&self, path: &str) -> Result<WebUrl, PickerError> {
        WebUrl::parse(&format!("{}/v1/{path}", self.origin)).map_err(|_| PickerError::Unreadable)
    }

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, PickerError> {
        let response = self
            .http
            .send(request.with_header("Accept", "application/json"))
            .await
            .map_err(|_| PickerError::Unreached)?;
        match response.status.0 {
            200 => Ok(response),
            status => Err(PickerError::Refused(status)),
        }
    }

    /// Opens a session for the person to pick in.
    pub async fn start(&self) -> Result<PickerSession, PickerError> {
        let request = HttpRequest::new(Method::Post, self.url("sessions")?)
            .with_header("Content-Type", "application/json")
            .with_body(b"{}".to_vec());
        let response = self.send(request).await?;
        let wire: SessionWire =
            serde_json::from_slice(&response.body).map_err(|_| PickerError::Unreadable)?;
        let id = SessionId::parse(&wire.id).ok_or(PickerError::BadSession)?;
        let polling = wire.polling;
        Ok(PickerSession {
            id,
            picker_uri: wire.picker_uri.ok_or(PickerError::Unreadable)?,
            poll_every: polling
                .as_ref()
                .and_then(|p| p.every.as_deref())
                .and_then(seconds)
                .unwrap_or(Duration::from_secs(5)),
            times_out_in: polling
                .as_ref()
                .and_then(|p| p.timeout.as_deref())
                .and_then(seconds)
                .unwrap_or(Duration::from_secs(600)),
        })
    }

    /// Whether the person has finished picking in `session`.
    pub async fn poll(&self, session: &SessionId) -> Result<PickerState, PickerError> {
        let request = HttpRequest::new(Method::Get, self.url(&format!("sessions/{session}"))?);
        let response = self.send(request).await?;
        let wire: SessionWire =
            serde_json::from_slice(&response.body).map_err(|_| PickerError::Unreadable)?;
        Ok(match wire.set {
            true => PickerState::Picked,
            false => PickerState::Waiting,
        })
    }

    /// Downloads what was picked in `session` into `<picked>/<session>/` and deletes the
    /// session. A person who has not finished is `NotYet` and nothing is touched.
    pub async fn import(&self, session: &SessionId) -> Result<Imported, PickerError> {
        if self.poll(session).await? != PickerState::Picked {
            return Err(PickerError::NotYet);
        }
        let dir = self.picked.join(session.as_str());
        let mut names: Vec<String> = Vec::new();
        let mut files = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut path = format!("mediaItems?sessionId={session}&pageSize=100");
            if let Some(token) = &page {
                path.push_str(&format!("&pageToken={}", encode(token)));
            }
            let response = self
                .send(HttpRequest::new(Method::Get, self.url(&path)?))
                .await?;
            let list: ListWire =
                serde_json::from_slice(&response.body).map_err(|_| PickerError::Unreadable)?;
            for item in list.items {
                let Some(file) = item.file else { continue };
                let suffix = match item.kind.as_deref() {
                    Some("VIDEO") => "=dv",
                    _ => "=d",
                };
                let url = WebUrl::parse(&format!("{}{suffix}", file.base))
                    .map_err(|_| PickerError::Unreadable)?;
                let bytes = self.send(HttpRequest::new(Method::Get, url)).await?.body;
                let wanted = safe_name(file.filename.as_deref().unwrap_or(&item.id));
                let name = unique(&wanted, &names);
                let target = dir.join(&name);
                cas::write_atomic(&target, &bytes).map_err(|e| PickerError::Disk(e.to_string()))?;
                names.push(name);
                files.push(target);
            }
            match list.next {
                Some(next) => page = Some(next),
                None => break,
            }
        }
        Ok(Imported {
            dir,
            files,
            session: self.end(session).await,
        })
    }

    /// Deletes `session` without importing (the person gave up).
    pub async fn cancel(&self, session: &SessionId) -> SessionEnd {
        self.end(session).await
    }

    async fn end(&self, session: &SessionId) -> SessionEnd {
        let Ok(url) = self.url(&format!("sessions/{session}")) else {
            return SessionEnd::Left;
        };
        match self.send(HttpRequest::new(Method::Delete, url)).await {
            Ok(_) => SessionEnd::Deleted,
            Err(_) => SessionEnd::Left,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_is_a_plain_token_or_nothing() {
        const CASES: &[(&str, bool)] = &[
            ("abc-DEF_123", true),
            ("", false),
            ("../x", false),
            ("a/b", false),
            ("a b", false),
            ("a?b=c", false),
        ];
        for (text, ok) in CASES {
            assert_eq!(SessionId::parse(text).is_some(), *ok, "{text:?}");
        }
    }

    #[test]
    fn google_durations_are_seconds_with_a_suffix() {
        const CASES: &[(&str, Option<Duration>)] = &[
            ("1s", Some(Duration::from_secs(1))),
            ("600.5s", Some(Duration::from_millis(600_500))),
            ("1", None),
            ("1m", None),
            ("-1s", None),
            ("999999s", None),
            ("", None),
        ];
        for (text, want) in CASES {
            assert_eq!(seconds(text), *want, "{text:?}");
        }
    }

    #[test]
    fn a_picked_name_is_one_plain_unique_file_name() {
        const NAMES: &[(&str, &str)] = &[
            ("IMG_1.jpg", "IMG_1.jpg"),
            ("../../etc/passwd", "passwd"),
            ("a\\b\\c.png", "c.png"),
            (".hidden", "hidden"),
            ("..", "item"),
            ("", "item"),
        ];
        for (name, want) in NAMES {
            assert_eq!(safe_name(name), *want, "{name:?}");
        }
        let taken = vec!["a.jpg".to_owned(), "a-2.jpg".to_owned(), "b".to_owned()];
        assert_eq!(unique("a.jpg", &taken), "a-3.jpg");
        assert_eq!(unique("b", &taken), "b-2");
        assert_eq!(unique("c.jpg", &taken), "c.jpg");
    }
}
