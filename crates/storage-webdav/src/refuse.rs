//! What a status the replica did not expect means to the contract: a read that failed
//! ([`ReplicaError`]) or a write that was refused ([`PutRefused`]).

use porter_http::{HttpError, HttpResponse};
use porter_sync::{PutRefused, ReplicaError, RetryAfter};

/// How long to wait when the server did not say.
const RETRY: RetryAfter = RetryAfter(30);

/// The wait a `Retry-After` of whole seconds asks for.
fn retry_after(response: &HttpResponse) -> RetryAfter {
    response
        .header("retry-after")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map_or(RETRY, RetryAfter)
}

/// A read answered with `response`, which was not what it needed.
pub fn read_error(response: &HttpResponse) -> ReplicaError {
    match response.status.0 {
        401 | 403 => ReplicaError::Unauthorized,
        404 | 410 => ReplicaError::Gone,
        _ => ReplicaError::Transient(retry_after(response)),
    }
}

/// A write answered with `response`, which was not a success or a precondition failure.
pub fn write_error(response: &HttpResponse) -> PutRefused {
    match response.status.0 {
        507 => PutRefused::Quota,
        401 | 403 | 405 | 409 | 413 | 423 => PutRefused::Forbidden,
        _ => PutRefused::Transient(retry_after(response)),
    }
}

/// A request that produced no response: nothing here is the account's fault, so try again.
pub fn unreached(_: HttpError) -> ReplicaError {
    ReplicaError::Transient(RETRY)
}

/// The same for a write.
pub fn write_unreached(_: HttpError) -> PutRefused {
    PutRefused::Transient(RETRY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::{Header, Status};

    fn answer(status: u16, retry: Option<&str>) -> HttpResponse {
        HttpResponse {
            status: Status(status),
            headers: retry
                .map(|v| Header::new("Retry-After", v))
                .into_iter()
                .collect(),
            body: vec![],
        }
    }

    #[test]
    fn a_failed_read_is_classed_by_its_status() {
        const CASES: &[(u16, Option<&str>, ReplicaError)] = &[
            (401, None, ReplicaError::Unauthorized),
            (403, None, ReplicaError::Unauthorized),
            (404, None, ReplicaError::Gone),
            (410, None, ReplicaError::Gone),
            (503, Some("120"), ReplicaError::Transient(RetryAfter(120))),
            (429, Some("soon"), ReplicaError::Transient(RETRY)),
            (500, None, ReplicaError::Transient(RETRY)),
        ];
        for (status, retry, want) in CASES {
            assert_eq!(read_error(&answer(*status, *retry)), *want, "{status}");
        }
    }

    #[test]
    fn a_refused_write_is_classed_by_its_status() {
        const CASES: &[(u16, Option<&str>, PutRefused)] = &[
            (507, None, PutRefused::Quota),
            (403, None, PutRefused::Forbidden),
            (423, None, PutRefused::Forbidden),
            (409, None, PutRefused::Forbidden),
            (503, Some("7"), PutRefused::Transient(RetryAfter(7))),
            (502, None, PutRefused::Transient(RETRY)),
        ];
        for (status, retry, want) in CASES {
            assert_eq!(write_error(&answer(*status, *retry)), *want, "{status}");
        }
    }
}
