//! The helper's side: one request per connection, answered in turn.
//!
//! [`Helper::answer`] checks the caller (its uid, SO_PEERCRED, must be the helper's own), reads
//! the request and does it through a [`Mounts`]: [`Fusermount`] on a real computer, a fake in the
//! tests. Only a folder directly in `~/Cloud` is ever touched: the request names it, never a
//! path. A mount is refused when the folder is a link or not a folder, holds files of its own, is
//! another file system's mount, or is a drive's folder still served (one live mount per name); a
//! drive's folder left by a crash (its connection gone) is cleared first. Each refusal answers
//! with its [`Code`] and is said in full on stderr.

use crate::mountinfo;
use crate::wire::{self, Access, BadMessage, Code, FSTYPE, FolderName, Reply, Request};
use rustix::io::{Errno, FdFlags};
use rustix::process::Uid;
use std::fs::DirBuilder;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long a caller has to send its request.
const REQUEST_WITHIN: Duration = Duration::from_secs(5);

/// What is at a drive's folder now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum At {
    /// Nothing is mounted there.
    Nothing,
    /// A drive's folder (`fuse.porter`); `live` when its connection is still served.
    Ours {
        /// Whether something still serves it.
        live: bool,
    },
    /// Another file system, of this type.
    Other(String),
}

/// The step that mounts, and what the helper asks about mounts.
pub trait Mounts {
    /// What is mounted at `path`.
    fn at(&self, path: &Path) -> At;
    /// Mounts a drive's folder at `path`: the open FUSE connection.
    ///
    /// # Errors
    /// The mount failed.
    fn mount(&self, path: &Path, access: Access) -> io::Result<OwnedFd>;
    /// Unmounts what is at `path`, lazily, and ends its FUSE connection, so whoever served it
    /// sees it gone at once even while a file there is still open.
    ///
    /// # Errors
    /// The unmount failed.
    fn unmount(&self, path: &Path) -> io::Result<()>;
}

/// Mounts with the system's setuid `fusermount3`, as fuser's own mount without libfuse does:
/// `fusermount3 -o <options> -- <path>` with `_FUSE_COMMFD` naming one end of a socket pair, on
/// which it sends back the `/dev/fuse` fd it opened and mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fusermount {
    /// The program: its path, or a name the host chose to leave to the system to find.
    pub program: String,
    /// Where the kernel lists FUSE connections, to end one after its unmount.
    pub connections: PathBuf,
}

impl Fusermount {
    /// Mounting with `program`, ending a connection in the kernel's usual place.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            connections: PathBuf::from(mountinfo::FUSE_CONNECTIONS),
        }
    }
}

impl Fusermount {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .env_remove("LISTEN_PID")
            .env_remove("LISTEN_FDS")
            .env_remove("LISTEN_FDNAMES")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
}

fn failed(program: &str, output: &std::process::Output) -> io::Error {
    let said = String::from_utf8_lossy(&output.stderr);
    io::Error::other(format!("{program} ({}): {}", output.status, said.trim()))
}

impl Mounts for Fusermount {
    fn at(&self, path: &Path) -> At {
        let entries = mountinfo::read();
        match mountinfo::at(&entries, path) {
            None => At::Nothing,
            Some(entry) if entry.fstype == FSTYPE => At::Ours {
                live: !std::fs::metadata(path)
                    .is_err_and(|e| e.raw_os_error() == Some(Errno::NOTCONN.raw_os_error())),
            },
            Some(entry) => At::Other(entry.fstype.clone()),
        }
    }

    fn mount(&self, path: &Path, access: Access) -> io::Result<OwnedFd> {
        let (ours, theirs) = rustix::net::socketpair(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::STREAM,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )?;
        // fusermount3 must inherit its end. The helper answers one request at a time on one
        // thread, so no other child is started while this end is inheritable.
        rustix::io::fcntl_setfd(&theirs, FdFlags::empty())?;
        let child = self
            .command()
            .arg("-o")
            .arg(access.options())
            .arg("--")
            .arg(path)
            .env(
                "_FUSE_COMMFD",
                std::os::fd::AsRawFd::as_raw_fd(&theirs).to_string(),
            )
            .spawn()?;
        drop(theirs);
        let received = wire::receive(&ours);
        drop(ours);
        let output = child.wait_with_output()?;
        match received {
            Ok((_, Some(fd))) => Ok(fd),
            Ok((_, None)) => Err(failed(&self.program, &output)),
            Err(why) => Err(io::Error::other(format!(
                "{why}; {}",
                failed(&self.program, &output)
            ))),
        }
    }

