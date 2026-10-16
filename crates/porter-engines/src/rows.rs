//! The pinned rows. Every address, hash, size and seal below was taken from the project's own
//! release (GitHub's published digest of each asset, checked by downloading it) on 2026-10-17,
//! and each seal was made with `tools/seal.py` over the archive unpacked as the installer
//! unpacks it. Updating an engine is changing a row, and nothing else.

use crate::host::{Arch, Backend};
use crate::id::EngineId;
use crate::manifest::{Artifact, Delivery, PythonEnv, Row};
use crate::release::{PackageManager, PackageSource, Releases, Unnumbered, Verified};

/// llama.cpp release b11669, the build of the project's stable v0.7.0 (its `nightly-tag.txt`
/// names it). The archives are the project's Ubuntu builds: they need a glibc as new as that
/// runner's and `libgomp`, which is not checked on every distro.
static LLAMA_CPU_X86: [Artifact; 1] = [Artifact {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/llama-b11669-bin-ubuntu-x64.tar.gz",
    sha256: "cf07c1c112d709bd67e522017471de57ed0ec8bea031d732c5422351c07f65ac",
    size: 17_851_611,
}];
static LLAMA_VULKAN_X86: [Artifact; 1] = [Artifact {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/llama-b11669-bin-ubuntu-vulkan-x64.tar.gz",
    sha256: "e8709e5ba6c1b093dea7f3de57f4aba842f9a188fba6b465782d65d6fcfa4d9f",
    size: 30_497_889,
}];
static LLAMA_CPU_ARM: [Artifact; 1] = [Artifact {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/llama-b11669-bin-ubuntu-arm64.tar.gz",
    sha256: "9ac6fda2058d35677608a06f751a50122ddfa4ce0ddb7f084466b8e79b6d4a99",
    size: 13_900_160,
}];
static LLAMA_VULKAN_ARM: [Artifact; 1] = [Artifact {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/llama-b11669-bin-ubuntu-vulkan-arm64.tar.gz",
    sha256: "d283522e479c0d532e3a04d715dd852ffb2d95925caa6d41ef2ba53a65e46a51",
    size: 24_187_561,
}];
static LLAMA_ROCM_X86: [Artifact; 1] = [Artifact {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/llama-b11669-bin-ubuntu-rocm-10.0-x64.tar.gz",
    sha256: "02d36f06accd8fdcb2d41078361b21fb2df5d139f169bf4efcb8424a7dbbef01",
    size: 245_070_738,
}];
/// The CUDA build and the CUDA runtime libraries it loads, which the project ships as a second
/// archive; both unpack into one tree.
static LLAMA_CUDA_X86: [Artifact; 2] = [
    Artifact {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/llama-b11669-bin-ubuntu-cuda-13.4-x64.tar.gz",
        sha256: "80878473d27278dbe9c38fb7e704f292a51e889d3c28a1e863137cdee1e2ba4e",
        size: 153_679_828,
    },
    Artifact {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11669/cudart-llama-b11669-bin-ubuntu-cuda-13.4-x64.tar.gz",
        sha256: "1108bd9300a281e8eabb18d3fa028d4240ecd983f9f90dbbbc3297671016d697",
        size: 440_236_547,
    },
];

const fn llama(
    arch: Arch,
    backend: Backend,
    files: &'static [Artifact],
    seal: &'static str,
    min_nvidia_driver: Option<u32>,
) -> Row {
    Row {
        engine: EngineId::LlamaCpp,
        arch,
        backend,
        version: "b11669",
        min_nvidia_driver,
        delivery: Delivery::Archives(files),
        program: "llama-server",
        tree_seal: Some(seal),
    }
}

/// vLLM 0.31.0 from PyPI (CUDA 13 wheels, so an NVIDIA driver of 580 or newer), in an
/// environment of its own: `uv` 0.13.1 and Python 3.12.15 from their projects' releases, and the
/// lock `locks/vllm-0.31.0-x86_64.lock` (made with `uv pip compile --generate-hashes
/// --python-platform x86_64-manylinux_2_31 --python-version 3.12 --no-build`), whose hashes are
/// required. There is no aarch64 row: one package of the lock (tilelang) has no wheel for it.
const VLLM_ENV: PythonEnv = PythonEnv {
    uv: Artifact {
        url: "https://github.com/astral-sh/uv/releases/download/0.13.1/uv-x86_64-unknown-linux-gnu.tar.gz",
        sha256: "eadd9d27c134ab5ac1ffd4473b97a741407f3ea238288f03121fc4deddf90220",
        size: 16_738_706,
    },
    python: Artifact {
        url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261013/cpython-3.12.15%2B20261013-x86_64-unknown-linux-gnu-install_only.tar.gz",
        sha256: "5d546fc004fc2435fa142eb8f42ed8de5eb4725963dc22f7e6671014a80eb560",
        size: 66_983_838,
    },
    python_seal: "b98fe40bf25f1a0ccdc0344436c3ac8fb829edcf9d4e63c5b8a5ed740bd343a2",
    lock: include_str!("locks/vllm-0.31.0-x86_64.lock"),
    package: "vllm",
    wheels_bytes: 4_411_066_545,
};

