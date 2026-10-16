//! Test support (feature `testing`): a tiny release a test can serve from a fake file host, and
//! the manifest row that pins it. The archive holds `top/llama-server`, a script that does
//! nothing.

use porter_engines::{Arch, Artifact, Backend, Delivery, EngineId, Row, file_digest, seal_of};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// An archive and what pins it.
#[derive(Debug, Clone)]
pub struct Release {
    /// The `tar.gz`.
    pub archive: Vec<u8>,
    /// Its SHA-256.
    pub sha256: String,
    /// The seal of the tree it unpacks to.
    pub seal: String,
}

/// Makes a release under `dir/<name>`; the program says `body`.
///
/// # Panics
/// When `dir` cannot be written or `tar` is missing: this is for tests.
pub fn release(dir: &Path, name: &str, body: &str) -> Release {
    let work = dir.join(name);
    let top = work.join("top");
    std::fs::create_dir_all(&top).expect("dirs");
    let program = top.join("llama-server");
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).expect("program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let archive = work.join("a.tar.gz");
    let made = Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .arg("top")
        .status()
        .expect("tar runs");
    assert!(made.success(), "tar made the archive");
    Release {
        archive: std::fs::read(&archive).expect("archive"),
        sha256: file_digest(&archive).expect("digest").1,
        seal: seal_of(&top).expect("seal").as_hex().to_owned(),
    }
}

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

/// The row that installs `release` from `url`, expecting the tree to have `seal` (the
/// release's own, or a wrong one) and the download to be `size` bytes. llama.cpp, x86-64,
/// processor only.
pub fn row(url: &str, release: &Release, seal: &str, size: u64) -> Row {
    let files: &'static [Artifact] = Box::leak(
        vec![Artifact {
            url: leak(url),
            sha256: leak(&release.sha256),
            size,
        }]
        .into_boxed_slice(),
    );
    Row {
        engine: EngineId::LlamaCpp,
        arch: Arch::X86_64,
        backend: Backend::Cpu,
        version: "b-test",
        min_nvidia_driver: None,
        delivery: Delivery::Archives(files),
        program: "llama-server",
        tree_seal: Some(leak(seal)),
    }
}
