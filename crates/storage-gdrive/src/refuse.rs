//! What a status the replica did not expect means to the contract: a read that failed
//! ([`ReplicaError`]) or a write that was refused ([`PutRefused`]). Drive throttles with `429`,
//! `503` and, as a `403`, the reasons `rateLimitExceeded` and `userRateLimitExceeded`; a full
//! account is a `403 storageQuotaExceeded`.

use crate::json::ErrorBody;
use porter_http::{HttpError, HttpResponse};
use porter_sync::{PutRefused, ReplicaError, RetryAfter};

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

/// A request that produced no response: nothing here is the account's fault.
pub fn unreached(_: HttpError) -> ReplicaError {
    ReplicaError::Transient(RETRY)
}

/// A read that failed while a write was being settled: nothing the account did wrong, except a
/// token the server refused.
pub fn refused(error: ReplicaError) -> PutRefused {
    match error {
        ReplicaError::Unauthorized => PutRefused::Forbidden,
        ReplicaError::Transient(after) => PutRefused::Transient(after),
        ReplicaError::Gone | ReplicaError::AnchorExpired => PutRefused::Transient(RetryAfter(1)),
    }
}

/// The same for a write.
pub fn write_unreached(_: HttpError) -> PutRefused {
    PutRefused::Transient(RETRY)
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
}
