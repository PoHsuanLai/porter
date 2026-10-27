//! `application/x-www-form-urlencoded` (through `form_urlencoded`) and the OAuth error body.

use crate::exchange::{ExchangeFault, IssuerSays, SAYS_MAX_CHARS};
use porter_core::EndpointUrl;
use porter_http::http::header::{ACCEPT, CONTENT_TYPE};
use porter_http::http::{Method, Response};
use porter_http::{Http, request_to};

/// The pairs of a query string or form body, decoded: `%XX` and `+` as a space, a malformed
/// escape kept as written so a truncated code fails at the issuer and not silently here. A part
/// without `=` is a key with an empty value.
pub(crate) fn pairs(query: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Pairs as a form body: a space is `+`, every other byte but `A-Z a-z 0-9 * - . _` is `%XX` (so `~` is `%7E`).
pub(crate) fn body(pairs: &[(&str, &str)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

/// A form POST to `url`, sent over `http`. An endpoint that is not a web URL, a request that
/// went nowhere: both are an issuer that cannot be reached.
pub(crate) async fn send_form<H: Http>(
    http: &H,
    url: &EndpointUrl,
    pairs: &[(&str, &str)],
) -> Result<Response<Vec<u8>>, ExchangeFault> {
    let request = request_to(Method::POST, url)
        .map_err(|_| ExchangeFault::Unreachable)?
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(ACCEPT, "application/json")
        .body(body(pairs).into_bytes())
        .map_err(|_| ExchangeFault::Unreachable)?;
    http.send(request)
        .await
        .map_err(|_| ExchangeFault::Unreachable)
}

/// The `error` of an OAuth error body, if the body is one.
pub(crate) fn oauth_error(response: &Response<Vec<u8>>) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(response.body())
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

/// Untrusted issuer text made safe to show: control characters dropped, cut to the cap, and
/// `None` when nothing printable is left.
fn tidy(text: &str) -> Option<String> {
    let kept: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(SAYS_MAX_CHARS)
        .collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// What an OAuth error body says (`error`, `error_description`), tidied; `None` when the body is
/// not one or says nothing.
pub(crate) fn issuer_says(response: &Response<Vec<u8>>) -> Option<IssuerSays> {
    let body = serde_json::from_slice::<serde_json::Value>(response.body()).ok()?;
    let field = |name: &str| body.get(name)?.as_str().and_then(tidy);
    let says = IssuerSays {
        error: field("error"),
        description: field("error_description"),
    };
    (says.error.is_some() || says.description.is_some()).then_some(says)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_table() {
        const CASES: &[(&str, &str)] = &[
            ("4%2F0Aa", "4/0Aa"),
            ("x%20y", "x y"),
            ("a+b", "a b"),
            ("a%2Bb", "a+b"),
            ("a%zz", "a%zz"),
            ("trailing%", "trailing%"),
            ("%e2%9c%93", "\u{2713}"),
        ];
        for (raw, want) in CASES {
            let got = pairs(&format!("k={raw}"));
            assert_eq!(got, vec![("k".to_owned(), (*want).to_owned())], "{raw}");
        }
    }

    #[test]
    fn a_body_round_trips_and_keeps_unreserved() {
        let text = "a b&c=d/\u{e9}~-._+";
        let encoded = body(&[("k", text)]);
        assert_eq!(pairs(&encoded), vec![("k".to_owned(), text.to_owned())]);
        assert_eq!(body(&[("k", "Az09*-._~")]), "k=Az09*-._%7E");
        assert_eq!(body(&[("a", "1 2"), ("b", "&")]), "a=1+2&b=%26");
        assert_eq!(
            body(&[("redirect", "http://127.0.0.1:1/cb")]),
            "redirect=http%3A%2F%2F127.0.0.1%3A1%2Fcb"
        );
    }

    #[test]
    fn a_part_without_a_value_is_an_empty_one() {
        assert_eq!(
            pairs("code=1&flag&state=x%20y"),
            vec![
                ("code".into(), "1".into()),
                ("flag".into(), String::new()),
                ("state".into(), "x y".into())
            ]
        );
        assert!(pairs("").is_empty());
    }
}
