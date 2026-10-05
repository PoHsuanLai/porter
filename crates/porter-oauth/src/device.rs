//! The issuer's answer to a device-code request (RFC 8628), for sign-in on a headless or SSH
//! session: the sheet shows `user_code` and `verification_uri`, and the family polls the token
//! endpoint every `interval` seconds until `expires_in`.

use crate::exchange::{ExchangeFault, TokenResponse, read_tokens};
use crate::form;
use porter_core::SecretText;
use porter_http::{Http, HttpResponse};
use porter_provider::{ClientEntry, IssuerEndpoints};
use serde::{Deserialize, Serialize};
use std::future::Future;

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// How much slower RFC 8628 §3.5 says to poll after `slow_down`.
const SLOW_DOWN_SECONDS: u32 = 5;

/// A device-code response as the issuer writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    /// What the family polls with; secret.
    pub device_code: SecretText,
    /// What the person types.
    pub user_code: String,
    /// Where they type it (Google spells the field `verification_url`).
    #[serde(alias = "verification_url")]
    pub verification_uri: String,
    /// Seconds until the codes stop working.
    pub expires_in: u32,
    /// Seconds between polls; RFC 8628 says 5 when absent.
    #[serde(default = "default_interval")]
    pub interval: u32,
}

fn default_interval() -> u32 {
    5
}

/// Why a device-code sign-in did not end in tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeviceFault {
    /// The issuer has no device endpoint.
    #[error("the issuer has no device flow")]
    Unsupported,
    /// The person said no.
    #[error("the person declined")]
    Denied,
    /// The codes ran out before the person answered.
    #[error("the device code expired")]
    Expired,
    /// A call failed.
    #[error(transparent)]
    Exchange(#[from] ExchangeFault),
}

/// One poll's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    /// Not yet; poll again after the interval.
    Pending,
    /// Poll more slowly.
    SlowDown,
    /// Approved.
    Approved(TokenResponse),
    /// Declined.
    Denied,
    /// The codes are no longer good.
    Expired,
}

/// Asks the issuer for a device code and a user code.
pub async fn request_device_code<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    scope: &str,
) -> Result<DeviceCodeResponse, DeviceFault> {
    let url = endpoints.device.as_ref().ok_or(DeviceFault::Unsupported)?;
    let response = http
        .send(form::post(
            url,
            &[("client_id", client.client_id.0.as_str()), ("scope", scope)],
        ))
        .await
        .map_err(|_| ExchangeFault::Unreachable)?;
    match response.status.0 {
        200..=299 => {
            serde_json::from_slice(&response.body).map_err(|_| ExchangeFault::Unreadable.into())
        }
        429 | 500..=599 => Err(ExchangeFault::Unreachable.into()),
        _ => Err(ExchangeFault::Unreadable.into()),
    }
}

/// Polls the token endpoint once.
pub async fn poll_device<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    device: &DeviceCodeResponse,
) -> Result<DevicePoll, ExchangeFault> {
    let mut pairs = vec![
        ("grant_type", DEVICE_GRANT),
        ("device_code", device.device_code.expose()),
        ("client_id", client.client_id.0.as_str()),
    ];
    pairs.extend(
        client
            .client_secret
            .as_ref()
            .map(|s| ("client_secret", s.expose())),
    );
    let response = http
        .send(form::post(&endpoints.token, &pairs))
        .await
        .map_err(|_| ExchangeFault::Unreachable)?;
    read_poll(&response)
}

fn read_poll(response: &HttpResponse) -> Result<DevicePoll, ExchangeFault> {
    match (response.status.0, form::oauth_error(response).as_deref()) {
        (400, Some("authorization_pending")) => Ok(DevicePoll::Pending),
        (400, Some("slow_down")) => Ok(DevicePoll::SlowDown),
        (400, Some("access_denied" | "authorization_declined")) => Ok(DevicePoll::Denied),
        (400, Some("expired_token")) => Ok(DevicePoll::Expired),
        _ => read_tokens(response).map(DevicePoll::Approved),
    }
}

