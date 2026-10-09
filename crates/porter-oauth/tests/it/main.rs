//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
//! `fake_issuer` needs the `io` feature.

#[cfg(feature = "io")]
mod fake_issuer;

#[test]
fn every_module_is_declared() {
    porter_fake::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
