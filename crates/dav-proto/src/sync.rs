//! A sync-collection answer (RFC 6578).

use crate::multistatus::Multistatus;
use crate::names::GETETAG;

/// One change a sync-collection report lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncChange {
    /// Created or changed, with its etag (empty when the server sent none).
    Changed {
        /// The resource.
        href: String,
        /// Its etag.
        etag: String,
    },
    /// Removed (a 404 response, RFC 6578 section 3.5.2).
    Removed {
        /// The resource.
        href: String,
    },
}

/// How a sync-collection reply ends.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncEnd {
    /// Every change is listed: the next report starts from this token.
    Complete(String),
    /// The server cut the report short (a 507 response, RFC 6578 section 3.6). The changes
    /// listed are good, and a report from this token, sent at once, lists the rest. This is
    /// not an expired token: starting over would be cut short at the same place again.
    Truncated(String),
    /// The reply carries no token, so nothing can continue from it: the caller lists again.
    Missing,
}

/// The changes since a token, and how the reply ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReply {
    /// What changed, in the server's order.
    pub changes: Vec<SyncChange>,
    /// The token for the next report, and whether that report is due at once.
    pub end: SyncEnd,
}

impl SyncReply {
    /// The token to ask from next, if the reply gave one.
    pub fn token(&self) -> Option<&str> {
        match &self.end {
            SyncEnd::Complete(token) | SyncEnd::Truncated(token) => Some(token),
            SyncEnd::Missing => None,
        }
    }
}

/// Reads the changes and the token out of a sync-collection multistatus.
///
/// A 404 response is a removal. A 507 response is the server saying the report was cut short;
/// it is not a change, and the reply ends [`SyncEnd::Truncated`]. Any other non-2xx response is
/// not a change and is skipped. An empty token counts as none.
pub fn parse_sync_collection(status: &Multistatus) -> SyncReply {
    let mut changes = Vec::new();
    let mut truncated = false;
    for response in &status.responses {
        match response.status {
            Some(404) => changes.push(SyncChange::Removed {
                href: response.href.clone(),
            }),
            Some(507) => truncated = true,
            Some(code) if !(200..300).contains(&code) => {}
            _ => changes.push(SyncChange::Changed {
                href: response.href.clone(),
                etag: response.found(GETETAG).unwrap_or_default().to_owned(),
            }),
        }
    }
    let end = match status.sync_token.clone().filter(|t| !t.is_empty()) {
        None => SyncEnd::Missing,
        Some(token) if truncated => SyncEnd::Truncated(token),
        Some(token) => SyncEnd::Complete(token),
    };
    SyncReply { changes, end }
}

/// Whether a failed sync-collection answer means the token is no longer good: the
/// `valid-sync-token` precondition (RFC 6578 section 3.2) in a 403, 409 or 412 error body.
///
/// A whole-request 507 is not one: a cut-short report is a 207 with a 507 response inside it
/// ([`SyncEnd::Truncated`]), and a 507 for the request is the server's own storage failing.
pub fn token_expired(http_status: u16, body: &str) -> bool {
    matches!(http_status, 403 | 409 | 412) && body.contains("valid-sync-token")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_multistatus;

    fn reply(xml: &str) -> SyncReply {
        parse_sync_collection(&parse_multistatus(xml).unwrap())
    }

    #[test]
    fn changes_removals_and_the_next_token() {
        let r = reply(
            r#"<d:multistatus xmlns:d="DAV:">
              <d:response><d:href>/b/a.vcf</d:href><d:propstat><d:prop><d:getetag>"e1"</d:getetag></d:prop>
                <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
              <d:response><d:href>/b/gone.vcf</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
              <d:sync-token>tok-2</d:sync-token></d:multistatus>"#,
        );
        assert_eq!(
            r.changes,
            [
                SyncChange::Changed {
                    href: "/b/a.vcf".into(),
                    etag: "\"e1\"".into()
                },
                SyncChange::Removed {
                    href: "/b/gone.vcf".into()
                },
            ]
        );
        assert_eq!(r.end, SyncEnd::Complete("tok-2".into()));
        assert_eq!(r.token(), Some("tok-2"));
    }

    #[test]
    fn a_507_response_is_a_truncation_that_keeps_its_token() {
        let cut = reply(
            r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/b/</d:href>
              <d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>
              <d:sync-token>tok-3</d:sync-token></d:multistatus>"#,
        );
        assert_eq!(cut.end, SyncEnd::Truncated("tok-3".into()));
        assert_eq!(cut.token(), Some("tok-3"));
        assert!(cut.changes.is_empty());
    }

    #[test]
    fn no_token_or_an_empty_one_is_missing() {
        assert_eq!(
            reply(r#"<d:multistatus xmlns:d="DAV:"/>"#).end,
            SyncEnd::Missing
        );
        let empty =
            reply(r#"<d:multistatus xmlns:d="DAV:"><d:sync-token></d:sync-token></d:multistatus>"#);
        assert_eq!(empty.end, SyncEnd::Missing);
        assert_eq!(empty.token(), None);
        // A truncation with no token cannot be continued either.
        let cut = reply(
            r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/b/</d:href>
              <d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response></d:multistatus>"#,
        );
        assert_eq!(cut.end, SyncEnd::Missing);
    }

    #[test]
    fn a_failed_status_is_not_a_change() {
        let r = reply(
            r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/b/x</d:href>
              <d:status>HTTP/1.1 403 Forbidden</d:status></d:response>
              <d:sync-token>t</d:sync-token></d:multistatus>"#,
        );
        assert!(r.changes.is_empty());
        assert_eq!(r.end, SyncEnd::Complete("t".into()));
    }

    #[test]
    fn expired_token_answers() {
        const ERR: &str = r#"<d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#;
        const CASES: &[(u16, &str, bool)] = &[
            (403, ERR, true),
            (409, ERR, true),
            (412, ERR, true),
            (507, "", false),
            (507, ERR, false),
            (403, "<d:error/>", false),
            (207, ERR, false),
            (401, ERR, false),
        ];
        for (code, body, want) in CASES {
            assert_eq!(token_expired(*code, body), *want, "{code} {body}");
        }
    }
}
