//! Reading an account from its issuer's API with a bearer token: who it is (OpenID Connect's
//! userinfo), and what a non-success status means for the account.

use porter_core::EndpointUrl;
use porter_http::http::header::{ACCEPT, AUTHORIZATION};
use porter_http::http::{Method, Response};
use porter_http::{Http, SensitiveHeader, request};
use porter_ids::WebUrl;
use porter_provider::ProviderError;
use serde::Deserialize;

#[derive(Deserialize)]
struct UserInfo {
    email: Option<String>,
}

/// The account's address, from the userinfo endpoint.
pub(crate) async fn userinfo_address<H: Http>(
    http: &H,
    userinfo: &EndpointUrl,
    token: &str,
) -> Result<String, ProviderError> {
    let answer = get(http, userinfo.as_str(), token).await?;
    let info: UserInfo = match answer.status().as_u16() {
        200..=299 => {
            serde_json::from_slice(answer.body()).map_err(|_| ProviderError::Unreadable)?
        }
        status => return Err(fault(status)),
    };
    info.email
        .filter(|a| !a.is_empty())
        .ok_or(ProviderError::Unreadable)
}

/// What a non-success status means for the account.
pub(crate) fn fault(status: u16) -> ProviderError {
    match status {
        401 => ProviderError::Unauthorized,
        403 => ProviderError::Forbidden,
        429 | 500..=599 => ProviderError::Unreachable,
        _ => ProviderError::Unreadable,
    }
}

/// A JSON `GET` of `url` with `token` as the bearer.
pub(crate) async fn get<H: Http>(
    http: &H,
    url: &str,
    token: &str,
) -> Result<Response<Vec<u8>>, ProviderError> {
    let url = WebUrl::parse(url).map_err(|_| ProviderError::Unreadable)?;
    let request = request(Method::GET, &url)
        .sensitive_header(AUTHORIZATION, &format!("Bearer {token}"))
        .header(ACCEPT, "application/json")
        .body(Vec::new())
        .map_err(|_| ProviderError::Unreachable)?;
    http.send(request)
        .await
        .map_err(|_| ProviderError::Unreachable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_what_the_account_needs() {
        const CASES: &[(u16, ProviderError)] = &[
            (401, ProviderError::Unauthorized),
            (403, ProviderError::Forbidden),
            (429, ProviderError::Unreachable),
            (503, ProviderError::Unreachable),
            (418, ProviderError::Unreadable),
        ];
        for (status, want) in CASES {
            assert_eq!(&fault(*status), want, "{status}");
        }
    }
}
