//! A sync-collection answer (RFC 6578).
//!
//! The removal rule (a 404 response) and the token reading follow mailo
//! `crates/mail-pim/src/dav/reply.rs` and `mod.rs` (same author, MIT OR Apache-2.0).

use crate::multistatus::{DavFault, Multistatus};
use crate::names::GETETAG;

/// One change a sync-collection report lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncChange {
    /// Created or changed, with its etag.
    Changed {
        /// The resource.
        href: String,
        /// Its etag.
        etag: String,
    },
    /// Removed (a 404 response).
    Removed {
        /// The resource.
        href: String,
    },
}

/// The changes since a token, and the token for next time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReply {
    /// What changed.
    pub changes: Vec<SyncChange>,
    /// The new token; absent when the server expired ours (`valid-sync-token`), which the
    /// consumer treats as an expired anchor.
    pub sync_token: Option<String>,
}

/// Reads the changes and the token out of a multistatus.
///
/// A 404 response is a removal. A response with the 507 status (RFC 6578 section 3.6, the server
/// cut the report short) leaves `sync_token` absent, so the consumer re-lists from scratch like
/// it does for an expired token. Any other non-2xx response is not a change and is skipped.
pub fn parse_sync_collection(status: &Multistatus) -> Result<SyncReply, DavFault> {
    let mut changes = Vec::new();
    let mut truncated = false;
    for response in status.responses.iter() {
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
    let sync_token = status
        .sync_token
        .clone()
        .filter(|t| !t.is_empty() && !truncated);
    Ok(SyncReply {
        changes,
        sync_token,
    })
}

/// Whether a failed sync-collection answer means the token is no longer good: 507, or the
/// `valid-sync-token` precondition (RFC 6578 section 3.2) in a 403, 409 or 412 error body.
pub fn token_expired(http_status: u16, body: &str) -> bool {
    http_status == 507
        || (matches!(http_status, 403 | 409 | 412) && body.contains("valid-sync-token"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_multistatus;

    fn reply(xml: &str) -> SyncReply {
        parse_sync_collection(&parse_multistatus(xml).unwrap()).unwrap()
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
        assert_eq!(r.sync_token.as_deref(), Some("tok-2"));
    }

    #[test]
    fn a_507_response_or_a_missing_token_is_an_expired_anchor() {
        let cut = reply(
            r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/b/</d:href>
              <d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>
              <d:sync-token>tok-3</d:sync-token></d:multistatus>"#,
        );
        assert_eq!(cut.sync_token, None);
        assert!(cut.changes.is_empty());
        let none = reply(r#"<d:multistatus xmlns:d="DAV:"/>"#);
        assert_eq!(none.sync_token, None);
    }

    #[test]
    fn a_failed_status_is_not_a_change() {
        let r = reply(
            r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/b/x</d:href>
              <d:status>HTTP/1.1 403 Forbidden</d:status></d:response>
              <d:sync-token>t</d:sync-token></d:multistatus>"#,
        );
        assert!(r.changes.is_empty());
    }

    #[test]
    fn expired_token_answers() {
        const ERR: &str = r#"<d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#;
        const CASES: &[(u16, &str, bool)] = &[
            (507, "", true),
            (403, ERR, true),
            (409, ERR, true),
            (403, "<d:error/>", false),
            (207, ERR, false),
            (401, ERR, false),
        ];
        for (code, body, want) in CASES {
            assert_eq!(token_expired(*code, body), *want, "{code} {body}");
        }
    }
}
