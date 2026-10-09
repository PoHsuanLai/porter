//! Letting the person's other computers use this computer's models: the one thing the Settings
//! switch "Let my other computers use this computer's models" does besides setting
//! `ai.tailnet.serve`. inferd's service is shut off from every network address but its own; the
//! file porter ships (`dist/inferd-tailnet.conf`, installed as data at
//! `$prefix/share/porter/inferd-tailnet.conf`) opens Tailscale's own addresses and nothing else.
//! It is the person's to turn on, so porter installs it only here, when they say yes.
//!
//! [`TailnetLending`] writes the file into the person's systemd user configuration, or takes it
//! away, then tells the manager to read its files again and restarts inferd. It never sets
//! `ai.tailnet.serve`: Settings does that. Everything it touches is handed in, so a test gives
//! a scratch directory and a [`UnitManager`] that records its calls.
//!
//! ```ignore
//! // The real case: the files of the person's own session, and their session bus.
//! let config = LendingConfig::from_env().ok_or("no home folder")?;
//! let lending = TailnetLending::new(config, SessionUnits::new(connection));
//! match lending.state() {
//!     LendingState::NotInstalled | LendingState::Differs => { /* the switch shows "off" */ }
//!     LendingState::Installed => { /* the switch shows "on" */ }
//!     _ => {}
//! }
//! // The person turned the switch on, and agreed:
//! match lending.enable().await {
//!     Ok(()) => { /* now set ai.tailnet.serve to "on" */ }
//!     Err(error) => show(error.to_string()),   // plain words, ready to show
//! }
//! lending.disable().await?;                    // the switch turned off: idempotent
//! ```

use porter_fs::atomic::AtomicWrite;
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

/// The service whose sandbox the file opens.
pub const INFERD_UNIT: UnitName = UnitName("inferd.service");

/// The directory beside the unit that holds its drop-ins, and the drop-in's name in it.
const DROP_IN_DIR: &str = "inferd.service.d";
const DROP_IN_FILE: &str = "tailnet.conf";

/// The name porter's shipped file has under `share/porter`.
pub const SHIPPED_NAME: &str = "inferd-tailnet.conf";

/// A systemd unit's name, such as `inferd.service`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitName(&'static str);

impl UnitName {
    /// The name as systemd spells it.
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Why the manager did not do what it was asked: a sentence for a log, not for the person.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UnitFailure(pub String);

/// The person's systemd manager, as far as this needs it. The real one is [`SessionUnits`] over
/// the session bus; a test gives one that records its calls.
pub trait UnitManager: Send + Sync {
    /// Reads the unit files again (`daemon-reload`).
    fn reload(&self) -> impl Future<Output = Result<(), UnitFailure>> + Send;
    /// Restarts `unit` and returns when the job has finished.
    fn restart(&self, unit: UnitName) -> impl Future<Output = Result<(), UnitFailure>> + Send;
}

/// Where the shipped file is and where the person's systemd user files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LendingConfig {
    shipped: PathBuf,
    systemd_user_dir: PathBuf,
}

impl LendingConfig {
    /// The shipped file at `shipped`, the drop-in under `systemd_user_dir`
    /// (`$XDG_CONFIG_HOME/systemd/user`).
    pub fn new(shipped: PathBuf, systemd_user_dir: PathBuf) -> Self {
        Self {
            shipped,
            systemd_user_dir,
        }
    }

    /// The shipped file under `prefix` (`<prefix>/share/porter/inferd-tailnet.conf`).
    pub fn under_prefix(prefix: &Path, systemd_user_dir: PathBuf) -> Self {
        Self::new(
            prefix.join("share/porter").join(SHIPPED_NAME),
            systemd_user_dir,
        )
    }

