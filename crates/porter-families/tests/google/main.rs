//! The Google family against a fake Google (its issuer and account APIs on loopback) and a
//! scripted browser: sign-in by loopback with the owner's client, what each service's answer does
//! to the claims, the session's tokens per audience, the seven day rule, revoke, and the guide
//! the owner follows matching the scopes the code asks.

mod drive_photos;
mod guide;
mod rig;
mod session;
mod signin;
