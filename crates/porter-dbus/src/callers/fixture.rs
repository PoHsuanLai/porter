//! A directory tree standing for `/proc`, for the test builds of the daemons (feature
//! `test-proc-root`) and porter's own tests: the processes of an acceptance run sit in no unit and
//! have no systemd to ask. Never part of a default build.
//!
//! The tree holds, per process, `<pid>/cgroup`, optionally `<pid>/root/.flatpak-info` and
//! `<pid>/exe` (a symlink to the process's image), and per service unit `units/<unit>` (the
//! decimal pid of its main process, the stand-in for the unit manager's `MainPID`). Every file of
//! the world the tree stands for is root-owned. Where the tree has no `exe` link:
//!
//! - the main process of a unit runs the first image of that unit's row (`*` written `fixture`);
//! - any other process runs [`STAND_IN_IMAGE`], which no row names.
//!
//! The unit manager's facts for a unit are those of a healthy one: a fragment in the system's
//! folder, no drop-ins, not transient, starting that image, clearing the loader variables.

use super::standing::caller_of;
use super::translate::peephole_table;
use super::{Caller, CallerTable, ProcCallers};
use crate::BusConnection;
use peephole::{Follow, Pinned, Prober, ProcFs};
use peephole_bus::{BusPeers, ManagerCheck, UnitError, UnitSource};
use peephole_core::{
    CgroupPath, DropIns, ExePath, FileTrust, Fragment, Loader, Pid, Pin, Transient, Uid, UnitFacts,
    UnitName, judge,
};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The image of a process the tree gives no image and that is no unit's main process.
pub(super) const STAND_IN_IMAGE: &str = "/usr/bin/porter-fixture-app";

/// The file system of the fixture world.
#[derive(Debug, Clone)]
pub(super) struct DirProc {
    root: PathBuf,
    /// Each service unit and the image its main process runs.
    images: Vec<(String, String)>,
}

impl DirProc {
    pub(super) fn new(root: PathBuf, table: &CallerTable) -> Self {
        let images = table
            .callers
            .iter()
            .filter_map(|row| {
                let unit = row.unit.clone()?;
                let image = row.images().first()?.replace('*', "fixture");
                Some((unit, image))
            })
            .collect();
        Self { root, images }
    }

    fn not_found() -> io::Error {
        io::Error::from(io::ErrorKind::NotFound)
    }

    /// The image of the process in `pid_dir` when the tree states none.
    fn stand_in(&self, pid_dir: &Path) -> String {
        self.main_of_unit(pid_dir)
            .and_then(|unit| {
                self.images
                    .iter()
                    .find(|(name, _)| *name == unit)
                    .map(|(_, image)| image.clone())
            })
            .unwrap_or_else(|| STAND_IN_IMAGE.to_owned())
    }

    /// The unit whose main process the tree says the process in `pid_dir` is.
    fn main_of_unit(&self, pid_dir: &Path) -> Option<String> {
        let text = std::fs::read_to_string(pid_dir.join("cgroup")).ok()?;
        let unit = CgroupPath::from_proc(&text).ok()?.leaf().to_owned();
        let pid = std::fs::read_to_string(self.root.join("units").join(&unit)).ok()?;
        (pid.trim() == pid_dir.file_name()?.to_str()?).then_some(unit)
    }
}

impl ProcFs for DirProc {
    fn proc_root(&self) -> &Path {
        &self.root
    }

    fn read_capped(&self, path: &Path, cap: u64) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)?;
        match u64::try_from(bytes.len()).is_ok_and(|n| n > cap) {
            true => Err(io::Error::new(io::ErrorKind::InvalidData, "over the cap")),
            false => Ok(bytes),
        }
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        match std::fs::read_link(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let pid_dir = path.parent().ok_or_else(Self::not_found)?;
                Ok(PathBuf::from(self.stand_in(pid_dir)))
            }
            other => other,
        }
    }

    fn trust_of(&self, _path: &Path, _follow: Follow) -> io::Result<FileTrust> {
        Ok(FileTrust::RootOnly)
    }

    fn list_dir(&self, _path: &Path) -> io::Result<Vec<PathBuf>> {
        Err(Self::not_found())
    }
}

/// The unit manager of the fixture world: `units/<unit>` names a unit's main process.
#[derive(Debug, Clone)]
pub(super) struct DirUnits {
    root: PathBuf,
    fs: DirProc,
}

impl DirUnits {
    pub(super) fn new(root: PathBuf, fs: DirProc) -> Self {
        Self { root, fs }
    }

    /// The facts of the unit `name`, if the tree states a main process for it.
    pub(super) fn facts(&self, name: &UnitName) -> Option<UnitFacts> {
        let text = std::fs::read_to_string(self.root.join("units").join(name.as_str())).ok()?;
        let pid = Pid::new(text.trim().parse().ok()?)?;
        let image = self
            .fs
            .read_link(&self.root.join(format!("{pid}/exe")))
            .ok()?;
        Some(UnitFacts {
            name: name.clone(),
            main_pid: Some(pid),
            fragment: Fragment::RootOwned,
            drop_ins: DropIns::None,
            transient: Transient::No,
            exec: ExePath::new(image.to_str()?).ok(),
            exec_trust: Some(FileTrust::RootOnly),
            loader: Loader::Cleared,
        })
    }
}

impl UnitSource for DirUnits {
    async fn unit(&self, name: &UnitName) -> Result<UnitFacts, UnitError> {
        self.facts(name).ok_or(UnitError::NotLoaded)
    }
}

impl ProcCallers {
    /// As [`ProcCallers::new`], reading the fixture tree `proc_root` instead of `/proc` and the
    /// user manager (module doc). Only in a test build (feature `test-proc-root`).
    pub fn with_proc_root(
        connection: BusConnection,
        table: CallerTable,
        proc_root: PathBuf,
    ) -> Self {
        let fs = DirProc::new(proc_root.clone(), &table);
        let units = DirUnits::new(proc_root, fs.clone());
        let peers: BusPeers<DirProc, DirUnits> = BusPeers::with_proc(
            connection,
            peephole_table(&table),
            ManagerCheck::Unchecked,
            fs,
        )
        .with_units(units);
        Self::over(table, super::proc::Peers::Tree(peers))
    }

    /// The caller that is process `pid` of the fixture tree at `proc_root`, as
    /// [`ProcCallers::with_proc_root`] judges it once the bus has named the pid (the process is
    /// taken as alive and held at connect time). Only in a test build.
    pub fn caller_of_pid(proc_root: &Path, pid: u32, table: &CallerTable) -> Option<Caller> {
        let fs = DirProc::new(proc_root.to_owned(), table);
        let units = DirUnits::new(proc_root.to_owned(), fs.clone());
        let who = Pinned::introduced(Pid::new(pid)?, Uid::new(1000), Pin::AtConnect);
        let mut facts = Prober::new(fs).probe(&who, None).ok()?;
        facts.unit = facts.cgroup.unit_leaf().and_then(|unit| units.facts(&unit));
        caller_of(&judge(&facts, &peephole_table(table)), table)
    }
}
