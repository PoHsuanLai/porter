//! Discovery (feature `discover`): where an account's calendars, address books and files are,
//! from an endpoint a person typed. [`Walk`] is the one state machine for it, with no IO: it
//! yields the next request, takes the reply, and ends with what it found, so the caller keeps
//! only the sending (and the credential every request needs).
//!
//! The walk is RFC 5397 and 4791/6352: PROPFIND the endpoint for `current-user-principal` and
//! the home sets; when the endpoint names no home, PROPFIND the principal for them; and, for
//! [`Reach::Collections`], PROPFIND each home at depth 1 for the calendars or address books in
//! it. The readers it uses ([`current_user_principal`], [`home_set`], [`collections`]) are
//! public for a caller that has a [`Multistatus`] already.
//!
//! ```
//! use dav_proto::discover::{Next, Reach, Scope, Walk};
//! use http::{Response, Uri};
//!
//! let endpoint: Uri = "https://dav.example.test/dav/".parse()?;
//! let (mut walk, first) = Walk::start(endpoint, Reach::Principal, &[], Scope::SameOrigin);
//! assert_eq!(first.method(), "PROPFIND");
//! let reply = Response::builder().status(207).body(
//!     br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/</d:href>
//!     <d:propstat><d:prop><d:current-user-principal><d:href>/dav/p/ada/</d:href>
//!     </d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
//!     </d:response></d:multistatus>"#.to_vec())?;
//! let Next::Done(found) = walk.advance(&reply)? else { panic!("one request") };
//! assert_eq!(found.principal.unwrap().path(), "/dav/p/ada/");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::http::bytes;
use crate::names::{
    ADDRESSBOOK_HOME_SET, CALENDAR_COLOR, CALENDAR_HOME_SET, CURRENT_USER_PRINCIPAL, DAV,
    DISPLAYNAME, RESOURCETYPE,
};
use crate::{Depth, Multistatus, parse_multistatus};
use ::http::{Request, Response, Uri};
use std::collections::VecDeque;

/// Which home set to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// `calendar-home-set` (CalDAV).
    Calendar,
    /// `addressbook-home-set` (CardDAV).
    Addressbook,
}

impl Home {
    /// The home set's property name.
    pub fn property(self) -> &'static str {
        match self {
            Home::Calendar => CALENDAR_HOME_SET,
            Home::Addressbook => ADDRESSBOOK_HOME_SET,
        }
    }

    fn collection_type(self) -> &'static str {
        match self {
            Home::Calendar => "urn:ietf:params:xml:ns:caldav:calendar",
            Home::Addressbook => "urn:ietf:params:xml:ns:carddav:addressbook",
        }
    }
}

/// The principal URL, as written (a path or a URL).
pub fn current_user_principal(status: &Multistatus) -> Option<String> {
    status
        .first_found_items(CURRENT_USER_PRINCIPAL)?
        .iter()
        .find(|item| !item.starts_with(DAV))
        .cloned()
}

/// The home-set hrefs of `home`, in order, each whole (an href with a space in it is one).
pub fn home_set(status: &Multistatus, home: Home) -> Vec<String> {
    status
        .first_found_items(home.property())
        .map(<[String]>::to_vec)
        .unwrap_or_default()
}

/// The hrefs among the responses whose `resourcetype` is a `home` collection (a calendar or an
/// address book): what a PROPFIND at depth 1 on the home lists.
pub fn collections(status: &Multistatus, home: Home) -> Vec<String> {
    status
        .responses
        .iter()
        .filter(|r| {
            r.found(RESOURCETYPE)
                .is_some_and(|t| t.split_whitespace().any(|k| k == home.collection_type()))
        })
        .map(|r| r.href.clone())
        .collect()
}

/// How far a walk goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The endpoint's `current-user-principal`, and no further.
    Principal,
    /// The principal's home sets.
    Homes,
    /// The collections in the home sets. A home the server names none of is read as the
    /// endpoint itself, and an endpoint that is itself a collection is its own only member.
    Collections,
}

/// Which hosts a walk follows an href to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only the endpoint's own scheme, host and port: the walk never sends to another.
    SameOrigin,
    /// Wherever the server points (iCloud keeps each account's homes on another host).
    Anywhere,
}

/// A calendar or address book in a home set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// Which kind of home it was listed in.
    pub home: Home,
    /// Its href as the server wrote it.
    pub href: String,
    /// Its URL, with the trailing slash of a collection.
    pub url: Uri,
    /// `displayname`, when the server gave a non-empty one.
    pub display_name: Option<String>,
    /// `calendar-color` as the server wrote it, when it gave one.
    pub color: Option<String>,
}

