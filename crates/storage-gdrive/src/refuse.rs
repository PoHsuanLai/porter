//! What a status the replica did not expect means to the contract: a read that failed
//! ([`ReplicaError`]) or a write that was refused ([`PutRefused`]). Drive throttles with `429`,
//! `503` and, as a `403`, the reasons `rateLimitExceeded` and `userRateLimitExceeded`; a full
//! account is a `403 storageQuotaExceeded`.

use crate::json::ErrorBody;
use porter_http::{HttpError, HttpResponse};
use porter_sync::{ExportError, PutRefused, ReplicaError, RetryAfter};

/// How long to wait when the server did not say.
const RETRY: RetryAfter = RetryAfter(30);

fn retry_after(response: &HttpResponse) -> RetryAfter {
    response.retry_after_seconds().map_or(RETRY, RetryAfter)
}

/// The first reason an error body gives.
fn reason(response: &HttpResponse) -> Option<String> {
    let body: ErrorBody = serde_json::from_slice(&response.body).ok()?;
    body.error?.errors.into_iter().find_map(|r| r.reason)
}

/// Whether a `403` is a throttle rather than a refusal.
fn throttled(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("rateLimitExceeded" | "userRateLimitExceeded" | "sharingRateLimitExceeded")
    )
}

/// A read answered with `response`, which was not what it needed.
pub fn read_error(response: &HttpResponse) -> ReplicaError {
    match response.status.0 {
        403 if throttled(reason(response).as_deref()) => {
            ReplicaError::Transient(retry_after(response))
        }
        401 | 403 => ReplicaError::Unauthorized,
        404 | 410 => ReplicaError::Gone,
        _ => ReplicaError::Transient(retry_after(response)),
    }
}

/// An export answered with `response`, which was not the file: Drive refuses a document past
/// its export limit with `403 exportSizeLimitExceeded`, and one with no file format (a Form, a
/// Site) with `403 cannotExportFile` or a `400`.
pub fn export_error(response: &HttpResponse) -> ExportError {
    match (response.status.0, reason(response).as_deref()) {
        (403, Some("exportSizeLimitExceeded")) => ExportError::TooLarge,
        (403, Some("cannotExportFile" | "fileNotExportable")) | (400, _) => {
            ExportError::Unsupported
        }
        _ => ExportError::Read(read_error(response)),
    }
}

/// A write answered with `response`, which was not a success or a precondition failure.
pub fn write_error(response: &HttpResponse) -> PutRefused {
    let why = reason(response);
    match response.status.0 {
        403 if why.as_deref() == Some("storageQuotaExceeded") => PutRefused::Quota,
        403 if throttled(why.as_deref()) => PutRefused::Transient(retry_after(response)),
        400 | 401 | 403 | 405 | 409 | 413 => PutRefused::Forbidden,
        _ => PutRefused::Transient(retry_after(response)),
    }
}

/// A request that produced no response: nothing here is the account's fault, except access it
/// has not given yet, which the host that opens connections says before any request is sent.
pub fn unreached(error: HttpError) -> ReplicaError {
    match error {
        HttpError::NotGranted => ReplicaError::NeedsConsent,
        _ => ReplicaError::Transient(RETRY),
    }
}

/// A read that failed while a write was being settled: nothing the account did wrong, except a
/// token the server refused.
pub fn refused(error: ReplicaError) -> PutRefused {
    match error {
        ReplicaError::Unauthorized | ReplicaError::NeedsConsent => PutRefused::Forbidden,
        ReplicaError::Transient(after) => PutRefused::Transient(after),
        ReplicaError::Gone | ReplicaError::AnchorExpired => PutRefused::Transient(RetryAfter(1)),
    }
}

/// The same for a write: access not given yet is a write the account may not make.
pub fn write_unreached(error: HttpError) -> PutRefused {
    match error {
        HttpError::NotGranted => PutRefused::Forbidden,
        _ => PutRefused::Transient(RETRY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::{Header, Status};

    fn answer(status: u16, retry: Option<&str>, reason: Option<&str>) -> HttpResponse {
        HttpResponse {
            status: Status(status),
            headers: retry
                .map(|v| Header::new("Retry-After", v))
                .into_iter()
                .collect(),
            body: reason
                .map(|r| format!(r#"{{"error":{{"errors":[{{"reason":"{r}"}}]}}}}"#).into_bytes())
                .unwrap_or_default(),
        }
    }

    #[test]
    fn a_refused_export_says_too_large_no_format_or_why_it_failed() {
        let cases = [
            (403, Some("exportSizeLimitExceeded"), ExportError::TooLarge),
            (403, Some("cannotExportFile"), ExportError::Unsupported),
            (400, Some("badRequest"), ExportError::Unsupported),
            (404, None, ExportError::Read(ReplicaError::Gone)),
            (
                403,
                Some("insufficientPermissions"),
                ExportError::Read(ReplicaError::Unauthorized),
            ),
            (503, None, ExportError::Read(ReplicaError::Transient(RETRY))),
        ];
        for (status, why, want) in cases {
            assert_eq!(
                export_error(&answer(status, None, why)),
                want,
                "{status} {why:?}"
            );
        }
    }

    #[test]
    fn a_failed_read_is_classed_by_its_status_and_reason() {
        const CASES: &[(u16, Option<&str>, Option<&str>, ReplicaError)] = &[
            (401, None, None, ReplicaError::Unauthorized),
            (
                403,
                None,
                Some("insufficientPermissions"),
                ReplicaError::Unauthorized,
            ),
            (
                403,
                None,
                Some("rateLimitExceeded"),
                ReplicaError::Transient(RETRY),
            ),
            (404, None, None, ReplicaError::Gone),
            (
                429,
                Some("17"),
                None,
                ReplicaError::Transient(RetryAfter(17)),
            ),
            (
                503,
                Some("120"),
                None,
                ReplicaError::Transient(RetryAfter(120)),
            ),
            (500, None, None, ReplicaError::Transient(RETRY)),
        ];
        for (status, retry, reason, want) in CASES {
            assert_eq!(
                read_error(&answer(*status, *retry, *reason)),
                *want,
                "{status} {reason:?}"
            );
        }
    }

    #[test]
    fn a_refused_write_is_classed_by_its_status_and_reason() {
        const CASES: &[(u16, Option<&str>, Option<&str>, PutRefused)] = &[
            (403, None, Some("storageQuotaExceeded"), PutRefused::Quota),
            (
                403,
                None,
                Some("insufficientFilePermissions"),
                PutRefused::Forbidden,
            ),
            (
                403,
                Some("9"),
                Some("userRateLimitExceeded"),
                PutRefused::Transient(RetryAfter(9)),
            ),
            (400, None, None, PutRefused::Forbidden),
            (413, None, None, PutRefused::Forbidden),
            (429, Some("7"), None, PutRefused::Transient(RetryAfter(7))),
            (502, None, None, PutRefused::Transient(RETRY)),
        ];
        for (status, retry, reason, want) in CASES {
            assert_eq!(
                write_error(&answer(*status, *retry, *reason)),
                *want,
                "{status} {reason:?}"
            );
        }
    }

    #[test]
    fn access_not_given_is_its_own_state_and_never_a_retry() {
        assert_eq!(unreached(HttpError::NotGranted), ReplicaError::NeedsConsent);
        assert_eq!(
            write_unreached(HttpError::NotGranted),
            PutRefused::Forbidden
        );
        assert_eq!(refused(ReplicaError::NeedsConsent), PutRefused::Forbidden);
        assert_eq!(
            unreached(HttpError::Unreachable),
            ReplicaError::Transient(RETRY)
        );
    }
}
