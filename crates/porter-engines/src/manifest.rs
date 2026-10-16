//! The pinned manifest: for each engine, each processor and each backend it is built for, the
//! exact files to download (address, SHA-256, size) and the seal the unpacked tree must have.
//! Compiled in. Nothing a caller says picks a file: a caller names an engine and a backend and
//! the manifest says the rest.

use crate::host::{Arch, Backend, BackendChoice, Gpus, preference};
use crate::id::EngineId;
use crate::release::{OsRelease, PackageSource};

/// One file to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artifact {
    /// An `https` address.
    pub url: &'static str,
    /// The SHA-256 of the file, 64 lower-case hex digits.
    pub sha256: &'static str,
    /// The size in bytes.
    pub size: u64,
}

/// An engine that is a Python program with its libraries: its own Python, `uv` to fill an
/// environment from a lock, and the lock that names every wheel by hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PythonEnv {
    /// The `uv` program's archive (a `tar.gz` with one top directory).
    pub uv: Artifact,
    /// The Python's archive (a `tar.gz` with one top directory).
    pub python: Artifact,
    /// The seal the unpacked Python tree must have.
    pub python_seal: &'static str,
    /// The requirements, every package pinned `==` with its `--hash` lines.
    pub lock: &'static str,
    /// The package whose files make the entry set (its directory in `site-packages`).
    pub package: &'static str,
    /// About how many bytes the lock's packages download.
    pub wheels_bytes: u64,
}

/// What a row downloads and how it becomes a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Archives (`tar.gz`, one top directory each), unpacked in order into one tree.
    Archives(&'static [Artifact]),
    /// A Python environment built from a lock.
    Python(PythonEnv),
}

/// One thing the manifest can install: an engine's build for a processor and a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The engine.
    pub engine: EngineId,
    /// The processor it is built for.
    pub arch: Arch,
    /// What it computes with.
    pub backend: Backend,
    /// The release, as the engine's project names it.
    pub version: &'static str,
    /// The oldest NVIDIA driver it runs on (CUDA builds); `None` when no card is needed.
    pub min_nvidia_driver: Option<u32>,
    /// What to download.
    pub delivery: Delivery,
    /// The program, as a path inside the tree.
    pub program: &'static str,
    /// The seal the whole tree must have; `None` for an engine whose tree cannot be known
    /// before it is built (a Python environment): its Python is pinned instead.
    pub tree_seal: Option<&'static str>,
}

impl Row {
    /// The directory this row installs into, under the engine's directory.
    pub fn dir_name(&self) -> String {
        format!("{}-{}", self.version, self.backend.slug())
    }

    /// The files it downloads, in the order they are unpacked.
    pub fn artifacts(&self) -> Vec<Artifact> {
        match self.delivery {
            Delivery::Archives(files) => files.to_vec(),
            Delivery::Python(env) => vec![env.uv, env.python],
        }
    }

    /// How many bytes it downloads.
    pub fn download_size(&self) -> u64 {
        match self.delivery {
            Delivery::Archives(files) => files.iter().map(|file| file.size).sum(),
            Delivery::Python(env) => env.uv.size + env.python.size + env.wheels_bytes,
        }
    }

    fn driver_fits(&self, gpus: &Gpus) -> bool {
        self.min_nvidia_driver
            .is_none_or(|least| gpus.nvidia_driver.is_some_and(|found| found >= least))
    }
}

/// What this computer is: the processor, the cards, the distro, and any forced backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// The processor.
    pub arch: Arch,
    /// The cards and the NVIDIA driver.
    pub gpus: Gpus,
    /// The distro.
    pub release: OsRelease,
    /// A backend the person forced.
    pub choice: BackendChoice,
}

impl Host {
    /// The computer whose files are under `root` (`/` for this one; a test's own directory
    /// otherwise): its processor is this program's, its cards are what `<root>/sys/class/drm`
    /// lists, its distro is `<root>/etc/os-release`. `None` for a processor no engine is built
    /// for.
    pub fn read(root: &std::path::Path, choice: BackendChoice) -> Option<Self> {
        let under = |path: &str| root.join(path.trim_start_matches('/'));
        Some(Self {
            arch: Arch::current()?,
            gpus: Gpus::read(
                &under("/sys/class/drm"),
                &under("/sys/module/nvidia/version"),
            ),
            release: OsRelease::parse(
                &std::fs::read_to_string(under("/etc/os-release")).unwrap_or_default(),
            ),
            choice,
        })
    }
}

