//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).

mod common;
mod http;
mod imap;
mod login;
mod pop3;
mod sieve;
mod smtp;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