    /// The answers of the process environment (`XDG_CONFIG_HOME`, `HOME`, `XDG_DATA_HOME` and
    /// `XDG_DATA_DIRS`), or `None` when there is no home folder to put the file in.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// As [`LendingConfig::from_env`], over `var` instead of the environment.
    pub fn from_lookup(var: impl Fn(&str) -> Option<OsString>) -> Option<Self> {
        let absolute = |name: &str| {
            var(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home = absolute("HOME");
        let config =
            absolute("XDG_CONFIG_HOME").or_else(|| Some(home.as_ref()?.join(".config")))?;
        let data_home =
            absolute("XDG_DATA_HOME").or_else(|| Some(home.as_ref()?.join(".local/share")));
        let data_dirs = var("XDG_DATA_DIRS")
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
        let candidates: Vec<PathBuf> = data_home
            .into_iter()
            .chain(std::env::split_paths(&data_dirs).filter(|path| path.is_absolute()))
            .map(|dir| dir.join("porter").join(SHIPPED_NAME))
            .collect();
        let shipped = candidates
            .iter()
            .find(|path| path.is_file())
            .or_else(|| candidates.first())?
            .clone();
        Some(Self::new(shipped, config.join("systemd/user")))
    }

    /// Where the shipped file is.
    pub fn shipped(&self) -> &Path {
        &self.shipped
    }

    /// Where the drop-in goes.
    pub fn drop_in(&self) -> PathBuf {
        self.systemd_user_dir.join(DROP_IN_DIR).join(DROP_IN_FILE)
    }
}

/// Whether this computer's models can be reached from the person's other computers, as far as
/// the file says. More may be added: match with a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LendingState {
    /// There is no drop-in: the switch is off.
    NotInstalled,
    /// The drop-in is there and is the one porter ships: the switch is on.
    Installed,
    /// A file is there but it is not the one porter ships (the person edited it, or an older
    /// version wrote it): the switch is off until it is turned on again, which replaces it.
    Differs,
}

/// Why turning the sharing on or off did not work. Each sentence is plain words that Settings
/// can show as they are. More may be added: match with a wildcard.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LendingError {
    /// The file porter ships is not on this computer.
    #[error(
        "This computer is missing a file it needs to share its models, so nothing was changed. \
         Installing the software again puts it back."
    )]
    ShippedFileMissing,
    /// The setting could not be saved in the person's folder.
    #[error(
        "Your setting for sharing this computer's models could not be saved. \
         Check that your home folder can be written to."
    )]
    CouldNotWrite(#[source] io::Error),
    /// The file is saved, but the computer would not read its files again.
    #[error(
        "Your setting was saved, but the computer did not take it up yet. \
         Signing out and back in will apply it."
    )]
    ReloadFailed(#[source] UnitFailure),
    /// The file is saved and read, but the models service did not restart.
    #[error(
        "Your setting was saved, but the models service did not restart. \
         Try again, or sign out and back in."
    )]
    RestartFailed(#[source] UnitFailure),
}

/// Installs and takes away the tailnet drop-in of inferd's unit.
#[derive(Debug)]
pub struct TailnetLending<M> {
    config: LendingConfig,
    manager: M,
}

impl<M: UnitManager> TailnetLending<M> {
    /// Lending by `config`, with `manager` to read the files again and restart inferd.
    pub fn new(config: LendingConfig, manager: M) -> Self {
        Self { config, manager }
    }

    /// What the drop-in says now. A file that cannot be read is not the shipped one, so
    /// `Differs`; no file is `NotInstalled`. A shipped file that cannot be read makes anything
    /// that is there `Differs`.
    pub fn state(&self) -> LendingState {
        match std::fs::read(self.config.drop_in()) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => LendingState::NotInstalled,
            Err(_) => LendingState::Differs,
            Ok(present) => match std::fs::read(self.config.shipped()) {
                Ok(shipped) if shipped == present => LendingState::Installed,
                _ => LendingState::Differs,
            },
        }
    }

    /// Writes the shipped file as the drop-in (replacing a different one), reads the unit
    /// files again and restarts inferd. Done twice it leaves the same file; it still reloads
    /// and restarts, so an earlier try that stopped after the write is finished. The file is
    /// written with small blocking calls.
    pub async fn enable(&self) -> Result<(), LendingError> {
        let bytes =
            std::fs::read(self.config.shipped()).map_err(|_| LendingError::ShippedFileMissing)?;
        write_world_readable(&self.config.drop_in(), &bytes)
            .map_err(LendingError::CouldNotWrite)?;
        self.apply().await
    }

    /// Takes the drop-in away if it is there, reads the unit files again and restarts inferd.
    /// Idempotent, as `enable` is. It never sets `ai.tailnet.serve`.
    pub async fn disable(&self) -> Result<(), LendingError> {
        let drop_in = self.config.drop_in();
        match std::fs::remove_file(&drop_in) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(LendingError::CouldNotWrite(e)),
        }
        if let Some(dir) = drop_in.parent() {
            // Only an empty directory goes; one with the person's own files stays.
            let _ = std::fs::remove_dir(dir);
        }
        self.apply().await
    }

    async fn apply(&self) -> Result<(), LendingError> {
        self.manager
            .reload()
            .await
            .map_err(LendingError::ReloadFailed)?;
        self.manager
            .restart(INFERD_UNIT)
            .await
            .map_err(LendingError::RestartFailed)
    }
}

