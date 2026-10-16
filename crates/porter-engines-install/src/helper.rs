//! The privileged helper's side: the only words it accepts, what it says back, and what it does.
//!
//! The helper is started by `pkexec` with a polkit action and runs as root, so it takes nothing
//! from its caller but the name of an engine and the name of a backend. Never an address, a
//! path, a version or a hash: those come from the compiled-in manifest. Anything else is refused
//! before anything is touched.

use crate::fetch::Fetcher;
use crate::installer::{InstallError, Installer};
use crate::package::install_package;
use crate::progress::{CancelToken, Meter};
use porter_engines::{
    Arch, Backend, EngineId, Failure, Manifest, OsRelease, PackageSource, Roots, Row, Scope,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What the helper was asked to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// The engine.
    pub engine: EngineId,
    /// The backend the caller chose.
    pub backend: Backend,
}

/// Why the arguments were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArgError {
    /// Not exactly `install <engine> <backend>`.
    #[error("usage: install <engine> <backend>")]
    Usage,
    /// The engine is not one of ours.
    #[error("unknown engine")]
    UnknownEngine,
    /// The backend is not one of ours.
    #[error("unknown backend")]
    UnknownBackend,
}

/// The arguments after the program's name.
///
/// # Errors
/// See [`ArgError`].
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<Request, ArgError> {
    let words: Vec<String> = args
        .into_iter()
        .map(OsString::into_string)
        .collect::<Result<_, _>>()
        .map_err(|_| ArgError::Usage)?;
    let [verb, engine, backend] = &words[..] else {
        return Err(ArgError::Usage);
    };
    if verb != "install" {
        return Err(ArgError::Usage);
    }
    Ok(Request {
        engine: EngineId::from_slug(engine).ok_or(ArgError::UnknownEngine)?,
        backend: Backend::from_slug(backend).ok_or(ArgError::UnknownBackend)?,
    })
}

/// One line the helper says on its standard output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// This many thousandths done.
    Progress(u32),
    /// Installed.
    Done,
    /// Not installed, and why.
    Failed(Failure),
}

impl Report {
    /// The line, without its newline.
    pub fn line(self) -> String {
        match self {
            Self::Progress(permille) => format!("progress {permille}"),
            Self::Done => "done".to_owned(),
            Self::Failed(failure) => format!("failed {}", failure.slug()),
        }
    }

    /// The report a line says; `None` for any other line.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line == "done" {
            return Some(Self::Done);
        }
        if let Some(rest) = line.strip_prefix("progress ") {
            return rest.parse().ok().map(Self::Progress);
        }
        line.strip_prefix("failed ")
            .and_then(Failure::from_slug)
            .map(Self::Failed)
    }
}

/// What the helper installs with.
#[derive(Debug, Clone)]
pub struct Helper {
    installer: Installer,
    manifest: Manifest,
    arch: Arch,
    release: OsRelease,
    /// Where `/` is for the package managers' fixed paths: `/`, or a test's scratch directory.
    prefix: PathBuf,
}

impl Helper {
    /// A helper that installs under `roots` on a computer of `arch` running `release`.
    pub fn new(fetcher: Fetcher, roots: Roots, arch: Arch, release: OsRelease) -> Self {
        Self {
            installer: Installer::new(fetcher, roots),
            manifest: Manifest::pinned(),
            arch,
            release,
            prefix: PathBuf::from("/"),
        }
    }

    /// The same over another manifest, finding the package managers under `prefix` instead of
    /// `/` (a test's own).
    #[must_use]
    pub fn with(self, manifest: Manifest, prefix: PathBuf) -> Self {
        Self {
            manifest,
            prefix,
            ..self
        }
    }

    /// The package to use and its manager's program, when this release has one and the manager
    /// is here.
    fn package(&self, engine: EngineId) -> Option<(&'static PackageSource, PathBuf)> {
        porter_engines::packages(engine)
            .iter()
            .filter(|source| source.exists_on(&self.release))
            .map(|source| {
                let program = self
                    .prefix
                    .join(source.manager.program().trim_start_matches('/'));
                (source, program)
            })
            .find(|(_, program)| program.exists())
    }

