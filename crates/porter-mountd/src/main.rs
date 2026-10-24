//! porter-mountd: listens on its socket (passed by `porter-mountd.socket`, or bound at
//! `$XDG_RUNTIME_DIR/porter/mountd.sock` when started by hand) and shows drives in `~/Cloud`.

use porter_mountd::server::{self, Fusermount, Helper};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        eprintln!("porter-mountd: there is no home folder (HOME)");
        return ExitCode::FAILURE;
    };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let listener = match server::listener(runtime.as_deref()) {
        Ok(listener) => listener,
        Err(why) => {
            eprintln!("porter-mountd: not listening: {why}");
            return ExitCode::FAILURE;
        }
    };
    let helper = Helper::new(PathBuf::from(home).join("Cloud"), Fusermount::default());
    helper.serve(&listener)
}
