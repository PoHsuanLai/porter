//! The scopes the family asks are the ones syncd's Drive and Photos calls need: the token a
//! sign-in gets (for exactly the scopes the authorize request named) is accepted by every call
//! the Drive replica, the Photos upload and the Picker make, and a token without the scope of a
//! service is refused by the fake as Google refuses it.

use super::rig::*;
use porter_core::sheet::SignInInput;
use porter_core::{AccountId, Credential, SecretText, UnixSeconds};
use porter_http::{Http, HttpRequest, Method};
use porter_provider::{Presented, Provider, ProviderSession, SignIn, SignInStep};

fn account() -> AccountId {
    AccountId::parse("22222222-2222-4222-8222-222222222222").expect("account id")
}

/// The scopes the sign-in asked of the fake issuer.
async fn asked(rig: &Rig) -> String {
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::OpenBrowser { url } = signin.next(SignInInput::Start).await else {
        panic!("expected the browser")
    };
    let page = url.as_str().to_owned();
    let browser = tokio::spawn(async move { browse(&page).await });
    until_settled(&mut signin, 200).await;
    browser.await.expect("task").expect("browser");
    rig.google
        .issuer
        .authorize_queries()
        .pop()
        .and_then(|q| q.into_iter().find(|(n, _)| n == "scope").map(|(_, v)| v))
        .expect("a scope in the authorize request")
}

async fn bearer_for(rig: &Rig, scope: &str, audience_slug: &str) -> String {
    let refresh = rig.google.issuer.seed_refresh(CLIENT_ID, scope);
    let session = rig
        .provider
        .open(
            &account(),
            Presented::Credential(Credential::OAuth {
                access: SecretText::new("stale"),
                refresh: SecretText::new(refresh),
                expires_at: UnixSeconds(0),
            }),
        )
        .await
        .expect("open");
    let token = session
        .access_token(&audience(audience_slug))
        .await
        .expect("a token");
    token.value.expose().to_owned()
}

/// One request of the Drive replica.
struct Call {
    name: &'static str,
    method: Method,
    path: &'static str,
    body: &'static str,
}

async fn call(
    rig: &Rig,
    method: Method,
    path: &str,
    token: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> u16 {
    let url = format!("{}{path}", rig.google.api.base_url());
    let mut request = HttpRequest::new(method, porter_core::WebUrl::parse(&url).expect("url"))
        .with_header("Authorization", format!("Bearer {token}"))
        .with_body(body.as_bytes().to_vec());
    for (name, value) in extra {
        request = request.with_header(name, *value);
    }
    Wire.send(request).await.expect("answer").status.0
}

#[tokio::test]
async fn the_scopes_asked_cover_every_drive_call_the_replica_makes() {
    let rig = Rig::new(PLAIN).await;
    let scope = asked(&rig).await;
    assert!(scope.contains("auth/drive.appdata"), "{scope}");
    let token = bearer_for(&rig, &scope, "google_drive").await;
    let json = [("Content-Type", "application/json")];
    let root = r#"{"name":"Photos","mimeType":"application/vnd.google-apps.folder","parents":["appDataFolder"]}"#;
    let call_of =
        |name: &'static str, method: Method, path: &'static str, body: &'static str| Call {
            name,
            method,
            path,
            body,
        };
    let calls = [
        call_of(
            "about",
            Method::Get,
            "/drive/v3/about?fields=storageQuota",
            "",
        ),
        call_of(
            "start token",
            Method::Get,
            "/drive/v3/changes/startPageToken",
            "",
        ),
        call_of(
            "app root",
            Method::Get,
            "/drive/v3/files/appDataFolder?fields=id",
            "",
        ),
        call_of(
            "listing",
            Method::Get,
            "/drive/v3/files?spaces=appDataFolder&q=trashed%20%3D%20false",
            "",
        ),
        call_of("make a folder", Method::Post, "/drive/v3/files", root),
        call_of(
            "changes",
            Method::Get,
            "/drive/v3/changes?pageToken=0&spaces=appDataFolder",
            "",
        ),
        call_of(
            "resumable start",
            Method::Post,
            "/upload/drive/v3/files?uploadType=resumable",
            r#"{"name":"a","parents":["appDataFolder"]}"#,
        ),
    ];
    for c in calls {
        let status = call(&rig, c.method, c.path, &token, c.body, &json).await;
        assert!(
            status < 300 || status == 308,
            "{}: Drive refused the scopes asked ({status}): {scope}",
            c.name
        );
    }
    // A token without the Drive scope is refused, as Google refuses it.
    let calendar_only = bearer_for(
        &rig,
        "openid https://www.googleapis.com/auth/calendar",
        "google_calendar",
    )
    .await;
    assert_eq!(
        call(
            &rig,
            Method::Get,
            "/drive/v3/about?fields=storageQuota",
            &calendar_only,
            "",
            &[]
        )
        .await,
        403
    );
}

#[tokio::test]
async fn the_scopes_asked_cover_the_photos_upload_and_the_picker_and_each_alone_covers_its_own() {
    let rig = Rig::new(PLAIN).await;
    let scope = asked(&rig).await;
    let upload = bearer_for(&rig, &scope, "google_photos_upload").await;
    let picker = bearer_for(&rig, &scope, "google_photos_picker").await;
    let raw = [
        ("X-Goog-Upload-Protocol", "raw"),
        ("Content-Type", "application/octet-stream"),
    ];
    let json = [("Content-Type", "application/json")];
    assert_eq!(
        call(&rig, Method::Post, "/v1/uploads", &upload, "bytes", &raw).await,
        200
    );
    assert_eq!(
        call(
            &rig,
            Method::Post,
            "/v1/albums",
            &upload,
            r#"{"album":{"title":"Quire"}}"#,
            &json
        )
        .await,
        200
    );
    assert_eq!(
        call(
            &rig,
            Method::Post,
            "/v1/mediaItems:batchCreate",
            &upload,
            r#"{"newMediaItems":[]}"#,
            &json
        )
        .await,
        200
    );
    assert_eq!(
        call(&rig, Method::Post, "/v1/sessions", &picker, "{}", &json).await,
        200
    );
    assert_eq!(
        call(
            &rig,
            Method::Get,
            "/v1/mediaItems?sessionId=none",
            &picker,
            "",
            &[]
        )
        .await,
        404
    );

    // Each scope is for its own API only: the Photos upload scope does not open the Picker, nor
    // the Picker scope the upload.
    let only_upload = bearer_for(
        &rig,
        "openid https://www.googleapis.com/auth/photoslibrary.appendonly",
        "google_photos_upload",
    )
    .await;
    assert_eq!(
        call(
            &rig,
            Method::Post,
            "/v1/sessions",
            &only_upload,
            "{}",
            &json
        )
        .await,
        403
    );
    let only_picker = bearer_for(
        &rig,
        "openid https://www.googleapis.com/auth/photospicker.mediaitems.readonly",
        "google_photos_picker",
    )
    .await;
    assert_eq!(
        call(
            &rig,
            Method::Post,
            "/v1/uploads",
            &only_picker,
            "bytes",
            &raw
        )
        .await,
        403
    );
    assert_eq!(
        call(
            &rig,
            Method::Post,
            "/v1/sessions",
            &only_picker,
            "{}",
            &json
        )
        .await,
        200
    );
}
