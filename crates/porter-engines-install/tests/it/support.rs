//! A fake release: an archive that unpacks into a tiny engine, and the row that pins it.

use porter_engines::{
    Arch, Artifact, Backend, Delivery, EngineId, Roots, Row, file_digest, seal_of,
};
use porter_engines_install::Fetcher;
use porter_fake_servers::tls;
use porter_http_reqwest::ReqwestHttp;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use test_kit::Scratch;

/// An archive and what pins it.
pub struct Fixture {
    pub archive: Vec<u8>,
    pub sha256: String,
    pub seal: String,
}

pub fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// An archive of `top/llama-server` (a script saying `body`) and `top/lib/x.so`.
pub fn fixture(scratch: &Scratch, name: &str, body: &str) -> Fixture {
    let work = scratch.path().join(format!("fixture-{name}"));
    let top = work.join("top");
    std::fs::create_dir_all(top.join("lib")).expect("dirs");
    let program = top.join("llama-server");
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).expect("program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
    std::fs::write(top.join("lib/x.so"), "library").expect("library");
    let archive = work.join("a.tar.gz");
    let made = Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .arg("top")
        .status()
        .expect("tar runs");
    assert!(made.success());
    Fixture {
        archive: std::fs::read(&archive).expect("archive"),
        sha256: file_digest(&archive).expect("digest").1,
        seal: seal_of(&top).expect("seal").as_hex().to_owned(),
    }
}

/// A row that installs `fixture`, served at `url`; `seal` is what it pins (the fixture's own,
/// or a wrong one).
pub fn row(url: &str, fixture: &Fixture, seal: &str, size: u64) -> &'static Row {
    let files: &'static [Artifact] = Box::leak(
        vec![Artifact {
            url: leak(url.to_owned()),
            sha256: leak(fixture.sha256.clone()),
            size,
        }]
        .into_boxed_slice(),
    );
    Box::leak(Box::new(Row {
        engine: EngineId::LlamaCpp,
        arch: Arch::X86_64,
        backend: Backend::Cpu,
        version: "b-test",
        min_nvidia_driver: None,
        delivery: Delivery::Archives(files),
        program: "llama-server",
        tree_seal: Some(leak(seal.to_owned())),
    }))
}

/// A downloader that trusts the scratch CA and does not wait long.
pub fn fetcher() -> Fetcher {
    Fetcher::new(ReqwestHttp::with_extra_roots(vec![tls::ca_der().to_vec()])).with_timing(
        Duration::from_secs(5),
        1,
        Duration::from_millis(10),
    )
}

/// Roots inside a scratch directory.
pub fn roots(dir: &Path) -> Roots {
    Roots::new(dir.join("system"), dir.join("person"))
}