/// Every row. Strata has none: its releases ship Windows builds only, and on Linux its own
/// setup compiles the engine on the person's computer (docs/INSTALL.md), which is not a pinned
/// download.
pub(crate) static ROWS: &[Row] = &[
    llama(
        Arch::X86_64,
        Backend::Cpu,
        &LLAMA_CPU_X86,
        "c5adb8e554b4967762320d2813ed3ca028518a2cbfc77611134b81e608171451",
        None,
    ),
    llama(
        Arch::X86_64,
        Backend::Vulkan,
        &LLAMA_VULKAN_X86,
        "7926d57f1c500cb018b63aaf180add1262b809447967faaf532fea6c749e361c",
        None,
    ),
    llama(
        Arch::X86_64,
        Backend::Rocm,
        &LLAMA_ROCM_X86,
        "fe06325c93dca56cc4d6a24a52de9f899e3fb906f6559d47a1e2b3f5890002f5",
        None,
    ),
    llama(
        Arch::X86_64,
        Backend::Cuda,
        &LLAMA_CUDA_X86,
        "3145ce98ec25232afdcfe7f5865bd26e44d4bdb818ef9aab34ea6a088f1fe120",
        Some(580),
    ),
    llama(
        Arch::Aarch64,
        Backend::Cpu,
        &LLAMA_CPU_ARM,
        "47af854cec01a80076fc85acbee8b1fedce6cd7f14c32754084295461763c5f9",
        None,
    ),
    llama(
        Arch::Aarch64,
        Backend::Vulkan,
        &LLAMA_VULKAN_ARM,
        "050eca40fe761f92cc4b68ddb1bdf75476a0a361bb77ca56f214ab44a4507076",
        None,
    ),
    Row {
        engine: EngineId::Vllm,
        arch: Arch::X86_64,
        backend: Backend::Cuda,
        version: "0.31.0",
        min_nvidia_driver: Some(580),
        delivery: Delivery::Python(VLLM_ENV),
        program: "venv/bin/python",
        tree_seal: None,
    },
];

/// The distro packages of each engine. A row is only ever offered on a release that was found
/// to ship the package; the ones marked unverified were not checked down to the file list.
pub fn packages(engine: EngineId) -> &'static [PackageSource] {
    match engine {
        // Debian's `forky` and `sid` and Ubuntu's `resolute` (26.04) list it, and each package
        // holds /usr/bin/llama-server (packages.debian.org, packages.ubuntu.com). Arch's
        // `extra/llama-cpp` holds /usr/bin/llama-server (archlinux.org's file list). Fedora's
        // `llama-cpp` is in releases 43 and rawhide (mdapi); its file list was not reachable.
        EngineId::LlamaCpp => &[
            PackageSource {
                manager: PackageManager::Apt,
                name: "llama.cpp-tools",
                releases: Releases::From {
                    id: "debian",
                    version: (14, 0),
                    unnumbered: Unnumbered::New,
                },
                verified: Verified::Yes,
            },
            PackageSource {
                manager: PackageManager::Apt,
                name: "llama.cpp-tools",
                releases: Releases::From {
                    id: "ubuntu",
                    version: (26, 4),
                    unnumbered: Unnumbered::Old,
                },
                verified: Verified::Yes,
            },
            PackageSource {
                manager: PackageManager::Dnf,
                name: "llama-cpp",
                releases: Releases::From {
                    id: "fedora",
                    version: (43, 0),
                    unnumbered: Unnumbered::Old,
                },
                verified: Verified::No,
            },
            PackageSource {
                manager: PackageManager::Pacman,
                name: "llama-cpp",
                releases: Releases::Rolling { id: "arch" },
                verified: Verified::Yes,
            },
        ],
        // No distro packages vLLM or Strata (Arch's official repositories hold neither).
        EngineId::Vllm | EngineId::Strata => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use std::collections::BTreeSet;

    fn hex64(text: &str) -> bool {
        text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    #[test]
    fn every_row_is_a_complete_https_pin() {
        let mut seen = BTreeSet::new();
        for row in ROWS {
            assert!(
                seen.insert((row.engine, row.dir_name(), row.arch as u8)),
                "{row:?} twice"
            );
            assert!(!row.artifacts().is_empty());
            for file in row.artifacts() {
                assert!(file.url.starts_with("https://"), "{}", file.url);
                assert!(hex64(file.sha256), "{}", file.url);
                assert!(file.size > 0);
            }
            assert!(row.tree_seal.is_some_and(hex64) || row.tree_seal.is_none());
            if let Delivery::Python(env) = row.delivery {
                assert!(hex64(env.python_seal));
                assert!(row.tree_seal.is_none());
                assert!(env.lock.contains("--hash=sha256:") && !env.lock.contains("--index-url"));
            } else {
                assert!(
                    row.tree_seal.is_some(),
                    "an archive row is pinned by its seal"
                );
            }
            assert!(!row.program.starts_with('/') && !row.program.contains(".."));
        }
    }

    #[test]
    fn every_engine_has_its_rows_where_a_project_ships_a_build() {
        let manifest = Manifest::pinned();
        let has = |engine, arch, backend| manifest.row(engine, arch, backend).is_some();
        assert!(has(EngineId::LlamaCpp, Arch::X86_64, Backend::Cpu));
        assert!(has(EngineId::LlamaCpp, Arch::Aarch64, Backend::Vulkan));
        assert!(has(EngineId::Vllm, Arch::X86_64, Backend::Cuda));
        assert!(!has(EngineId::Vllm, Arch::Aarch64, Backend::Cuda));
        assert!(ROWS.iter().all(|row| row.engine != EngineId::Strata));
        assert!(ROWS.iter().all(|row| !row.version.is_empty()));
    }
}
