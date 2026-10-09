//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! Separate targets, each with its reason in Cargo.toml: `attached`, `engine_group`,
//! `engine_start` and `shutdown` start this test executable again as a fake child by test name,
//! which a module path would change. They reach the shared rig through `#[path]`.

mod agents;
mod agents_local;
mod cloud;
mod dist;
mod hosted;
mod hosting;
mod places;
mod probed;
mod proc_root;
mod replayed;
mod scores;
mod serve;
mod settings_module;
mod speech;
mod start;
mod support;
mod tailnet;
mod voice_chat;
