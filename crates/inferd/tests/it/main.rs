//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! `attached`, `engine_group`, `engine_start` and `shutdown` start this test executable again as
//! a fake child, by test path (`shutdown::fake_inferd`).

mod agents;
mod agents_local;
mod attached;
mod cloud;
mod daemon;
mod dist;
mod engine_group;
mod engine_start;
mod hosted;
mod hosting;
mod places;
mod probed;
mod proc_root;
mod replayed;
mod scores;
mod serve;
mod settings_module;
mod shutdown;
mod speech;
mod start;
mod support;
mod tailnet;
mod voice_chat;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