    /// Installs for everyone, saying progress through `report`; returns the final report.
    pub async fn run(
        &self,
        request: Request,
        report: &mut (dyn FnMut(Report) + Send),
        cancel: &CancelToken,
    ) -> Report {
        let row = self
            .manifest
            .row(request.engine, self.arch, request.backend);
        if let Some((source, program)) = self.package(request.engine) {
            match install_package(&program, source, cancel).await {
                Ok(()) => return Report::Done,
                Err(Failure::Cancelled) => return Report::Failed(Failure::Cancelled),
                Err(_) if row.is_none() => return Report::Failed(Failure::Package),
                // A package that would not install falls back to the pinned download.
                Err(_) => {}
            }
        }
        let Some(row) = row else {
            return Report::Failed(Failure::Unsupported);
        };
        self.download(row, report, cancel).await
    }

    async fn download(
        &self,
        row: &'static Row,
        report: &mut (dyn FnMut(Report) + Send),
        cancel: &CancelToken,
    ) -> Report {
        let meter = Meter::new(row.download_size());
        let finished = Arc::new(AtomicBool::new(false));
        let work = async {
            let result = self
                .installer
                .install(Scope::AllUsers, row, &meter, cancel)
                .await;
            finished.store(true, Ordering::SeqCst);
            result
        };
        let ticks = async {
            let mut last = u32::MAX;
            while !finished.load(Ordering::SeqCst) {
                let now = meter.permille();
                if now != last {
                    report(Report::Progress(now));
                    last = now;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            // The last count, whatever the install ended in.
            let now = meter.permille();
            if now != last {
                report(Report::Progress(now));
            }
        };
        let (result, ()) = tokio::join!(work, ticks);
        match result {
            Ok(_) => Report::Done,
            Err(error) => Report::Failed(InstallError::failure(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_install_with_an_engine_and_a_backend_of_ours_is_accepted() {
        assert_eq!(
            parse_args(args(&["install", "llama_cpp", "vulkan"])),
            Ok(Request {
                engine: EngineId::LlamaCpp,
                backend: Backend::Vulkan
            })
        );
        let refused = [
            (args(&[]), ArgError::Usage),
            (args(&["install"]), ArgError::Usage),
            (args(&["install", "llama_cpp"]), ArgError::Usage),
            (
                args(&["install", "llama_cpp", "cpu", "extra"]),
                ArgError::Usage,
            ),
            (args(&["remove", "llama_cpp", "cpu"]), ArgError::Usage),
            (
                args(&["install", "https://evil.example/x", "cpu"]),
                ArgError::UnknownEngine,
            ),
            (
                args(&["install", "../../bin/sh", "cpu"]),
                ArgError::UnknownEngine,
            ),
            (
                args(&["install", "llama_cpp", "/tmp/x"]),
                ArgError::UnknownBackend,
            ),
            (
                args(&["install", "llama_cpp", "b11669"]),
                ArgError::UnknownBackend,
            ),
            (args(&["install", "", ""]), ArgError::UnknownEngine),
        ];
        for (given, why) in refused {
            assert_eq!(parse_args(given.clone()), Err(why), "{given:?}");
        }
        use std::os::unix::ffi::OsStringExt;
        let raw = vec![OsString::from_vec(vec![0xff, 0xfe]); 3];
        assert_eq!(parse_args(raw), Err(ArgError::Usage));
    }

    #[test]
    fn a_report_line_round_trips_and_other_lines_are_nothing() {
        for report in [
            Report::Progress(0),
            Report::Progress(1000),
            Report::Done,
            Report::Failed(Failure::NoSpace),
            Report::Failed(Failure::Declined),
        ] {
            assert_eq!(Report::parse(&report.line()), Some(report));
        }
        for line in [
            "",
            "progress",
            "progress x",
            "failed sudo",
            "ok",
            "done now",
        ] {
            assert_eq!(Report::parse(line), None, "{line:?}");
        }
    }
}
