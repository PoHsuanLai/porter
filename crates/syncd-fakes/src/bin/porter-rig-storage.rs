//! `porter-rig-storage --dir <scratch> [--name <bus name>]`
//!
//! A fake `org.quire.Storage1` at `/org/quire/Storage1` on the session bus
//! (`DBUS_SESSION_BUS_ADDRESS`), for building Files before syncd serves the real one. It plants
//! two drives of real files under `<scratch>/drives/` (a OneDrive and a Google Drive), prints
//! their folders, and moves their states as the real ones will: Download Now fills a progress
//! fraction ten times a second, Keep Downloaded and Remove Download change what they say. Lines
//! on stdin change the world (`offline gdrive`, `upload <path>`, `conflict <path>`, ...; an
//! unknown line prints them all). SIGTERM or SIGINT ends it; at the end of stdin it keeps serving.
//!
//! The name is `org.quire.Sync1`, syncd's, unless `--name` gives another: run it on a private bus
//! (`dbus-run-session -- ...`) or where no syncd runs, or point Files at the other name. Test
//! tooling, never installed.

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use syncd_fakes::storage::{FakeStorage, World, serve};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal::unix::{SignalKind, signal};
use zbus::object_server::SignalEmitter;

/// A fake Storage1 for building Files.
#[derive(Debug, Parser)]
#[command(name = "porter-rig-storage", version)]
struct Args {
    /// The scratch folder: the drives' files go under `drives/` in it.
    #[arg(long)]
    dir: PathBuf,
    /// The bus name to own.
    #[arg(long, default_value = "org.quire.Sync1")]
    name: String,
}

fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("porter-rig-storage: {why}");
    ExitCode::FAILURE
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Args { dir, name } = Args::parse();
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        return fail("cannot listen for SIGTERM");
    };
    let world = match World::plant(&dir) {
        Ok(world) => world,
        Err(why) => {
            return fail(format!(
                "cannot plant the drives under {}: {why}",
                dir.display()
            ));
        }
    };
    for (id, details) in world.drives() {
        let path = syncd_fakes::storage::word_of(&details, porter_dbus::sync::DRIVE_KEY_PATH)
            .unwrap_or_default();
        eprintln!("porter-rig-storage: drive {id} at {path}");
    }
    let storage = FakeStorage::new(world);
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(why) => return fail(format!("no session bus: {why}")),
    };
    if let Err(why) = serve(&connection, &name, storage.clone()).await {
        return fail(format!("cannot serve org.quire.Storage1 as {name}: {why}"));
    }
    let emitter = match SignalEmitter::new(&connection, porter_dbus::sync::STORAGE_PATH) {
        Ok(emitter) => emitter,
        Err(why) => return fail(format!("cannot send signals: {why}")),
    };
    eprintln!("porter-rig-storage: serving org.quire.Storage1 as {name}");
    eprintln!("porter-rig-storage: {}", syncd_fakes::storage::HELP);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut reading = true;
    loop {
        tokio::select! {
            _ = term.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
            line = lines.next_line(), if reading => {
                let Ok(Some(line)) = line else {
                    reading = false;
                    continue;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let said = storage
                    .apply(&emitter, |world, changed| match world.command(&line) {
                        Ok((moved, drives)) => {
                            *changed = moved;
                            (None, drives)
                        }
                        Err(help) => (Some(help), false),
                    })
                    .await;
                if let Some(help) = said {
                    eprintln!("porter-rig-storage: {help}");
                }
            }
        }
    }
    ExitCode::SUCCESS
}
