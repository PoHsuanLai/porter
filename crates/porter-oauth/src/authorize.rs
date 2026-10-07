//! The URL the person opens: the authorize endpoint with PKCE, `state` and the scopes, and the
//! same for OpenRouter, whose key mint has its own page.
//!
//! The scope-per-resource rule is ported from mailo's `first_resource`
//! (`~/mailo/crates/mail-runtime/src/oauth.rs`).

use crate::form::encode;
use crate::pkce::Pkce;
use porter_provider::{ClientEntry, Issuer, IssuerEndpoints};

/// The authorize URL for `client` at `endpoints`, redirecting to `redirect`.
///
/// Google gets `access_type=offline` and `prompt=consent`: without them a repeat authorization
/// returns no refresh token and the account stops working an hour later. It also gets
/// `include_granted_scopes=true`, so asking for one more service later (incremental
/// authorization) keeps what was granted before.
pub fn authorize_url(
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    pkce: &Pkce,
    redirect: &str,
    scopes: &[String],
) -> String {
    let scope = scopes.join(" ");
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", client.client_id.0.as_str()),
        ("redirect_uri", redirect),
        ("scope", scope.as_str()),
        ("state", pkce.state.0.as_str()),
    ];
    let challenge = pkce.challenge();
    pairs.extend([
        ("code_challenge", challenge.0.as_str()),
        ("code_challenge_method", "S256"),
    ]);
    if client.issuer == Issuer::Google {
        pairs.extend([
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("include_granted_scopes", "true"),
        ]);
    }
    with_query(endpoints.authorize.as_str(), &pairs)
}

/// OpenRouter's key-mint page: it takes a callback and a challenge, and sends the person back
/// with `?code=`. It has no client id and no `state` parameter, so the state travels in the
/// callback's own query and comes back with the code.
pub fn openrouter_auth_url(endpoints: &IssuerEndpoints, pkce: &Pkce, redirect: &str) -> String {
    let callback = format!(
        "{}/?state={}",
        redirect.trim_end_matches('/'),
        encode(&pkce.state.0)
    );
    with_query(
        endpoints.authorize.as_str(),
        &[
            ("callback_url", callback.as_str()),
            ("code_challenge", pkce.challenge().0.as_str()),
            ("code_challenge_method", "S256"),
        ],
    )
}

fn with_query(base: &str, pairs: &[(&str, &str)]) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}{}", crate::form::body(pairs))
}

/// When `scopes` span more than one resource, the scopes a code should be redeemed for.
///
/// Microsoft lets one sign-in consent to several resources (Exchange's IMAP and Graph's
/// `Mail.Send`) but issues each access token for one, and refuses to redeem such a code without
/// being told which (`AADSTS28003`). The first resource named is the one the sign-in is for; the
/// others are reached later by refreshing with their scopes. Scopes that belong to no resource
/// (`offline_access`, `openid`) go with it. One resource, or none: `None`.
pub fn redeem_scope(scopes: &[String]) -> Option<String> {
    let resource = |s: &str| {
        s.strip_prefix("https://")
            .and_then(|rest| rest.split('/').next())
            .map(str::to_owned)
    };
    let resources: Vec<String> = scopes.iter().filter_map(|s| resource(s)).collect();
    let first = resources.first()?;
    if resources.iter().all(|r| r == first) {
        return None;
    }
    let chosen: Vec<&str> = scopes
        .iter()
        .filter(|s| resource(s).is_none_or(|r| &r == first))
        .map(String::as_str)
        .collect();
    Some(chosen.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::pairs;
    use porter_provider::{ClientChannel, ClientId};

    fn client(issuer: Issuer) -> ClientEntry {
        ClientEntry {
            issuer,
            channel: ClientChannel::Stable,
            client_id: ClientId("cid".into()),
            client_secret: None,
            endpoints: None,
        }
    }

    fn query(url: &str) -> Vec<(String, String)> {
        pairs(url.split_once('?').expect("a query").1)
    }

    #[test]
    fn the_authorize_url_carries_pkce_state_and_scopes() {
        let pkce = Pkce::from_random([5; 32], [6; 16]);
        let url = authorize_url(
            &Issuer::Microsoft.endpoints(),
            &client(Issuer::Microsoft),
            &pkce,
            "http://127.0.0.1:4000",
            &["Mail.Read".into(), "offline_access".into()],
        );
        assert!(url.starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?"));
        let q = query(&url);
        let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("code_challenge"), Some(pkce.challenge().0.as_str()));
        assert_eq!(get("code_challenge_method"), Some("S256"));
        assert_eq!(get("state"), Some(pkce.state.0.as_str()));
        assert_eq!(get("redirect_uri"), Some("http://127.0.0.1:4000"));
        assert_eq!(get("scope"), Some("Mail.Read offline_access"));
        assert_eq!(get("access_type"), None);
        assert!(!url.contains(pkce.verifier.expose()));
    }

    #[test]
    fn google_asks_for_offline_access() {
        let pkce = Pkce::from_random([5; 32], [6; 16]);
        let url = authorize_url(
            &Issuer::Google.endpoints(),
            &client(Issuer::Google),
            &pkce,
            "http://127.0.0.1:4000",
            &[],
        );
        assert!(url.contains("access_type=offline") && url.contains("prompt=consent"));
        assert!(url.contains("include_granted_scopes=true"));
        let microsoft = authorize_url(
            &Issuer::Microsoft.endpoints(),
            &client(Issuer::Microsoft),
            &pkce,
            "http://127.0.0.1:4000",
            &[],
        );
        assert!(!microsoft.contains("include_granted_scopes"));
    }

    #[test]
    fn openrouter_carries_state_in_the_callback() {
        let pkce = Pkce::from_random([5; 32], [6; 16]);
        let url = openrouter_auth_url(
            &Issuer::OpenRouter.endpoints(),
            &pkce,
            "http://127.0.0.1:4000",
        );
        let q = query(&url);
        let callback = &q
            .iter()
            .find(|(n, _)| n == "callback_url")
            .expect("callback")
            .1;
        assert_eq!(
            callback,
            &format!("http://127.0.0.1:4000/?state={}", pkce.state.0)
        );
    }

    #[test]
    fn a_code_for_two_resources_is_redeemed_for_the_first() {
        let s = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        const IMAP: &str = "https://outlook.office.com/IMAP.AccessAsUser.All";
        const SMTP: &str = "https://outlook.office.com/SMTP.Send";
        const GRAPH: &str = "https://graph.microsoft.com/Mail.Send";
        let cases = [
            ("one resource", s(&[IMAP, SMTP, "offline_access"]), None),
            ("none", s(&["offline_access"]), None),
            (
                "two",
                s(&[IMAP, GRAPH, "offline_access"]),
                Some(format!("{IMAP} offline_access")),
            ),
            ("graph first", s(&[GRAPH, IMAP]), Some(GRAPH.to_owned())),
        ];
        for (name, scopes, want) in cases {
            assert_eq!(redeem_scope(&scopes), want, "{name}");
        }
    }
}
