use super::*;
use porter_engines::{Arch, Artifact, Backend, Delivery, Record, Row, Scope, seal_of};
use std::os::unix::fs::PermissionsExt;
use test_kit::Scratch;

static NO_FILES: [Artifact; 0] = [];

fn held(_: &Path) -> bool {
    true
}

/// An install of a pinned row made by hand under `roots`: its tree, its seal and its record.
fn installed_row(roots: &Roots) -> (Row, PathBuf) {
    let dir = roots
        .engine_dir(Scope::JustMe, EngineId::LlamaCpp)
        .join("b-test-cpu");
    let tree = dir.join("engine");
    std::fs::create_dir_all(&tree).expect("tree");
    let program = tree.join("llama-server");
    std::fs::write(&program, "#!/bin/sh\n").expect("program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let seal = seal_of(&tree).expect("seal").as_hex().to_owned();
    let row = Row {
        engine: EngineId::LlamaCpp,
        arch: Arch::X86_64,
        backend: Backend::Cpu,
        version: "b-test",
        min_nvidia_driver: None,
        delivery: Delivery::Archives(&NO_FILES),
        program: "llama-server",
        tree_seal: Some(Box::leak(seal.clone().into_boxed_str())),
    };
    Record {
        engine: "llama_cpp".into(),
        version: "b-test".into(),
        backend: "cpu".into(),
        seal,
        entries: Default::default(),
        venv_seal: None,
    }
    .write(&dir.join("installed.json"))
    .expect("record");
    (row, program)
}

fn installed(dir: &Scratch) -> (Installed, PathBuf) {
    let roots = Roots::new(dir.path().join("system"), dir.path().join("person"));
    let (row, program) = installed_row(&roots);
    let manifest = Manifest::of(Box::leak(vec![row].into_boxed_slice()));
    (
        Installed::new(roots, manifest, held, Arc::new(|| None)),
        program,
    )
}

#[test]
fn an_install_of_ours_stands_ready_until_a_check_finds_its_tree_changed() {
    let dir = Scratch::new("installed-guard");
    let (installed, program) = installed(&dir);
    let guard = installed.guard();
    assert!(matches!(
        installed.standing(EngineId::LlamaCpp),
        Standing::Ready(found) if found.version.as_deref() == Some("b-test")
    ));
    assert_eq!(guard.check(&program), Ok(()));
    std::fs::write(program.with_file_name("libplanted.so"), "x").expect("plant");
    assert_eq!(
        guard.check(&program),
        Err(Cause::ProgramChanged {
            program: program.clone()
        })
    );
    assert!(matches!(
        installed.standing(EngineId::LlamaCpp),
        Standing::Changed(_)
    ));
}

#[test]
fn a_program_that_is_not_a_persons_install_passes_and_an_engine_with_no_install_is_not_there() {
    let dir = Scratch::new("installed-other");
    let (installed, _) = installed(&dir);
    assert_eq!(
        installed.guard().check(Path::new("/usr/bin/llama-server")),
        Ok(())
    );
    assert_eq!(installed.standing(EngineId::Vllm), Standing::NotInstalled);
    assert_eq!(installed.standing(EngineId::Strata), Standing::NotInstalled);
}

#[test]
fn a_strata_folder_the_person_pointed_at_counts_as_installed() {
    let dir = Scratch::new("installed-strata");
    let folder = dir.path().join("strata");
    std::fs::create_dir_all(folder.join("serve")).expect("serve");
    std::fs::write(folder.join("serve/server.py"), "").expect("server");
    let python = folder.join(".venv/bin/python");
    std::fs::create_dir_all(python.parent().expect("parent")).expect("bin");
    std::fs::write(&python, "").expect("python");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let roots = Roots::new(dir.path().join("system"), dir.path().join("person"));
    let at = folder.clone();
    let installed = Installed::new(
        roots,
        Manifest::of(&[]),
        held,
        Arc::new(move || Some(at.clone())),
    );
    assert!(matches!(
        installed.standing(EngineId::Strata),
        Standing::Ready(found) if found.program == python
    ));
}
