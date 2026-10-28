//! The issuer's endpoints: authorize, token and revoke.

use super::*;

pub(super) fn next(state: &mut State, prefix: &str) -> String {
    state.counter += 1;
    format!("{prefix}-{}", state.counter)
}

pub(super) fn route(shared: &Shared, request: &Request) -> Response {
    match (request.method.as_str(), request.path()) {
        ("GET", "/authorize") => authorize(shared, request),
        ("POST", "/token") => token(shared, request),
        ("POST", "/revoke") => revoke(shared, request),
        _ => Response::new(404),
    }
}

fn oauth_error(refused: &Refused) -> Response {
    match refused.description {
        Some(description) => Response::json(
            400,
            &json!({ "error": refused.error, "error_description": description }),
        ),
        None => Response::json(400, &json!({ "error": refused.error })),
    }
}

/// A token request refused: the OAuth `error`, and the `error_description` where the issuer
/// writes one that matters.
#[derive(Debug)]
struct Refused {
    error: String,
    description: Option<&'static str>,
}

impl From<&str> for Refused {
    fn from(error: &str) -> Self {
        Self {
            error: error.to_owned(),
            description: None,
        }
    }
}

impl From<String> for Refused {
    fn from(error: String) -> Self {
        Self {
            error,
            description: None,
        }
    }
}

/// What Microsoft says when a refresh names a scope the person never consented to.
const AADSTS65001: &str = "AADSTS65001: The user or administrator has not consented to use the \
    application with ID 'client'. Send an interactive authorization request for this user and \
    resource.";

fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn authorize(shared: &Shared, request: &Request) -> Response {
    let q = |name: &str| request.query_value(name);
    let refuse = |error: &str| {
        shared.events.push(IssuerEvent::AuthorizeRefused {
            error: error.to_owned(),
        });
        Response::new(400).typed("text/plain", format!("authorize refused: {error}"))
    };
    let (Some(client_id), Some(redirect_uri)) = (q("client_id"), q("redirect_uri")) else {
        return refuse("invalid_request: client_id and redirect_uri are required");
    };
    shared.queries.push(request.query());
    let state_param = q("state");
    let back = |query: String| {
        let sep = if redirect_uri.contains('?') { '&' } else { '?' };
        let state = state_param.as_deref().map_or(String::new(), |s| {
            format!("&state={}", porter_ids::percent::encode_unreserved(s))
        });
        Response::redirect(&format!("{redirect_uri}{sep}{query}{state}"))
    };
    let valid = q("response_type").as_deref() == Some("code")
        && q("code_challenge_method").as_deref() == Some("S256")
        && q("code_challenge").is_some_and(|c| !c.is_empty());
    if !valid {
        shared.events.push(IssuerEvent::AuthorizeRefused {
            error: "invalid_request: response_type=code and a S256 code_challenge are required"
                .to_owned(),
        });
        return back("error=invalid_request".to_owned());
    }
    let mut state = lock(&shared.state);
    let consent = state.consent;
    shared.events.push(IssuerEvent::Authorize {
        client_id: client_id.clone(),
        redirect_uri: redirect_uri.clone(),
        state: state_param.clone(),
        consent,
    });
    match consent {
        Consent::Deny => back("error=access_denied".to_owned()),
        Consent::Grant => {
            let code = next(&mut state, "fake-code");
            // Microsoft keeps every consent of a client; Google only when asked to
            // (`include_granted_scopes`), which porter always is.
            let keep = state.style == Style::Rotating
                || q("include_granted_scopes").as_deref() == Some("true");
            let scope = consented(&state, &client_id, &q("scope").unwrap_or_default(), keep);
            let grant = Code {
                offline: q("access_type").as_deref() == Some("offline"),
                client_id,
                redirect_uri: redirect_uri.clone(),
                challenge: q("code_challenge").unwrap_or_default(),
                scope,
            };
            state.codes.insert(code.clone(), grant);
            back(format!("code={code}"))
        }
    }
}

