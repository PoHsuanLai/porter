//! The issuer's endpoints: authorize, token, revoke and device authorization.

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
        ("POST", "/device") => device(shared, request),
        _ => Response::new(404),
    }
}

fn oauth_error(error: &str) -> Response {
    Response::json(400, &json!({ "error": error }))
}

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
        let state = state_param
            .as_deref()
            .map_or(String::new(), |s| format!("&state={}", percent_encode(s)));
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
            let grant = Code {
                offline: q("access_type").as_deref() == Some("offline"),
                client_id,
                redirect_uri: redirect_uri.clone(),
                challenge: q("code_challenge").unwrap_or_default(),
                scope: q("scope").unwrap_or_default(),
            };
            state.codes.insert(code.clone(), grant);
            back(format!("code={code}"))
        }
    }
}

/// The `expires_in` of an access token unless a test set another.
pub(super) const DEFAULT_LIFETIME_S: u64 = 3600;

fn issue(state: &mut State, client_id: &str, scope: &str) -> (String, String, serde_json::Value) {
    let access = next(state, "fake-access");
    let refresh = next(state, "fake-refresh");
    record_access(state, &access, client_id, scope);
    let grant = Grant {
        client_id: client_id.to_owned(),
        scope: scope.to_owned(),
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
        _ if !secret_ok => Err("invalid_client".to_owned()),
        "authorization_code" => exchange_code(&mut state, request, &client_id),
        "refresh_token" => refresh(&mut state, &form("refresh_token"), &client_id),
        DEVICE_GRANT => poll_device(&mut state, &form("device_code"), &client_id),
        _ => Err("unsupported_grant_type".to_owned()),
    };
    let (result, response) = match outcome {
        Ok((access, refresh, body)) => (
            TokenResult::Issued { access, refresh },
            Response::json(200, &body),
        ),
        Err(error) => (TokenResult::Rejected(error.clone()), oauth_error(&error)),
    };
    shared.events.push(IssuerEvent::Token {
        grant_type,
        client_id,
        result,
    });
    response
}

type Issued = Result<(String, String, serde_json::Value), String>;

fn exchange_code(state: &mut State, request: &Request, client_id: &str) -> Issued {
    let form = |name: &str| request.form_value(name).unwrap_or_default();
    // A code is single use: it is removed whether or not the rest checks out.
    let code = state.codes.remove(&form("code")).ok_or("invalid_grant")?;
    let verifier = form("code_verifier");
    let proves = !verifier.is_empty() && s256(&verifier) == code.challenge;
    let matches = code.client_id == client_id && code.redirect_uri == form("redirect_uri");
    match proves && matches {
        true => issue_for_code(state, code.offline, client_id, &code.scope),
        false => Err("invalid_grant".to_owned()),
    }
}

fn refresh(state: &mut State, presented: &str, client_id: &str) -> Issued {
    if state.refuse_refreshes > 0 {
        state.refuse_refreshes -= 1;
        return Err("invalid_grant".to_owned());
    }
    let scope = match state.refresh.get(presented) {
        Some(grant) if grant.client_id == client_id => grant.scope.clone(),
        _ => return Err("invalid_grant".to_owned()),
    };
    if state.style == Style::Google {
        return issue_access(state, client_id, &scope, presented);
    }
    // Rotation: the presented token is spent, a new one comes back.
    state.refresh.remove(presented);
    Ok(issue(state, client_id, &scope))
}

fn poll_device(state: &mut State, device_code: &str, client_id: &str) -> Issued {
    let device = state.devices.get(device_code).ok_or("invalid_grant")?;
    if device.client_id != client_id {
        return Err("invalid_grant".to_owned());
    }
    match device.state {
        DeviceState::Pending => Err("authorization_pending".to_owned()),
        DeviceState::Denied => Err("access_denied".to_owned()),
        DeviceState::Approved => {
            let scope = device.scope.clone();
            state.devices.remove(device_code);
            Ok(issue(state, client_id, &scope))
        }
    }
}

fn revoke(shared: &Shared, request: &Request) -> Response {
    let token = request.form_value("token").unwrap_or_default();
    let mut state = lock(&shared.state);
    let known = state.refresh.remove(&token).is_some() | state.access.remove(&token).is_some();
    shared.events.push(IssuerEvent::Revoke { token, known });
    // RFC 7009: an unknown token is not an error.
    Response::new(200)
}

fn device(shared: &Shared, request: &Request) -> Response {
    let client_id = request.form_value("client_id").unwrap_or_default();
    let mut state = lock(&shared.state);
    let code = next(&mut state, "fake-device");
    let n = code.rsplit('-').next().unwrap_or("0").to_owned();
    state.devices.insert(
        code.clone(),
        Device {
            client_id: client_id.clone(),
            scope: request.form_value("scope").unwrap_or_default(),
            state: DeviceState::Pending,
        },
    );
    shared.events.push(IssuerEvent::DeviceCode { client_id });
    Response::json(
        200,
        &json!({
            "device_code": code, "user_code": format!("FAKE-{n:0>4}"),
            "verification_uri": format!("{}/activate", shared.base),
            "expires_in": 600, "interval": 1,
        }),
    )
}

/// Notes an issued access token, its client and the scopes it carries.
fn record_access(state: &mut State, access: &str, client_id: &str, scope: &str) {
    state.access.insert(access.to_owned(), client_id.to_owned());
    state
        .access_scopes
        .insert(access.to_owned(), scope.to_owned());
}