/// `bytes` at `target` atomically, with mode 0644 whatever the umask says: systemd reads it as
/// the person, but a file other tools show should not depend on how the shell was started.
fn write_world_readable(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let writer = AtomicWrite::SHARED;
    let staged = writer.stage(target, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o644)) {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
    }
    writer.commit(&staged, target)
}

#[cfg(feature = "dbus")]
pub use session::SessionUnits;

#[cfg(feature = "dbus")]
mod session {
    use super::{UnitFailure, UnitManager, UnitName};
    use porter_dbus::{BusConnection, BusStream, SYSTEMD, SYSTEMD_MANAGER, SYSTEMD_PATH};
    use std::pin::Pin;
    use std::time::Duration;
    use zbus::zvariant::OwnedObjectPath;

    /// How long a restart may take before it is called failed. A loaded computer can take a
    /// while to stop and start a service; the person waits behind a spinner, not for ever.
    const JOB_WAIT: Duration = Duration::from_secs(60);

    #[zbus::proxy(interface = "org.freedesktop.systemd1.Manager", gen_blocking = false)]
    trait Manager {
        fn reload(&self) -> zbus::Result<()>;
        fn restart_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
        #[zbus(signal)]
        fn job_removed(
            &self,
            id: u32,
            job: OwnedObjectPath,
            unit: &str,
            result: &str,
        ) -> zbus::Result<()>;
    }

    /// The person's own systemd manager on the session bus
    /// (`org.freedesktop.systemd1` `Manager`): `Reload`, then `RestartUnit(unit, "replace")`
    /// and the wait for that job's `JobRemoved`.
    #[derive(Debug, Clone)]
    pub struct SessionUnits {
        connection: BusConnection,
    }

    impl SessionUnits {
        /// The manager on `connection`, which must be the session bus.
        pub fn new(connection: BusConnection) -> Self {
            Self { connection }
        }

        /// The manager over a new connection to the session bus
        /// (`DBUS_SESSION_BUS_ADDRESS` or the runtime directory's socket).
        pub async fn session() -> Result<Self, UnitFailure> {
            BusConnection::session()
                .await
                .map(Self::new)
                .map_err(failure)
        }

        async fn proxy(&self) -> Result<ManagerProxy<'static>, UnitFailure> {
            ManagerProxy::builder(&self.connection)
                .destination(SYSTEMD)
                .and_then(|b| b.path(SYSTEMD_PATH))
                .and_then(|b| b.interface(SYSTEMD_MANAGER))
                .map_err(failure)?
                .build()
                .await
                .map_err(failure)
        }
    }

    fn failure(error: zbus::Error) -> UnitFailure {
        UnitFailure(error.to_string())
    }

    impl UnitManager for SessionUnits {
        async fn reload(&self) -> Result<(), UnitFailure> {
            self.proxy().await?.reload().await.map_err(failure)
        }

        async fn restart(&self, unit: UnitName) -> Result<(), UnitFailure> {
            let proxy = self.proxy().await?;
            // Subscribe first, so a job that finishes at once is not missed.
            let mut removed = proxy.receive_job_removed().await.map_err(failure)?;
            let job = proxy
                .restart_unit(unit.as_str(), "replace")
                .await
                .map_err(failure)?;
            let wait = async {
                loop {
                    let signal =
                        std::future::poll_fn(|cx| BusStream::poll_next(Pin::new(&mut removed), cx))
                            .await
                            .ok_or_else(|| UnitFailure("the bus closed".to_owned()))?;
                    let args = signal.args().map_err(failure)?;
                    if args.job == job {
                        return match args.result {
                            "done" => Ok(()),
                            other => Err(UnitFailure(format!(
                                "the restart of {} ended as '{other}'",
                                unit.as_str()
                            ))),
                        };
                    }
                }
            };
            tokio::time::timeout(JOB_WAIT, wait)
                .await
                .map_err(|_| UnitFailure("the restart did not finish in time".to_owned()))?
        }
    }
}
