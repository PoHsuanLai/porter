//! The secrets the rig plants: fixed, unmistakable strings, so a scenario's secret scan can grep
//! a jail for them (a password that reaches a log, a file or an app's memory dump shows up as
//! one of these). Every one is also listed in `rig.json` under `secrets`.

/// The IMAP, SMTP and POP3 fakes' password.
pub const MAIL_PASSWORD: &str = "RIG-SECRET-MAIL-PASSWORD-9d41c7";
/// The DAV fake's password.
pub const DAV_PASSWORD: &str = "RIG-SECRET-DAV-PASSWORD-2b8e05";
/// An app password the Nextcloud fake accepts.
pub const NEXTCLOUD_APP_PASSWORD: &str = "RIG-SECRET-NEXTCLOUD-APP-PASSWORD-61f0aa";
/// A refresh token the OAuth issuer holds as live.
pub const OAUTH_REFRESH_TOKEN: &str = "RIG-SECRET-REFRESH-TOKEN-c3d9e1";
/// An access token the OAuth issuer (and the Graph fake, which asks it) accepts.
pub const OAUTH_ACCESS_TOKEN: &str = "RIG-SECRET-ACCESS-TOKEN-4a7b22";
/// The key the LLM API fake accepts.
pub const API_KEY: &str = "RIG-SECRET-API-KEY-f05d83";

/// The login name of the mail fakes.
pub const MAIL_USER: &str = "ada@rig.test";
/// The login name of the DAV and Nextcloud fakes.
pub const DAV_USER: &str = "ada";
/// The OAuth client the planted tokens belong to.
pub const OAUTH_CLIENT: &str = "rig-client";
/// The scope the planted refresh token carries.
pub const OAUTH_SCOPE: &str = "rig";

/// Every planted secret.
pub const ALL: [&str; 6] = [
    MAIL_PASSWORD,
    DAV_PASSWORD,
    NEXTCLOUD_APP_PASSWORD,
    OAUTH_REFRESH_TOKEN,
    OAUTH_ACCESS_TOKEN,
    API_KEY,
];