/// What can be installed for an engine here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    /// The build to download, when the manifest has one for this computer.
    pub row: Option<&'static Row>,
    /// The distro's package, when this release has one.
    pub package: Option<&'static PackageSource>,
}

/// The rows and packages, as a value so a test can hold a small one.
#[derive(Debug, Clone, Copy)]
pub struct Manifest {
    rows: &'static [Row],
}

impl Manifest {
    /// The manifest this build pins.
    pub fn pinned() -> Self {
        Self {
            rows: crate::rows::ROWS,
        }
    }

    /// A manifest of these rows.
    pub fn of(rows: &'static [Row]) -> Self {
        Self { rows }
    }

    /// Every row.
    pub fn rows(&self) -> &'static [Row] {
        self.rows
    }

    /// The row for exactly this engine, processor and backend.
    pub fn row(&self, engine: EngineId, arch: Arch, backend: Backend) -> Option<&'static Row> {
        self.rows
            .iter()
            .find(|row| row.engine == engine && row.arch == arch && row.backend == backend)
    }

    /// The row whose directory is `dir_name` under `engine`'s: how an installed tree finds its
    /// pins.
    pub fn row_in_dir(&self, engine: EngineId, dir_name: &str) -> Option<&'static Row> {
        self.rows
            .iter()
            .find(|row| row.engine == engine && row.dir_name() == dir_name)
    }

    /// The best row for `host`: the first backend it prefers that has a row its driver fits.
    pub fn choose(&self, engine: EngineId, host: &Host) -> Option<&'static Row> {
        preference(&host.gpus, host.choice)
            .into_iter()
            .filter_map(|backend| self.row(engine, host.arch, backend))
            .find(|row| row.driver_fits(&host.gpus))
    }

    /// What can be installed for `engine` on `host`.
    pub fn offer(&self, engine: EngineId, host: &Host) -> Offer {
        Offer {
            row: self.choose(engine, host),
            package: crate::rows::packages(engine)
                .iter()
                .find(|package| package.exists_on(&host.release)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::GpuVendor;

    const FILE: Artifact = Artifact {
        url: "https://example.org/x.tar.gz",
        sha256: "00",
        size: 10,
    };
    static FILES: [Artifact; 2] = [FILE, FILE];

    const fn row(backend: Backend, driver: Option<u32>) -> Row {
        Row {
            engine: EngineId::LlamaCpp,
            arch: Arch::X86_64,
            backend,
            version: "b1",
            min_nvidia_driver: driver,
            delivery: Delivery::Archives(&FILES),
            program: "llama-server",
            tree_seal: None,
        }
    }

    static ROWS: [Row; 3] = [
        row(Backend::Cpu, None),
        row(Backend::Vulkan, None),
        row(Backend::Cuda, Some(580)),
    ];

    fn host(vendors: &[GpuVendor], driver: Option<u32>) -> Host {
        Host {
            arch: Arch::X86_64,
            gpus: Gpus {
                vendors: vendors.to_vec(),
                nvidia_driver: driver,
            },
            release: OsRelease::default(),
            choice: BackendChoice::Detect,
        }
    }

    #[test]
    fn the_row_follows_the_cards_and_a_driver_too_old_for_cuda_falls_back_to_vulkan() {
        let manifest = Manifest::of(&ROWS);
        let backend = |host: &Host| {
            manifest
                .choose(EngineId::LlamaCpp, host)
                .map(|row| row.backend)
        };
        assert_eq!(backend(&host(&[], None)), Some(Backend::Cpu));
        assert_eq!(
            backend(&host(&[GpuVendor::Amd], None)),
            Some(Backend::Vulkan)
        );
        assert_eq!(
            backend(&host(&[GpuVendor::Nvidia], Some(580))),
            Some(Backend::Cuda)
        );
        assert_eq!(
            backend(&host(&[GpuVendor::Nvidia], Some(550))),
            Some(Backend::Vulkan)
        );
        assert_eq!(manifest.choose(EngineId::Vllm, &host(&[], None)), None);
    }

    #[test]
    fn a_row_is_found_by_its_directory_and_sizes_add_up() {
        let manifest = Manifest::of(&ROWS);
        let found = manifest.row_in_dir(EngineId::LlamaCpp, "b1-vulkan");
        assert_eq!(found.map(|row| row.backend), Some(Backend::Vulkan));
        assert_eq!(found.map(Row::download_size), Some(20));
        assert!(manifest.row_in_dir(EngineId::LlamaCpp, "b1-rocm").is_none());
        assert!(manifest.row_in_dir(EngineId::Vllm, "b1-vulkan").is_none());
    }
}
