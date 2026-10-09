//! The hosted daemon under test: a private bus, the real `Inference1` object over real seams, fake
//! OpenAI-compatible engines on Unix sockets, and a scratch directory standing in for HOME. The
//! engine host is a recorder that starts nothing, so no process runs and no GPU is touched.
//! `accountd` and `cloud` are the fakes of the hosted models: accountd on the bus, and a provider
//! over TLS on loopback. `tailnet` is a fake Tailscale and the addresses of a test's computers.
#![allow(dead_code)]

pub mod accountd;
pub mod agent;
pub mod bus;
pub mod cloud;
pub mod engine;
pub use porter_router::testkit::entries;
pub mod lab;
pub mod rig;
pub mod speech_host;
pub mod tailnet;
