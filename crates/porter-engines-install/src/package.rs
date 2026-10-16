//! Installing an engine as the distro's own package, with the distro's own package manager at
//! its fixed path. Only the manifest names the package.

use crate::progress::CancelToken;
use porter_engines::{Failure, PackageManager, PackageSource};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// What the package manager's wait ended in.
enum Ended {
    Exited(std::io::Result<std::process::ExitStatus>),
    Stopped,
}

/// The arguments that refresh the package lists first, for the managers that need it.
fn refresh_args(manager: PackageManager) -> Option<&'static [&'static str]> {
    match manager {
        PackageManager::Apt => Some(&["update"]),
        PackageManager::Dnf | PackageManager::Pacman => None,
    }
}

async fn run(program: &Path, args: &[&str], cancel: &CancelToken) -> Result<(), Failure> {
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Failure::Package)?;
    let ended = tokio::select! {
        status = child.wait() => Ended::Exited(status),
        () = cancel.cancelled() => Ended::Stopped,
    };
    match ended {
        Ended::Exited(Ok(status)) if status.success() => Ok(()),
        Ended::Exited(_) => Err(Failure::Package),
        Ended::Stopped => {
            let _killed = child.kill().await;
            Err(Failure::Cancelled)
        }
    }
}

/// Installs `source`'s package with the package manager at `program` (the manager's own fixed
/// path, [`PackageManager::program`], unless a test puts another in its place).
///
/// # Errors
/// [`Failure::Package`] when the manager fails, [`Failure::Cancelled`] when stopped.
pub async fn install_package(
    program: &Path,
    source: &PackageSource,
    cancel: &CancelToken,
) -> Result<(), Failure> {
    if let Some(args) = refresh_args(source.manager) {
        // Stale lists are the only reason to look; the install says if that was not it.
        let _refreshed = run(program, args, cancel).await;
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
    }
    let mut args: Vec<&str> = source.manager.install_args().to_vec();
    args.push(source.name);
    run(program, &args, cancel).await
}