    fn unmount(&self, path: &Path) -> io::Result<()> {
        let entries = mountinfo::read();
        let connection = mountinfo::at(&entries, path).map(mountinfo::Entry::connection);
        let output = self.command().args(["-u", "-z", "--"]).arg(path).output()?;
        if !output.status.success() {
            return Err(failed(&self.program, &output));
        }
        // A lazy unmount leaves the connection up while a file there is open, and the caller's
        // session waiting on it: end it. A connection already gone (a stale mount) is fine.
        if let Some(connection) = connection
            && let Err(why) = mountinfo::abort(&self.connections, connection)
            && why.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "porter-mountd: connection {connection} of {} was not ended: {why}",
                path.display()
            );
        }
        Ok(())
    }
}

/// Why a request was refused.
#[derive(Debug)]
#[non_exhaustive]
pub enum Refusal {
    /// The caller is another user.
    Peer(u32),
    /// The request could not be read.
    Read(io::Error),
    /// The request is not of the format.
    Message(BadMessage),
    /// The folder is a link or not a folder, so not a folder in Cloud.
    NotInCloud(PathBuf),
    /// The folder holds files of its own.
    NotEmpty(PathBuf),
    /// Another file system is mounted at the folder.
    OtherMount(PathBuf, String),
    /// A drive is still shown there.
    AlreadyShown(PathBuf),
    /// Cloud or the folder could not be made or read.
    Folder(PathBuf, io::Error),
    /// The mount failed.
    Mount(PathBuf, io::Error),
    /// The unmount failed.
    Unmount(PathBuf, io::Error),
}

impl Refusal {
    /// What the caller is answered.
    pub fn code(&self) -> Code {
        match self {
            Refusal::NotEmpty(_) | Refusal::OtherMount(..) => Code::FolderInUse,
            Refusal::AlreadyShown(_) => Code::AlreadyShown,
            Refusal::Unmount(..) => Code::NotTakenAway,
            Refusal::Peer(_)
            | Refusal::Read(_)
            | Refusal::Message(_)
            | Refusal::NotInCloud(_)
            | Refusal::Folder(..)
            | Refusal::Mount(..) => Code::NotHere,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Peer(uid) => write!(f, "a caller of uid {uid}, not this user, was refused"),
            Refusal::Read(why) => write!(f, "the request could not be read: {why}"),
            Refusal::Message(why) => write!(f, "the request was refused: {why}"),
            Refusal::NotInCloud(path) => {
                write!(f, "{} is a link or not a folder", path.display())
            }
            Refusal::NotEmpty(path) => write!(f, "{} is not empty", path.display()),
            Refusal::OtherMount(path, fstype) => {
                write!(f, "{} is a mount of {fstype}", path.display())
            }
            Refusal::AlreadyShown(path) => write!(f, "{} is still served", path.display()),
            Refusal::Folder(path, why) => write!(f, "{}: {why}", path.display()),
            Refusal::Mount(path, why) => write!(f, "{} was not mounted: {why}", path.display()),
            Refusal::Unmount(path, why) => {
                write!(f, "{} was not unmounted: {why}", path.display())
            }
        }
    }
}

impl std::error::Error for Refusal {}

/// Whether a caller of uid `peer` may ask a helper running as `owner`: only the same user.
pub fn peer_allowed(peer: Uid, owner: Uid) -> bool {
    peer == owner
}

/// The helper: the person's `~/Cloud` and the mounts it does there.
#[derive(Debug)]
pub struct Helper<M> {
    cloud: PathBuf,
    mounts: M,
    owner: Uid,
}

impl<M: Mounts> Helper<M> {
    /// A helper for this process's user, showing drives in `cloud` (`~/Cloud`).
    pub fn new(cloud: PathBuf, mounts: M) -> Self {
        Self::for_owner(cloud, mounts, rustix::process::geteuid())
    }

    /// A helper that answers callers of uid `owner` only.
    pub fn for_owner(cloud: PathBuf, mounts: M, owner: Uid) -> Self {
        Self {
            cloud,
            mounts,
            owner,
        }
    }

    /// Its mounts.
    pub fn mounts(&self) -> &M {
        &self.mounts
    }

