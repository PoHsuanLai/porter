use crate::support::{fetcher, fixture, row};
use porter_engines::{
    Arch, BackendChoice, EngineId, Failure, Gpus, Host, Manifest, OsRelease, Roots, Scope,
};
use porter_engines_install::{EngineInstalls, HelperProgram, Parts, Standing, StartError};
use porter_fake_servers::start_artifacts;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use test_kit::Scratch;

fn held(_: &Path) -> bool {
    true
}

fn nothing() -> HelperProgram {
    HelperProgram {
        pkexec: PathBuf::from("/nonexistent/pkexec"),
        program: PathBuf::from("/nonexistent/helper"),
    }
}

fn installs(dir: &Scratch, pinned: porter_engines::Row) -> EngineInstalls {
    EngineInstalls::new(Parts {
        roots: Roots::new(dir.path().join("system"), dir.path().join("person")),
        manifest: Manifest::of(Box::leak(vec![pinned].into_boxed_slice())),
        host: Host {
            arch: Arch::X86_64,
            gpus: Gpus::default(),
            release: OsRelease::default(),
            choice: BackendChoice::Detect,
        },
        fetcher: fetcher(),
        helper: nothing(),
        strata_folder: Arc::new(|| None),
        held,
    })
}

async fn until(installs: &EngineInstalls, done: impl Fn(&Standing) -> bool) -> Standing {
    for _ in 0..1000 {
        let now = installs.standing(EngineId::LlamaCpp);
        if done(&now) {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the install did not end");
}

#[tokio::test]
async fn an_install_for_the_person_runs_to_ready_and_is_found_with_its_release() {
    let dir = Scratch::new("jobs-person");
    let host = start_artifacts().await.expect("host");
    let release = fixture(&dir, "a", "echo hi");
    let size = release.archive.len() as u64;
    host.put("/e.tar.gz", release.archive.clone());
    let pinned = *row(&host.url("/e.tar.gz"), &release, &release.seal, size);
    let installs = installs(&dir, pinned);
    assert_eq!(
        installs.standing(EngineId::LlamaCpp),
        Standing::NotInstalled
    );
    let choice = installs.choice(EngineId::LlamaCpp).expect("a choice");
    assert_eq!(
        (choice.scopes, choice.size),
        (vec![Scope::JustMe], Some(size))
    );
    installs
        .start(EngineId::LlamaCpp, Scope::JustMe)
        .expect("started");
    let ended = until(&installs, |now| matches!(now, Standing::Ready(_))).await;
    let Standing::Ready(found) = ended else {
        panic!("{ended:?}")
    };
    assert_eq!(found.version.as_deref(), Some("b-test"));
    assert!(found.program.ends_with("engine/llama-server"));
}

#[tokio::test]
async fn nothing_is_offered_for_everyone_without_the_helper_or_for_an_engine_with_no_row() {
    let dir = Scratch::new("jobs-nothing");
    let host = start_artifacts().await.expect("host");
    let release = fixture(&dir, "b", "echo hi");
    host.put("/e.tar.gz", release.archive.clone());
    let pinned = *row(&host.url("/e.tar.gz"), &release, &release.seal, 1);
    let installs = installs(&dir, pinned);
    assert_eq!(
        installs.start(EngineId::LlamaCpp, Scope::AllUsers),
        Err(StartError::Nothing)
    );
    assert_eq!(
        installs.start(EngineId::Vllm, Scope::JustMe),
        Err(StartError::Nothing)
    );
    assert_eq!(installs.choice(EngineId::Strata), None);
}

#[tokio::test]
async fn a_failure_is_kept_until_it_is_forgotten_or_the_next_try_begins() {
    let dir = Scratch::new("jobs-failed");
    let host = start_artifacts().await.expect("host");
    let release = fixture(&dir, "c", "echo hi");
    let size = release.archive.len() as u64;
    host.put("/e.tar.gz", release.archive.clone());
    let wrong = "2".repeat(64);
    let pinned = *row(&host.url("/e.tar.gz"), &release, &wrong, size);
    let installs = installs(&dir, pinned);
    installs
        .start(EngineId::LlamaCpp, Scope::JustMe)
        .expect("started");
    let ended = until(&installs, |now| matches!(now, Standing::Failed(_))).await;
    assert_eq!(ended, Standing::Failed(Failure::Corrupt));
    installs.forget(EngineId::LlamaCpp);
    assert_eq!(
        installs.standing(EngineId::LlamaCpp),
        Standing::NotInstalled
    );
}
