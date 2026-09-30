//! syncd: the sync journal, anchors, the polling scheduler and the dataset plug-ins
//! (design/31 §4.1, §6). A skeleton: it names what it would carry and exits, saying so.

use clap::Parser;
use porter_sync::DatasetKind;
use std::process::ExitCode;

/// porter's sync service (`org.quire.Sync1`).
#[derive(Debug, Parser)]
#[command(name = "syncd", version)]
struct Args {}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Args {} = Args::parse();
    let datasets = [DatasetKind::PhotosOriginals, DatasetKind::PhotosMetadata];
    let rules: Vec<_> = datasets.iter().map(|d| (d, d.conflict_rule())).collect();
    // The daemon's one log path is standard error, prefixed with its name.
    eprintln!(
        "syncd: not implemented: {} is frozen as an interface; {rules:?} have no journal yet",
        porter_dbus::SYNC_BUS
    );
    ExitCode::from(2)
}
