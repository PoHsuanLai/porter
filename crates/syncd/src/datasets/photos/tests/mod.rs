//! Photos end to end: two machines (two libraries in separate scratch directories, each with
//! its own journals and engines) against one fake WebDAV server on loopback (acceptance 5),
//! and the registration and wipe seams. No network beyond loopback; fixtures are tiny files
//! generated per test, never committed.

mod acceptance;
mod registration;
mod rig;
