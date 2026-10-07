//! `porter-rig-secrets`: a fake `org.freedesktop.secrets` on the session bus
//! (`DBUS_SESSION_BUS_ADDRESS`), in memory, plain algorithm only. Run it in a jail before the
//! real accountd so `Oo7Secrets` has somewhere to put the credentials. SIGTERM ends it; its
//! secrets are gone with it. Test tooling, never installed.

use clap::Parser;
use std::process::ExitCode;
use tokio::signal::unix::{SignalKind, signal};

/// A fake Secret Service for a jailed scenario.
#[derive(Debug, Parser)]
#[command(name = "porter-rig-secrets", version)]
struct Args {}

fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("porter-rig-secrets: {why}");
    ExitCode::FAILURE
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Args {} = Args::parse();
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        return fail("cannot listen for SIGTERM");
    };
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(why) => return fail(format!("no session bus: {why}")),
    };
    let _kept = match porter_rig::secrets_service::serve(&connection).await {
        Ok(shared) => shared,
        Err(why) => return fail(format!("cannot serve org.freedesktop.secrets: {why}")),
    };
    eprintln!("porter-rig-secrets: serving org.freedesktop.secrets");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    ExitCode::SUCCESS
}
