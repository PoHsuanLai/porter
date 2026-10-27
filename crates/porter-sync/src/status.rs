//! What an HTTP status the replica did not expect means to the contract, written once for the
//! replicas that speak HTTP: a read that failed ([`ReplicaError`]) or a write that was refused
//! ([`PutRefused`]). The statuses every server answers alike are here; a vendor's own reasons
//! (a throttle spelled as a `403`, a locked item) stay in that vendor's crate, on top of these.

use crate::refusal::{PutRefused, ReplicaError, RetryAfter};
use porter_http::http::Response;
use porter_http::{HeaderText, HttpError};

/// How long to wait when the server did not say.
pub const RETRY: RetryAfter = RetryAfter(30);

/// The wait a `Retry-After` of whole seconds asks for, or [`RETRY`] where there is none.
pub fn retry_after(response: &Response<Vec<u8>>) -> RetryAfter {
    response.retry_after_seconds().map_or(RETRY, RetryAfter)
}

/// A read answered with `response`, which was not what it needed.
pub fn read_error(response: &Response<Vec<u8>>) -> ReplicaError {
    match response.status().as_u16() {
        401 | 403 => ReplicaError::Unauthorized,
        404 | 410 => ReplicaError::Gone,
        _ => ReplicaError::Transient(retry_after(response)),
    }
}

/// A request that produced no response: nothing here is the account's fault, so try again,
/// except access it has not given yet, which the host that opens connections says before any
/// request is sent.
pub fn unreached(error: HttpError) -> ReplicaError {
    match error {
        HttpError::NotGranted => ReplicaError::NeedsConsent,
        _ => ReplicaError::Transient(RETRY),
    }
}

/// The same for a write: access not given yet is a write the account may not make.
pub fn write_unreached(error: HttpError) -> PutRefused {
    match error {
        HttpError::NotGranted => PutRefused::Forbidden,
        _ => PutRefused::Transient(RETRY),
    }
}

/// A read that failed while a write was being settled: nothing the account did wrong, except a
/// token the server refused.
pub fn refused(error: ReplicaError) -> PutRefused {
    match error {
        ReplicaError::Unauthorized => PutRefused::Unauthorized,
        ReplicaError::NeedsConsent => PutRefused::Forbidden,
        ReplicaError::Transient(after) => PutRefused::Transient(after),
        ReplicaError::Gone | ReplicaError::AnchorExpired => PutRefused::Transient(RetryAfter(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, retry: Option<&str>) -> Response<Vec<u8>> {
        retry
            .into_iter()
            .fold(Response::builder().status(status), |b, v| {
                b.header("Retry-After", v)
            })
            .body(vec![])
            .expect("response")
    }

    #[test]
    fn a_failed_read_is_classed_by_its_status() {
        const CASES: &[(u16, Option<&str>, ReplicaError)] = &[
            (401, None, ReplicaError::Unauthorized),
            (403, None, ReplicaError::Unauthorized),
            (404, None, ReplicaError::Gone),
            (410, None, ReplicaError::Gone),
            (429, Some("17"), ReplicaError::Transient(RetryAfter(17))),
            (503, Some("120"), ReplicaError::Transient(RetryAfter(120))),
            (429, Some("soon"), ReplicaError::Transient(RETRY)),
            (500, None, ReplicaError::Transient(RETRY)),
        ];
        for (status, retry, want) in CASES {
            assert_eq!(read_error(&answer(*status, *retry)), *want, "{status}");
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
            refused(ReplicaError::Unauthorized),
            PutRefused::Unauthorized
        );
        assert_eq!(
            unreached(HttpError::Unreachable),
            ReplicaError::Transient(RETRY)
        );
        assert_eq!(
            write_unreached(HttpError::Unreachable),
            PutRefused::Transient(RETRY)
        );
    }
}
