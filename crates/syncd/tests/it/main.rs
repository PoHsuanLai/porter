//! The crate's integration tests: one executable, one module per topic (CONVENTIONS.md, Tests).
// The PIM supervisor's future nests the whole account service's generics; peephole's caller
// types (porter-dbus ProcCallers) take its layout past the default depth of 128.
#![recursion_limit = "256"]

mod binary;
mod common;
mod confirm_bus;
mod daemon;
mod dist;
mod drive_listing;
mod fake_words;
mod gdrive_bus;
mod google_pim_bus;
mod googlerig;
mod gphotos_bus;
mod graph_bus;
mod graph_calendar_bus;
#[cfg(feature = "photos-picker")]
mod photos_picker_bus;
mod pim_bus;
mod resolve_bus;
mod retry_bus;
mod settings_grant_bus;
mod show_in_files_bus;
mod stalled;
mod start;
mod storage1_bus;
mod storage_bus;
mod sync_bus;
mod webdav_bus;

#[test]
fn every_module_is_declared() {
    test_kit::guard::every_module_is_declared(
        env!("CARGO_MANIFEST_DIR"),
        "tests/it",
        include_str!("main.rs"),
    );
}