/// What a walk found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    /// The principal's URL.
    pub principal: Option<Uri>,
    /// The home sets of each home asked for, as URLs with a trailing slash, in the server's
    /// order and without repeats.
    pub homes: Vec<(Home, Vec<Uri>)>,
    /// The collections in the homes, without repeats ([`Reach::Collections`] only).
    pub collections: Vec<Collection>,
}

impl Found {
    /// The homes the server named for `home`.
    pub fn homes_of(&self, home: Home) -> &[Uri] {
        self.homes
            .iter()
            .find(|(h, _)| *h == home)
            .map_or(&[], |(_, urls)| urls.as_slice())
    }
}

/// What the walk does next.
#[derive(Debug)]
pub enum Next {
    /// Send this request (with the credential) and give the reply to [`Walk::advance`].
    Ask(Request<Vec<u8>>),
    /// The walk is over.
    Done(Found),
}

/// Why a reply ended the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The server refused the credential (401 or 403).
    #[error("the server refused the account")]
    Unauthorized,
    /// The server answered with this status, which is neither a success nor a refusal.
    #[error("the server answered {0}")]
    Status(u16),
    /// A success whose body was not a multistatus.
    #[error("the server's answer could not be read")]
    Unreadable,
}

/// The discovery state machine.
#[derive(Debug)]
pub struct Walk {
    endpoint: Uri,
    scope: Scope,
    reach: Reach,
    wants: Vec<Home>,
    stage: Stage,
    principal: Option<Uri>,
    homes: Vec<(Home, Vec<Uri>)>,
    collections: Vec<Collection>,
}

#[derive(Debug)]
enum Stage {
    /// The reply to the request at the endpoint.
    Endpoint,
    /// The reply to the request at the principal, for the homes the endpoint did not name.
    Principal(Vec<Home>),
    /// The reply to the listing of the home at `.1`, a home of kind `.0`; the rest are still to
    /// list.
    Listing(Home, Uri, VecDeque<(Home, Uri)>),
    Done,
}

impl Walk {
    /// Starts a walk from `endpoint` (an absolute URL) to `reach`, reading the `wants` homes
    /// (none for [`Reach::Principal`]). Returns the walk and its first request.
    pub fn start(
        endpoint: Uri,
        reach: Reach,
        wants: &[Home],
        scope: Scope,
    ) -> (Self, Request<Vec<u8>>) {
        let mut props = vec![CURRENT_USER_PRINCIPAL];
        if reach != Reach::Principal {
            props.extend(wants.iter().map(|home| home.property()));
        }
        let first = bytes::propfind(endpoint.clone(), Depth::Zero, &props);
        let walk = Walk {
            endpoint,
            scope,
            reach,
            wants: wants.to_vec(),
            stage: Stage::Endpoint,
            principal: None,
            homes: Vec::new(),
            collections: Vec::new(),
        };
        (walk, first)
    }

    /// The principal, once the endpoint's reply has been read: it stays known when a later
    /// reply fails, for a caller that goes on without the rest.
    pub fn principal(&self) -> Option<&Uri> {
        self.principal.as_ref()
    }

    /// Takes the reply to the last request. An error ends the walk.
    pub fn advance(&mut self, reply: &Response<Vec<u8>>) -> Result<Next, Error> {
        let stage = std::mem::replace(&mut self.stage, Stage::Done);
        match stage {
            Stage::Endpoint => Ok(self.endpoint_replied(&read(reply)?)),
            Stage::Principal(missing) => {
                let status = read(reply)?;
                let base = self
                    .principal
                    .clone()
                    .unwrap_or_else(|| self.endpoint.clone());
                for home in missing {
                    self.name_homes(home, &status, &base);
                }
                Ok(self.homes_known())
            }
            Stage::Listing(home, url, rest) => {
                self.listed(home, &read(reply)?, &url);
                Ok(self.list_next(rest))
            }
            Stage::Done => Ok(Next::Done(self.found())),
        }
    }

    fn endpoint_replied(&mut self, status: &Multistatus) -> Next {
        self.principal = current_user_principal(status)
            .and_then(|href| self.resolve(&self.endpoint, &href, false));
        if self.reach == Reach::Principal {
            return Next::Done(self.found());
        }
        let mut missing = Vec::new();
        for home in self.wants.clone() {
            if !self.name_homes(home, status, &self.endpoint.clone()) {
                missing.push(home);
            }
        }
        match (&self.principal, missing.is_empty()) {
            (Some(principal), false) => {
                let props: Vec<&str> = missing.iter().map(|home| home.property()).collect();
                let request = bytes::propfind(principal.clone(), Depth::Zero, &props);
                self.stage = Stage::Principal(missing);
                Next::Ask(request)
            }
            _ => self.homes_known(),
        }
    }

