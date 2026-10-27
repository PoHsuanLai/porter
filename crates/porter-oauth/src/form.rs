//! `application/x-www-form-urlencoded`, through `form_urlencoded`: the pairs of a query or a form
//! body, and a body made of pairs. It names nothing else of this crate.

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
