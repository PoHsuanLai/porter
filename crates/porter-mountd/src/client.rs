//! The caller's side: syncd asks the helper to show a drive, and gets the open FUSE connection.

use crate::wire::{self, Access, Code, FolderName, Reply, Request};
use std::io;
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long the helper has to answer (it runs `fusermount3`, which is quick).
const ANSWER_WITHIN: Duration = Duration::from_secs(30);

/// Why the helper did not do what was asked.
#[derive(Debug)]
#[non_exhaustive]
pub enum AskError {
    /// No helper listens at the socket (it is not there, or nothing answers it): the caller
    /// may mount by itself.
    Absent,
    /// The helper refused, for this reason.
    Refused(Code),
    /// The exchange failed, or the answer was not of the format.
    Broken(io::Error),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::Absent => f.write_str("porter-mountd is not running"),
            AskError::Refused(code) => write!(f, "porter-mountd refused: {code}"),
            AskError::Broken(why) => write!(f, "porter-mountd did not answer: {why}"),
        }
    }
}

impl std::error::Error for AskError {}

fn connect(socket: &Path) -> Result<UnixStream, AskError> {
    match UnixStream::connect(socket) {
        Ok(stream) => Ok(stream),
        Err(why)
            if matches!(
                why.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(AskError::Absent)
        }
        Err(why) => Err(AskError::Broken(why)),
    }
}

/// Sends `request` on `stream` and reads the reply, with the fd beside it when there is one.
///
/// # Errors
/// The socket's, or an answer not of the format.
pub fn ask(stream: &UnixStream, request: &Request) -> io::Result<(Reply, Option<OwnedFd>)> {
    stream.set_read_timeout(Some(ANSWER_WITHIN))?;
    stream.set_write_timeout(Some(ANSWER_WITHIN))?;
    wire::send(stream, &request.encode(), None)?;
    stream.shutdown(Shutdown::Write)?;
    let (bytes, fd) = wire::receive(stream)?;
    let reply =
        Reply::decode(&bytes).map_err(|why| io::Error::new(io::ErrorKind::InvalidData, why))?;
    Ok((reply, fd))
}

/// Asks the helper at `socket` to show a drive at `~/Cloud/<name>`: the open FUSE connection,
/// for the caller to serve.
///
/// # Errors
/// No helper, its refusal, or a failed exchange.
pub fn mount(socket: &Path, name: &FolderName, access: Access) -> Result<OwnedFd, AskError> {
    let stream = connect(socket)?;
    let request = Request::Mount {
        name: name.clone(),
        access,
    };
    match ask(&stream, &request).map_err(AskError::Broken)? {
        (Reply::Done, Some(fd)) => Ok(fd),
        (Reply::Done, None) => Err(AskError::Broken(io::Error::new(
            io::ErrorKind::InvalidData,
            "the answer came with no connection",
        ))),
        (Reply::Refused(code), _) => Err(AskError::Refused(code)),
    }
}

/// Asks the helper at `socket` to stop showing the drive at `~/Cloud/<name>`.
///
/// # Errors
/// No helper, its refusal, or a failed exchange.
pub fn unmount(socket: &Path, name: &FolderName) -> Result<(), AskError> {
    let stream = connect(socket)?;
    let request = Request::Unmount { name: name.clone() };
    match ask(&stream, &request).map_err(AskError::Broken)? {
        (Reply::Done, _) => Ok(()),
        (Reply::Refused(code), _) => Err(AskError::Refused(code)),
    }
}