/// What the person grants at the authorize page: what was asked, less what they unticked
/// (granular consent), and with `keep` what the client was granted before (incremental
/// authorization). The fake serves one person, so "before" is the client's earlier codes.
fn consented(state: &State, client_id: &str, asked: &str, keep: bool) -> String {
    let mut scopes: Vec<&str> = match keep {
        true => state
            .consented
            .get(client_id)
            .map(|held| held.iter().map(String::as_str).collect())
            .unwrap_or_default(),
        false => Vec::new(),
    };
    for scope in asked.split_whitespace() {
        if !state.unticked.iter().any(|u| u == scope) && !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes.join(" ")
}

/// The `expires_in` of an access token unless a test set another.
pub(super) const DEFAULT_LIFETIME_S: u64 = 3600;

fn issue(state: &mut State, client_id: &str, scope: &str) -> (String, String, serde_json::Value) {
    issue_limited(state, client_id, scope, true)
}

/// [`issue`], for a grant that a refresh may narrow only within its scope when `limited` (one a
/// person consented to) or that serves any scope asked (one planted by `seed_refresh`).
fn issue_limited(
    state: &mut State,
    client_id: &str,
    scope: &str,
    limited: bool,
) -> (String, String, serde_json::Value) {
    let access = next(state, "fake-access");
    let refresh = next(state, "fake-refresh");
    record_access(state, &access, client_id, scope);
    let grant = Grant {
        client_id: client_id.to_owned(),
        scope: scope.to_owned(),
        limited,
    };
    state.refresh.insert(refresh.clone(), grant);
    let body = json!({
        "access_token": access, "token_type": "Bearer", "expires_in": state.lifetime_s,
        "refresh_token": refresh, "scope": scope,
    });
    (access, refresh, body)
}

/// An access token alone, for the refresh token `kept`, which stays valid (Google's refresh).
fn issue_access(state: &mut State, client_id: &str, scope: &str, kept: &str) -> Issued {
    let access = next(state, "fake-access");
    record_access(state, &access, client_id, scope);
    let body = json!({
        "access_token": access, "token_type": "Bearer", "expires_in": state.lifetime_s,
        "scope": scope,
    });
    Ok((access, kept.to_owned(), body))
}

/// A code's first tokens: Google issues a refresh token only to a code that asked for offline
/// access.
fn issue_for_code(state: &mut State, offline: bool, client_id: &str, scope: &str) -> Issued {
    match (state.style, offline) {
        (Style::Google, false) => {
            let access = next(state, "fake-access");
            record_access(state, &access, client_id, scope);
            let body = json!({
                "access_token": access, "token_type": "Bearer",
                "expires_in": state.lifetime_s, "scope": scope,
            });
            Ok((access, String::new(), body))
        }
        _ => Ok(issue(state, client_id, scope)),
    }
}

fn token(shared: &Shared, request: &Request) -> Response {
    let form = |name: &str| request.form_value(name).unwrap_or_default();
    let (grant_type, client_id) = (form("grant_type"), form("client_id"));
    let mut state = lock(&shared.state);
    let secret_ok = state
        .client_secret
        .as_deref()
        .is_none_or(|want| form("client_secret") == want);
    let outcome = match grant_type.as_str() {
        _ if !secret_ok => Err(Refused::from("invalid_client")),
        "authorization_code" => exchange_code(&mut state, request, &client_id),
        "refresh_token" => refresh(
            &mut state,
            &form("refresh_token"),
            &client_id,
            &form("scope"),
        ),
        _ => Err(Refused::from("unsupported_grant_type")),
    };
    let (result, response) = match outcome {
        Ok((access, refresh, body)) => (
            TokenResult::Issued { access, refresh },
            Response::json(200, &body),
        ),
        Err(refused) => (
            TokenResult::Rejected(refused.error.clone()),
            oauth_error(&refused),
        ),
    };
    shared.events.push(IssuerEvent::Token {
        grant_type,
        client_id,
        result,
    });
    response
}

type Issued = Result<(String, String, serde_json::Value), Refused>;

fn exchange_code(state: &mut State, request: &Request, client_id: &str) -> Issued {
    let form = |name: &str| request.form_value(name).unwrap_or_default();
    // A code is single use: it is removed whether or not the rest checks out.
    let code = state.codes.remove(&form("code")).ok_or("invalid_grant")?;
    let verifier = form("code_verifier");
    let proves = !verifier.is_empty() && s256(&verifier) == code.challenge;
    let matches = code.client_id == client_id && code.redirect_uri == form("redirect_uri");
    if !(proves && matches) {
        return Err("invalid_grant".into());
    }
    let held = state.consented.entry(client_id.to_owned()).or_default();
    for scope in code.scope.split_whitespace() {
        if !held.iter().any(|s| s == scope) {
            held.push(scope.to_owned());
        }
    }
    issue_for_code(state, code.offline, client_id, &code.scope)
}

/// Whether a grant of `held` (space separated) covers every scope of `asked`: each is held, or
/// is a resource's `.default` and the grant holds a scope of that resource.
fn covers(held: &str, asked: &str) -> bool {
    let held: Vec<&str> = held.split_whitespace().collect();
    asked.split_whitespace().all(|scope| {
        held.contains(&scope)
            || scope
                .strip_suffix(".default")
                .is_some_and(|resource| held.iter().any(|h| h.starts_with(resource)))
    })
}

/// A refresh: for the whole grant, or narrowed to the `scope` asked, which must be within it (a
/// grant planted by `seed_refresh` is for whatever is asked). Asking beyond it is refused as each
/// issuer does: Google with `invalid_scope`, Microsoft with `invalid_grant` and `AADSTS65001`.
fn refresh(state: &mut State, presented: &str, client_id: &str, asked: &str) -> Issued {
    if state.refuse_refreshes > 0 {
        state.refuse_refreshes -= 1;
        return Err("invalid_grant".into());
    }
    let (scope, limited) = match state.refresh.get(presented) {
        Some(grant) if grant.client_id == client_id => (grant.scope.clone(), grant.limited),
        _ => return Err("invalid_grant".into()),
    };
    if limited && !covers(&scope, asked) {
        return Err(match state.style {
            Style::Google => "invalid_scope".into(),
            Style::Rotating => Refused {
                error: "invalid_grant".to_owned(),
                description: Some(AADSTS65001),
            },
        });
    }
    let carried = match asked.trim().is_empty() {
        true => scope.clone(),
        false => asked.to_owned(),
    };
    if state.style == Style::Google {
        return issue_access(state, client_id, &carried, presented);
    }
    // Rotation: the presented token is spent, a new one comes back, for the whole grant.
    state.refresh.remove(presented);
    let (access, refresh, mut body) = issue_limited(state, client_id, &scope, limited);
    record_access(state, &access, client_id, &carried);
    body["scope"] = json!(carried);
    Ok((access, refresh, body))
}

fn revoke(shared: &Shared, request: &Request) -> Response {
    let token = request.form_value("token").unwrap_or_default();
    let mut state = lock(&shared.state);
    let known = state.refresh.remove(&token).is_some() | state.access.remove(&token).is_some();
    shared.events.push(IssuerEvent::Revoke { token, known });
    // RFC 7009: an unknown token is not an error.
    Response::new(200)
}

/// Notes an issued access token, its client and the scopes it carries.
fn record_access(state: &mut State, access: &str, client_id: &str, scope: &str) {
    state.access.insert(access.to_owned(), client_id.to_owned());
    state
        .access_scopes
        .insert(access.to_owned(), scope.to_owned());
    let lapse = Instant::now() + Duration::from_secs(state.lifetime_s);
    state.access_lapses.insert(access.to_owned(), lapse);
}
