//! The issuer's answer to a device-code request (RFC 8628), for sign-in on a headless or SSH
//! session: the sheet shows `user_code` and `verification_uri`, and the family polls the token
//! endpoint every `interval` seconds until `expires_in`.

use porter_core::SecretText;
use serde::{Deserialize, Serialize};

/// A device-code response as the issuer writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    /// What the family polls with; secret.
    pub device_code: SecretText,
    /// What the person types.
    pub user_code: String,
    /// Where they type it.
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
}
