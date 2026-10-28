//! What a status the replica did not expect means to the contract: a write that was refused
//! ([`PutRefused`]). The statuses every HTTP replica reads alike are porter-sync's `status`.

use porter_http::http::Response;
use porter_ids::UnixSeconds;
use porter_sync::PutRefused;
use porter_sync::status::retry_after;

/// A write answered with `response`, which was not a success or a precondition failure.
pub fn write_error(response: &Response<Vec<u8>>, now: UnixSeconds) -> PutRefused {
    match response.status().as_u16() {
        507 => PutRefused::Quota,
        401 | 403 | 405 | 409 | 413 | 423 => PutRefused::Forbidden,
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
            (403, None, PutRefused::Forbidden),
            (423, None, PutRefused::Forbidden),
            (409, None, PutRefused::Forbidden),
            (503, Some("7"), PutRefused::Transient(RetryAfter(7))),
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
