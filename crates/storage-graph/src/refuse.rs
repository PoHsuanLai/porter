//! What a status the replica did not expect means to the contract: a read that failed
//! ([`ReplicaError`]) or a write that was refused ([`PutRefused`]). Graph throttles with `429`
//! and `503` and a `Retry-After` in seconds, and answers `423` for an item locked on the drive
//! (open in an Office app there, or checked out).

use crate::json::ErrorBody;
use porter_http::http::Response;
use porter_ids::UnixSeconds;
use porter_sync::PutRefused;
use porter_sync::status::retry_after;

/// The code an error's answer gives (`nameAlreadyExists`, `invalidRequest`).
pub fn error_code(response: &Response<Vec<u8>>) -> Option<String> {
    let body: ErrorBody = serde_json::from_slice(response.body()).ok()?;
    body.error?.code
}

/// A write answered with `response`, which was not a success or a precondition failure.
pub fn write_error(response: &Response<Vec<u8>>, now: UnixSeconds) -> PutRefused {
    match response.status().as_u16() {
        507 => PutRefused::Quota,
        401 => PutRefused::Unauthorized,
        423 => PutRefused::InUse(retry_after(response, now)),
        409 => PutRefused::Declined,
        400 | 403 | 405 | 413 => PutRefused::Forbidden,
        _ => PutRefused::Transient(retry_after(response, now)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_sync::RetryAfter;
    use porter_sync::status::RETRY;

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
    fn a_refused_write_is_classed_by_its_status() {
        const CASES: &[(u16, Option<&str>, PutRefused)] = &[
            (507, None, PutRefused::Quota),
            (401, None, PutRefused::Unauthorized),
            (403, None, PutRefused::Forbidden),
            (409, None, PutRefused::Declined),
            (413, None, PutRefused::Forbidden),
            (423, None, PutRefused::InUse(RETRY)),
            (423, Some("12"), PutRefused::InUse(RetryAfter(12))),
            (429, Some("7"), PutRefused::Transient(RetryAfter(7))),
            (502, None, PutRefused::Transient(RETRY)),
        ];
        for (status, retry, want) in CASES {
            assert_eq!(
                write_error(&answer(*status, *retry), UnixSeconds(0)),
                *want,
                "{status}"
            );
        }
    }
}
