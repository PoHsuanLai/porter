use super::*;
use porter_engines::{Arch, BackendChoice, Gpus, OsRelease};
use porter_engines_install::testkit::{release, row};
use porter_fake::Scratch;
use porter_fake_servers::{start_artifacts, tls};
use porter_http::HyperHttp;
use std::time::Duration;

fn installs(dir: &Scratch, row: Row, helper: HelperProgram) -> (EngineInstalls, Roots) {
    let roots = Roots::new(dir.path().join("system"), dir.path().join("person"));
    let manifest = Manifest::of(Box::leak(vec![row].into_boxed_slice()));
    let parts = Parts {
        roots: roots.clone(),
        manifest,
        host: Host {
            arch: Arch::X86_64,
            gpus: Gpus::default(),
            release: OsRelease::default(),
            choice: BackendChoice::Detect,
        },
        fetcher: Fetcher::new(HyperHttp::with_extra_roots(vec![tls::ca_der().to_vec()]))
            .with_timing(Duration::from_secs(5), 1, Duration::from_millis(10)),
        helper,
        strata_folder: Arc::new(|| None),
    };
    (EngineInstalls::new(parts), roots)
}

fn nothing() -> HelperProgram {
    HelperProgram {
        pkexec: PathBuf::from("/nonexistent/pkexec"),
        program: PathBuf::from("/nonexistent/helper"),
    }
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
    let dir = Scratch::new("installs-person");
    let host = start_artifacts().await.expect("host");
    let made = release(dir.path(), "fixture", "");
    host.put("/e.tar.gz", made.archive.clone());
    let size = made.archive.len() as u64;
    let pinned = row(&host.url("/e.tar.gz"), &made, &made.seal, size);
    let (installs, _) = installs(&dir, pinned, nothing());
    assert_eq!(
        installs.standing(EngineId::LlamaCpp),
        Standing::NotInstalled
    );
    let offer = installs.offer(EngineId::LlamaCpp).expect("an offer");
    assert_eq!(offer.scopes, ["just_me"]);
    assert_eq!(offer.size, Some(size));
    installs
        .start(EngineId::LlamaCpp, Scope::JustMe)
        .expect("started");
    let ended = until(&installs, |now| matches!(now, Standing::Ready(_))).await;
    let Standing::Ready(found) = ended else {
        panic!("{ended:?}")
    };
    assert_eq!(found.version.as_deref(), Some("b-test"));
    assert!(found.program.ends_with("engine/llama-server"));
    assert_eq!(
        installs.offer(EngineId::LlamaCpp).map(|o| o.scopes),
        Some(vec!["just_me".to_owned()])
    );
}

#[tokio::test]
async fn an_install_that_changed_is_refused_at_the_start_and_replaced_by_installing_again() {
    let dir = Scratch::new("installs-guard");
    let host = start_artifacts().await.expect("host");
    let made = release(dir.path(), "fixture", "");
    host.put("/e.tar.gz", made.archive.clone());
    let size = made.archive.len() as u64;
    let pinned = row(&host.url("/e.tar.gz"), &made, &made.seal, size);
    let (installs, roots) = installs(&dir, pinned.clone(), nothing());
    installs
        .start(EngineId::LlamaCpp, Scope::JustMe)
        .expect("started");
    until(&installs, |now| matches!(now, Standing::Ready(_))).await;
    let program = roots.dir(Scope::JustMe, &pinned).program(&pinned);
    let guard = installs.guard();
    assert_eq!(guard.check(&program), Ok(()));
    std::fs::write(program.with_file_name("libplanted.so"), "x").expect("plant");
    assert_eq!(
        guard.check(&program),
        Err(Cause::ProgramChanged {
            program: program.clone()
        })
    );
    assert_eq!(installs.standing(EngineId::LlamaCpp), Standing::Changed);
    installs
        .start(EngineId::LlamaCpp, Scope::JustMe)
        .expect("started again");
    until(&installs, |now| matches!(now, Standing::Ready(_))).await;
    assert_eq!(guard.check(&program), Ok(()));
    assert!(!program.with_file_name("libplanted.so").exists());
}

#[tokio::test]
async fn nothing_for_everyone_without_the_helper_and_a_failure_is_kept_until_the_next_try() {
    let dir = Scratch::new("installs-failed");
    let host = start_artifacts().await.expect("host");
    let made = release(dir.path(), "fixture", "");
    host.put("/e.tar.gz", made.archive.clone());
    let wrong = "2".repeat(64);
    let pinned = row(
        &host.url("/e.tar.gz"),
        &made,
        &wrong,
        made.archive.len() as u64,
    );
    let (installs, _) = installs(&dir, pinned, nothing());
    assert_eq!(
        installs.start(EngineId::LlamaCpp, Scope::AllUsers),
        Err(StartError::Nothing)
    );
    assert_eq!(
        installs.start(EngineId::Vllm, Scope::JustMe),
        Err(StartError::Nothing)
    );
    installs
        .start(EngineId::LlamaCpp, Scope::JustMe)
        .expect("started");
    let ended = until(&installs, |now| matches!(now, Standing::Failed(_))).await;
    assert_eq!(ended, Standing::Failed(Failure::Corrupt));
    // The next try is allowed, and the failure stands until it ends.
    assert!(installs.start(EngineId::LlamaCpp, Scope::JustMe).is_ok());
}
