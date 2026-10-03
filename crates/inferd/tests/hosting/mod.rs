//! The hosted daemon under test: a private bus, the real `Inference1` object over real seams, fake
//! OpenAI-compatible engines on Unix sockets, and a scratch directory standing in for HOME. The
//! engine host is a recorder that starts nothing, so no process runs and no GPU is touched.
#![allow(dead_code)]

pub mod bus;
pub mod engine;
pub mod entries;
pub mod rig;