    fn folder(&self, name: &FolderName) -> Result<PathBuf, Refusal> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.cloud)
            .map_err(|why| Refusal::Folder(self.cloud.clone(), why))?;
        let cloud = self
            .cloud
            .canonicalize()
            .map_err(|why| Refusal::Folder(self.cloud.clone(), why))?;
        Ok(cloud.join(name.as_str()))
    }

    /// Does `request`: a mount's answer is the open connection.
    ///
    /// # Errors
    /// Why it was refused.
    pub fn handle(&self, request: &Request) -> Result<Option<OwnedFd>, Refusal> {
        match request {
            Request::Mount { name, access } => self.mount(name, *access).map(Some),
            Request::Unmount { name } => self.unmount(name).map(|()| None),
        }
    }

    fn mount(&self, name: &FolderName, access: Access) -> Result<OwnedFd, Refusal> {
        let path = self.folder(name)?;
        match self.mounts.at(&path) {
            At::Nothing => {}
            At::Ours { live: true } => return Err(Refusal::AlreadyShown(path)),
            At::Ours { live: false } => {
                // Left by a crash: nothing answers it. Clear it, then go on.
                self.mounts
                    .unmount(&path)
                    .map_err(|why| Refusal::Unmount(path.clone(), why))?;
            }
            At::Other(fstype) => return Err(Refusal::OtherMount(path, fstype)),
        }
        match std::fs::symlink_metadata(&path) {
            Err(why) if why.kind() == io::ErrorKind::NotFound => DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .map_err(|why| Refusal::Folder(path.clone(), why))?,
            Err(why) => return Err(Refusal::Folder(path, why)),
            Ok(meta) if !meta.is_dir() => return Err(Refusal::NotInCloud(path)),
            Ok(_) => {
                let mut entries =
                    std::fs::read_dir(&path).map_err(|why| Refusal::Folder(path.clone(), why))?;
                if entries.next().is_some() {
                    return Err(Refusal::NotEmpty(path));
                }
            }
        }
        self.mounts
            .mount(&path, access)
            .map_err(|why| Refusal::Mount(path, why))
    }

    fn unmount(&self, name: &FolderName) -> Result<(), Refusal> {
        let Ok(cloud) = self.cloud.canonicalize() else {
            // No Cloud folder: nothing is shown in it.
            return Ok(());
        };
        let path = cloud.join(name.as_str());
        match self.mounts.at(&path) {
            At::Nothing => Ok(()),
            At::Ours { .. } => self
                .mounts
                .unmount(&path)
                .map_err(|why| Refusal::Unmount(path, why)),
            At::Other(fstype) => Err(Refusal::OtherMount(path, fstype)),
        }
    }

    fn take(&self, stream: &UnixStream) -> Result<Option<OwnedFd>, Refusal> {
        let peer = rustix::net::sockopt::socket_peercred(stream)
            .map_err(|why| Refusal::Read(why.into()))?;
        stream
            .set_read_timeout(Some(REQUEST_WITHIN))
            .map_err(Refusal::Read)?;
        // Read whoever asks to the end (at most a message), so the refusal reaches them rather
        // than a reset. An fd the caller sent is closed here, unused.
        let read = wire::receive(stream);
        if !peer_allowed(peer.uid, self.owner) {
            return Err(Refusal::Peer(peer.uid.as_raw()));
        }
        let (bytes, _) = read.map_err(Refusal::Read)?;
        let request = Request::decode(&bytes).map_err(Refusal::Message)?;
        self.handle(&request)
    }

    /// Answers the request on `stream`, with the refusal's code when it is refused.
    ///
    /// # Errors
    /// Why it was refused (the caller has been told), or the answer could not be sent (a mount
    /// made for it is left unserved, and cleared by the next request for that name).
    pub fn answer(&self, stream: &UnixStream) -> Result<(), Refusal> {
        let outcome = self.take(stream);
        let reply = match &outcome {
            Ok(_) => Reply::Done,
            Err(refusal) => Reply::Refused(refusal.code()),
        };
        let fd = outcome
            .as_ref()
            .ok()
            .and_then(|fd| fd.as_ref().map(AsFd::as_fd));
        let _ = stream.set_write_timeout(Some(REQUEST_WITHIN));
        let sent = wire::send(stream, &reply.encode(), fd);
        outcome?;
        sent.map_err(Refusal::Read)
    }

    /// Answers each connection on `listener` in turn, for as long as the process lives; each
    /// refusal is said on stderr.
    pub fn serve(&self, listener: &UnixListener) -> ! {
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(why) = self.answer(&stream) {
                        eprintln!("porter-mountd: {why}");
                    }
                }
                Err(why) => {
                    eprintln!("porter-mountd: a caller was not accepted: {why}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

/// The socket systemd passed this process (socket activation: the caller took it from its
/// environment), or else one bound at `$XDG_RUNTIME_DIR/porter/mountd.sock` for a helper
/// started by hand.
///
/// # Errors
/// There is no runtime directory, or binding failed.
pub fn listener(
    activated: Option<UnixListener>,
    runtime_dir: Option<&Path>,
) -> io::Result<UnixListener> {
    if let Some(listener) = activated {
        return Ok(listener);
    }
    let runtime_dir = runtime_dir.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "there is no runtime directory (XDG_RUNTIME_DIR)",
        )
    })?;
    bind_at(&wire::socket_in(runtime_dir))
}

/// Binds the socket at `socket`, 0600 in a 0700 folder, after removing one left by a helper that
/// is gone.
///
/// # Errors
/// Another helper answers there, or binding failed.
pub fn bind_at(socket: &Path) -> io::Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    if UnixStream::connect(socket).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another porter-mountd answers at {}", socket.display()),
        ));
    }
    if std::fs::symlink_metadata(socket).is_ok_and(|meta| meta.file_type().is_socket()) {
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