    /// Records the homes `status` names for `home`; whether it named any.
    fn name_homes(&mut self, home: Home, status: &Multistatus, base: &Uri) -> bool {
        let hrefs = home_set(status, home);
        let mut urls: Vec<Uri> = Vec::new();
        for href in &hrefs {
            if let Some(url) = self.resolve(base, href, true)
                && !urls.contains(&url)
            {
                urls.push(url);
            }
        }
        self.set_homes(home, urls);
        !hrefs.is_empty()
    }

    fn set_homes(&mut self, home: Home, urls: Vec<Uri>) {
        self.homes.retain(|(h, _)| *h != home);
        self.homes.push((home, urls));
    }

    fn homes_known(&mut self) -> Next {
        if self.reach == Reach::Homes {
            return Next::Done(self.found());
        }
        let mut queue = VecDeque::new();
        for home in self.wants.clone() {
            let mut urls = self.found_homes(home);
            if urls.is_empty() {
                // A server that names no home is read as if the endpoint were it.
                urls.extend(self.resolve(&self.endpoint, self.endpoint.path(), true));
                self.set_homes(home, urls.clone());
            }
            queue.extend(urls.into_iter().map(|url| (home, url)));
        }
        self.list_next(queue)
    }

    fn found_homes(&self, home: Home) -> Vec<Uri> {
        self.homes
            .iter()
            .find(|(h, _)| *h == home)
            .map(|(_, urls)| urls.clone())
            .unwrap_or_default()
    }

    fn list_next(&mut self, mut queue: VecDeque<(Home, Uri)>) -> Next {
        let Some((home, url)) = queue.pop_front() else {
            return Next::Done(self.found());
        };
        let request = bytes::propfind(
            url.clone(),
            Depth::One,
            &[RESOURCETYPE, DISPLAYNAME, CALENDAR_COLOR],
        );
        self.stage = Stage::Listing(home, url.clone(), queue);
        Next::Ask(request)
    }

    fn listed(&mut self, home: Home, listing: &Multistatus, base: &Uri) {
        for href in collections(listing, home) {
            let Some(url) = self.resolve(base, &href, true) else {
                continue;
            };
            if self.collections.iter().any(|c| c.url == url) {
                continue;
            }
            let response = listing.responses.iter().find(|r| r.href == href);
            let text = |name: &str| {
                response
                    .and_then(|r| r.found(name))
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
            };
            self.collections.push(Collection {
                home,
                display_name: text(DISPLAYNAME),
                color: text(CALENDAR_COLOR),
                href,
                url,
            });
        }
    }

    fn found(&mut self) -> Found {
        self.stage = Stage::Done;
        Found {
            principal: self.principal.clone(),
            homes: std::mem::take(&mut self.homes),
            collections: std::mem::take(&mut self.collections),
        }
    }

    /// The URL an href names in a reply from `base`: a path on `base`'s origin, or a URL. A space
    /// in it is written `%20`. `None` for what is not a web URL, or (with [`Scope::SameOrigin`])
    /// for another origin than the endpoint's. With `collection`, a trailing slash is added.
    fn resolve(&self, base: &Uri, href: &str, collection: bool) -> Option<Uri> {
        let href = href.trim();
        if href.is_empty() {
            return None;
        }
        let mut text = match href.contains("://") {
            true => href.to_owned(),
            false => format!(
                "{}://{}/{}",
                base.scheme_str()?,
                base.authority()?,
                href.trim_start_matches('/')
            ),
        };
        text = text.replace(' ', "%20");
        if collection && !text.ends_with('/') {
            text.push('/');
        }
        let uri: Uri = text.parse().ok()?;
        uri.authority()?;
        let web = matches!(uri.scheme_str(), Some("http" | "https"));
        let allowed = match self.scope {
            Scope::Anywhere => true,
            Scope::SameOrigin => origin(&uri) == origin(&self.endpoint),
        };
        (web && allowed).then_some(uri)
    }
}

/// Scheme, host and port, with the scheme's default port filled in.
fn origin(uri: &Uri) -> Option<(String, String, u16)> {
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let port = uri.port_u16().unwrap_or(match scheme.as_str() {
        "https" => 443,
        _ => 80,
    });
    Some((scheme, uri.host()?.to_ascii_lowercase(), port))
}

fn read(reply: &Response<Vec<u8>>) -> Result<Multistatus, Error> {
    match reply.status().as_u16() {
        401 | 403 => Err(Error::Unauthorized),
        200..=299 => {
            parse_multistatus(&String::from_utf8_lossy(reply.body())).map_err(|_| Error::Unreadable)
        }
        other => Err(Error::Status(other)),
    }
}
