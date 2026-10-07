//! `porter-rig-servers --dir <scratch> [--imap] [--smtp] [--pop3] [--dav] [--nextcloud]
//! [--oauth] [--graph] [--token-lifetime-s N] [--ollama] [--llm-api]
//! [--tls implicit|starttls|plain]`
//!
//! Starts the fakes on 127.0.0.1, writes `<scratch>/rig.json` and `<scratch>/ca.pem`, serves
//! the control endpoint (see `porter_rig::control`) and waits. SIGTERM, SIGINT or
//! `POST /stop` ends it cleanly and removes `rig.json`.

use clap::{Parser, ValueEnum};
use porter_core::Tls;
use porter_rig::control::{Levers, serve_levers};
use porter_rig::rigfile::RigFile;
use porter_rig::servers::{Options, Rig};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TlsArg {
    Implicit,
    Starttls,
    Plain,
}

/// Fake servers for a jailed scenario (test tooling, never installed).
#[derive(Debug, Parser)]
#[command(name = "porter-rig-servers", version)]
struct Args {
    /// The scratch directory: rig.json and ca.pem go here.
    #[arg(long)]
    dir: PathBuf,
    /// An IMAP server.
    #[arg(long)]
    imap: bool,
    /// An SMTP server.
    #[arg(long)]
    smtp: bool,
    /// A POP3 server.
    #[arg(long)]
    pop3: bool,
    /// A plain DAV server.
    #[arg(long)]
    dav: bool,
    /// A Nextcloud.
    #[arg(long)]
    nextcloud: bool,
    /// The OAuth issuer.
    #[arg(long)]
    oauth: bool,
    /// The Graph drive (starts the issuer too: the drive accepts its tokens).
    #[arg(long)]
    graph: bool,
    /// The `expires_in` of the issuer's access tokens, in seconds (the issuer's own: 3600). A
    /// short one makes a daemon refresh again inside a scenario.
    #[arg(long, value_name = "SECONDS")]
    token_lifetime_s: Option<u64>,
    /// An Ollama (stop and start it from the control endpoint).
    #[arg(long)]
    ollama: bool,
    /// An LLM API that wants a bearer key.
    #[arg(long)]
    llm_api: bool,
    /// How the mail servers are secured.
    #[arg(long, value_enum, default_value = "implicit")]
    tls: TlsArg,
}

fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("porter-rig-servers: {why}");
    ExitCode::FAILURE
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let options = Options {
        dir: args.dir.clone(),
        imap: args.imap,
        smtp: args.smtp,
        pop3: args.pop3,
        dav: args.dav,
        nextcloud: args.nextcloud,
        oauth: args.oauth,
        graph: args.graph,
        token_lifetime_s: args.token_lifetime_s,
        ollama: args.ollama,
        llm_api: args.llm_api,
        tls: match args.tls {
            TlsArg::Implicit => Tls::Implicit,
            TlsArg::Starttls => Tls::StartTls,
            TlsArg::Plain => Tls::Plain,
        },
    };
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        return fail("cannot listen for SIGTERM");
    };
    let rig = match Rig::start(&options).await {
        Ok(rig) => rig,
        Err(why) => return fail(format!("cannot start the fakes: {why}")),
    };
    let stop = Arc::new(Notify::new());
    let levers = Levers {
        issuer: rig.issuer(),
        graph: rig.graph(),
        imap: rig.imap(),
        smtp: rig.smtp(),
        pop3: rig.pop3(),
        ollama: rig.ollama(),
        rig: Arc::new(Mutex::new(String::new())),
        stop: Arc::clone(&stop),
    };
    let (control, _control) = match serve_levers(levers.clone()).await {
        Ok(served) => served,
        Err(why) => return fail(format!("cannot serve the control endpoint: {why}")),
    };
    let file = rig.describe(&args.dir, &control);
    match serde_json::to_string_pretty(&file) {
        Ok(text) => {
            *levers
                .rig
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = text;
        }
        Err(why) => return fail(why),
    }
    if let Err(why) = file.write(&args.dir) {
        return fail(format!("cannot write rig.json: {why}"));
    }
    eprintln!(
        "porter-rig-servers: ready, {}",
        RigFile::path(&args.dir).display()
    );
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
        () = stop.notified() => {}
    }
    let _ = std::fs::remove_file(RigFile::path(&args.dir));
    drop(rig);
    ExitCode::SUCCESS
}
