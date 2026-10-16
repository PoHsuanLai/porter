//! The program `pkexec` runs to install an engine for everyone on this computer.
//!
//! `porter-engine-helper install <engine> <backend>`, nothing else. It prints one report line at
//! a time on its standard output (`progress <thousandths>`, then `done` or `failed <why>`), and
//! stops when its standard input is closed or says `cancel`. All it knows of what to download
//! is compiled in.

use porter_engines::{Arch, OsRelease, Roots};
use porter_engines_install::helper::{Helper, Report, parse_args};
use porter_engines_install::{CancelToken, Fetcher};
use porter_http::HyperHttp;
use std::io::{BufRead, Write};
use std::process::ExitCode;

fn say(report: Report) {
    let mut out = std::io::stdout().lock();
    // A closed output means the caller has gone: there is nobody to tell.
    let _told = writeln!(out, "{}", report.line()).and_then(|()| out.flush());
}

/// Cancels when the caller says so or goes away.
fn watch_stdin(cancel: CancelToken) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if line.is_ok_and(|text| text.trim() == "cancel") {
                break;
            }
        }
        cancel.cancel();
    });
}

fn main() -> ExitCode {
    let Ok(request) = parse_args(std::env::args_os().skip(1)) else {
        say(Report::Failed(porter_engines::Failure::Unsupported));
        return ExitCode::from(2);
    };
    let Some(arch) = Arch::current() else {
        say(Report::Failed(porter_engines::Failure::Unsupported));
        return ExitCode::from(2);
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        say(Report::Failed(porter_engines::Failure::Other));
        return ExitCode::from(1);
    };
    let release = OsRelease::parse(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
    let helper = Helper::new(
        Fetcher::new(HyperHttp::new()),
        Roots::new(Roots::SYSTEM.into(), std::path::PathBuf::new()),
        arch,
        release,
    );
    let cancel = CancelToken::new();
    watch_stdin(cancel.clone());
    let done = runtime.block_on(async {
        helper
            .run(request, &mut |report| say(report), &cancel)
            .await
    });
    say(done);
    match done {
        Report::Done => ExitCode::SUCCESS,
        Report::Progress(_) | Report::Failed(_) => ExitCode::from(1),
    }
}
