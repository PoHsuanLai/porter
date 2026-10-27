//! porter-mountd: listens on its socket (passed by `porter-mountd.socket`, or bound at
//! `$XDG_RUNTIME_DIR/porter/mountd.sock` when started by hand) and shows drives in `~/Cloud`.

use porter_mountd::server::{self, Fusermount, Helper};
use rustix::fs::{FileType, fstat};
use rustix::io::{FdFlags, fcntl_setfd};
use std::io;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        eprintln!("porter-mountd: there is no home folder (HOME)");
        return ExitCode::FAILURE;
    };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let activated = match activated() {
        Ok(activated) => activated,
        Err(why) => {
            eprintln!("porter-mountd: the socket passed is no use: {why}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match server::listener(activated, runtime.as_deref()) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("porter-mountd: not listening: {why}");
            return ExitCode::FAILURE;
        }
    };
    let helper = Helper::new(
        PathBuf::from(home).join("Cloud"),
        Fusermount::new("fusermount3"),
    );
    helper.serve(&listener)
}

/// The listening socket systemd passed, when it passed one to this process. `listenfd` checks
/// `LISTEN_PID` and takes the fd; the close-on-exec flag and the socket check stay here, because
/// `fusermount3` must not inherit the fd and this is a privileged helper.
fn activated() -> io::Result<Option<UnixListener>> {
    if std::env::var("LISTEN_FDS").ok().as_deref() != Some("1") {
        return Ok(None);
    }
    let Some(listener) = listenfd::ListenFd::from_env().take_unix_listener(0)? else {
        return Ok(None);
    };
    fcntl_setfd(&listener, FdFlags::CLOEXEC)?;
    let stat = fstat(&listener)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the fd systemd passed is not a socket",
        ));
    }
    Ok(Some(listener))
}
