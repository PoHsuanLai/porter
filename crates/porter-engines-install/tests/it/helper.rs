use crate::support::{fetcher, fixture, roots, row};
use porter_engines::{Arch, Backend, EngineId, Failure, Manifest, OsRelease, Scope};
use porter_engines_install::CancelToken;
use porter_engines_install::helper::{Helper, Report, Request};
use porter_fake_servers::start_artifacts;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use test_kit::Scratch;

fn request(engine: EngineId) -> Request {
    Request {
        engine,
        backend: Backend::Cpu,
    }
}

/// A package manager that writes what it was asked into `<its program>.log`, and exits `code`.
fn fake_manager(prefix: &Path, name: &str, code: i32) {
    let bin = prefix.join("usr/bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let program = bin.join(name);
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho \"$@\" >> \"$0.log\"\nexit {code}\n"),
    )
    .expect("script");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
}

fn fedora() -> OsRelease {
    OsRelease::parse("ID=fedora\nVERSION_ID=43\n")
}

#[tokio::test]
async fn a_package_is_installed_by_the_distros_manager_and_nothing_is_downloaded() {
    let dir = Scratch::new("helper-package");
    fake_manager(dir.path(), "dnf", 0);
    let roots = roots(dir.path());
    let helper = Helper::new(fetcher(), roots.clone(), Arch::X86_64, fedora())
        .with(Manifest::pinned(), dir.path().to_path_buf());
    let mut told = Vec::new();
    let done = helper
        .run(
            request(EngineId::LlamaCpp),
            &mut |report| told.push(report),
            &CancelToken::new(),
        )
        .await;
    assert_eq!(done, Report::Done);
    let log = std::fs::read_to_string(dir.path().join("usr/bin/dnf.log")).expect("log");
    assert_eq!(log.trim(), "install -y llama-cpp");
    assert!(!roots.root(Scope::AllUsers).exists());
}

#[tokio::test]
async fn a_package_that_will_not_install_falls_back_to_the_pinned_download() {
    let dir = Scratch::new("helper-fallback");
    fake_manager(dir.path(), "dnf", 1);
    let host = start_artifacts().await.expect("host");
    let release = fixture(&dir, "e", "echo hi");
    host.put("/e.tar.gz", release.archive.clone());
    let row = row(
        &host.url("/e.tar.gz"),
        &release,
        &release.seal,
        release.archive.len() as u64,
    );
    let manifest = Manifest::of(Box::leak(vec![*row].into_boxed_slice()));
    let roots = roots(dir.path());
    let helper = Helper::new(fetcher(), roots.clone(), Arch::X86_64, fedora())
        .with(manifest, dir.path().to_path_buf());
    let mut told = Vec::new();
    let done = helper
        .run(
            request(EngineId::LlamaCpp),
            &mut |report| told.push(report),
            &CancelToken::new(),
        )
        .await;
    assert_eq!(done, Report::Done);
    assert!(roots.dir(Scope::AllUsers, row).record().exists());
    assert!(told.contains(&Report::Progress(1000)), "{told:?}");
}

#[tokio::test]
async fn nothing_pinned_for_this_computer_is_unsupported_and_a_package_that_fails_alone_is_a_package_failure()
 {
    let dir = Scratch::new("helper-none");
    let helper = Helper::new(fetcher(), roots(dir.path()), Arch::X86_64, fedora())
        .with(Manifest::pinned(), dir.path().to_path_buf());
    let nothing = helper
        .run(request(EngineId::Strata), &mut |_| {}, &CancelToken::new())
        .await;
    assert_eq!(nothing, Report::Failed(Failure::Unsupported));
    fake_manager(dir.path(), "dnf", 1);
    let alone = Helper::new(fetcher(), roots(dir.path()), Arch::X86_64, fedora())
        .with(Manifest::of(&[]), dir.path().to_path_buf())
        .run(
            request(EngineId::LlamaCpp),
            &mut |_| {},
            &CancelToken::new(),
        )
        .await;
    assert_eq!(alone, Report::Failed(Failure::Package));
}