/// Polls until the person answers or the codes expire. `sleep` waits that many seconds (the
/// daemon passes the runtime's timer, a test passes nothing), and the expiry is counted in the
/// seconds slept, so no clock is read: `interval`, `slow_down` (+5 s) and `expires_in` are all
/// the issuer's.
pub async fn await_device<H, S, Fut>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    device: &DeviceCodeResponse,
    mut sleep: S,
) -> Result<TokenResponse, DeviceFault>
where
    H: Http,
    S: FnMut(u32) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut interval = device.interval.max(1);
    let mut waited = 0u32;
    while waited < device.expires_in {
        sleep(interval).await;
        waited = waited.saturating_add(interval);
        match poll_device(http, endpoints, client, device).await? {
            DevicePoll::Pending => {}
            DevicePoll::SlowDown => interval = interval.saturating_add(SLOW_DOWN_SECONDS),
            DevicePoll::Approved(tokens) => return Ok(tokens),
            DevicePoll::Denied => return Err(DeviceFault::Denied),
            DevicePoll::Expired => return Err(DeviceFault::Expired),
        }
    }
    Err(DeviceFault::Expired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_reads_and_the_interval_defaults() {
        let text = r#"{"device_code":"dc-secret","user_code":"ABCD-EFGH","verification_uri":"https://login.example.org/device","expires_in":900}"#;
        let response: DeviceCodeResponse = serde_json::from_str(text).expect("reads");
        assert_eq!((response.interval, response.expires_in), (5, 900));
        assert!(!format!("{response:?}").contains("dc-secret"));
    }

    use crate::scripted::{Scripted, answer};
    use porter_provider::{ClientChannel, ClientId, Issuer};

    fn client() -> ClientEntry {
        ClientEntry {
            issuer: Issuer::Microsoft,
            channel: ClientChannel::Stable,
            client_id: ClientId("cid".into()),
            client_secret: None,
            endpoints: None,
        }
    }

    fn device(interval: u32, expires_in: u32) -> DeviceCodeResponse {
        DeviceCodeResponse {
            device_code: SecretText::new("dc"),
            user_code: "U".into(),
            verification_uri: "https://x/device".into(),
            expires_in,
            interval,
        }
    }

    #[test]
    fn google_spells_the_uri_verification_url() {
        let text = r#"{"device_code":"d","user_code":"u","verification_url":"https://google.com/device","expires_in":1800,"interval":5}"#;
        let response: DeviceCodeResponse = serde_json::from_str(text).expect("reads");
        assert_eq!(response.verification_uri, "https://google.com/device");
    }

    #[tokio::test]
    async fn slow_down_adds_five_seconds_to_every_later_wait() {
        let pending = || answer(400, r#"{"error":"authorization_pending"}"#);
        let http = Scripted::new(vec![
            pending(),
            answer(400, r#"{"error":"slow_down"}"#),
            pending(),
            answer(
                200,
                r#"{"access_token":"a","refresh_token":"r","expires_in":60}"#,
            ),
        ]);
        let waits = std::cell::RefCell::new(Vec::new());
        let sleep = |s: u32| {
            waits.borrow_mut().push(s);
            std::future::ready(())
        };
        let tokens = await_device(
            &http,
            &Issuer::Microsoft.endpoints(),
            &client(),
            &device(5, 900),
            sleep,
        )
        .await
        .expect("tokens");
        assert_eq!(tokens.access_token.expose(), "a");
        assert_eq!(*waits.borrow(), vec![5, 5, 10, 10]);
        let body = http.body_of(0);
        assert!(body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
        assert!(body.contains("device_code=dc") && body.contains("client_id=cid"));
    }

    #[tokio::test]
    async fn polling_ends_in_the_issuers_verdict_or_the_clock() {
        let ends = Issuer::Microsoft.endpoints();
        let nap = |_: u32| std::future::ready(());
        let verdicts: Vec<(&str, Vec<_>, u32, Result<(), DeviceFault>)> = vec![
            (
                "denied",
                vec![answer(400, r#"{"error":"authorization_declined"}"#)],
                900,
                Err(DeviceFault::Denied),
            ),
            (
                "expired_token",
                vec![answer(400, r#"{"error":"expired_token"}"#)],
                900,
                Err(DeviceFault::Expired),
            ),
            (
                "the clock",
                vec![answer(400, r#"{"error":"authorization_pending"}"#); 3],
                15,
                Err(DeviceFault::Expired),
            ),
            (
                "5xx",
                vec![answer(503, "")],
                900,
                Err(DeviceFault::Exchange(ExchangeFault::Unreachable)),
            ),
            (
                "invalid_grant",
                vec![answer(400, r#"{"error":"invalid_grant"}"#)],
                900,
                Err(DeviceFault::Exchange(ExchangeFault::Refused)),
            ),
        ];
        for (name, script, expires_in, want) in verdicts {
            let http = Scripted::new(script);
            let got = await_device(&http, &ends, &client(), &device(5, expires_in), nap)
                .await
                .map(|_| ());
            assert_eq!(got, want, "{name}");
        }
    }
}
