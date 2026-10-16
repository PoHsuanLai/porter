use super::*;
use porter_fake::Scratch;

/// A tree with a trusted directory (`sys`) and the person's (`home`, `run`).
struct Tree(Scratch);

impl Tree {
    fn new() -> Self {
        let tree = Self(Scratch::new("origin"));
        for dir in [
            "sys/x.service.d",
            "home/x.service.d",
            "run/x.service.d",
            "sys/wrong",
        ] {
            std::fs::create_dir_all(tree.0.path().join(dir)).expect("dir");
        }
        tree
    }

    fn file(&self, rel: &str) -> PathBuf {
        let path = self.0.path().join(rel);
        std::fs::write(&path, "").expect("file");
        path
    }

    fn rule(&self) -> UnitRule {
        UnitRule::new(vec![self.0.path().join("sys")], |p| {
            !p.to_string_lossy().contains("not-root")
        })
    }
}

fn facts(definition: Definition, unset: &[&str]) -> UnitFacts {
    UnitFacts {
        invocation: "1".to_owned(),
        definition,
        unset: unset.iter().map(|n| (*n).to_owned()).collect(),
    }
}

fn stored(fragment: PathBuf, drop_ins: Vec<PathBuf>) -> Definition {
    Definition::Stored {
        fragment: Some(fragment),
        drop_ins,
    }
}

#[test]
fn a_trusted_fragment_with_trusted_drop_ins_keeps_its_role() {
    let t = Tree::new();
    let definition = stored(
        t.file("sys/x.service"),
        vec![t.file("sys/x.service.d/10.conf")],
    );
    assert_eq!(
        t.rule()
            .judge("x.service", &facts(definition, &LOADER_VARIABLES)),
        Ok(())
    );
}

#[test]
fn a_definition_from_anywhere_else_is_refused() {
    let t = Tree::new();
    let good = t.file("sys/x.service");
    let dir = t.0.path().to_owned();
    let all = &LOADER_VARIABLES;
    let cases = [
        (
            "home fragment",
            stored(t.file("home/x.service"), vec![]),
            Refusal::Fragment(dir.join("home/x.service")),
        ),
        (
            "runtime drop-in",
            stored(good.clone(), vec![t.file("run/x.service.d/10.conf")]),
            Refusal::DropIn(dir.join("run/x.service.d/10.conf")),
        ),
        (
            "home drop-in",
            stored(good.clone(), vec![t.file("home/x.service.d/10.conf")]),
            Refusal::DropIn(dir.join("home/x.service.d/10.conf")),
        ),
        ("transient", Definition::Transient, Refusal::Transient),
        (
            "no fragment",
            Definition::Stored {
                fragment: None,
                drop_ins: vec![],
            },
            Refusal::NoFragment,
        ),
        (
            "not root-held",
            stored(t.file("sys/not-root.service"), vec![]),
            Refusal::Fragment(dir.join("sys/not-root.service")),
        ),
        (
            "another unit's drop-in dir",
            stored(good.clone(), vec![t.file("sys/wrong/10.conf")]),
            Refusal::DropIn(dir.join("sys/wrong/10.conf")),
        ),
        (
            "a .. path out of the directory",
            stored(dir.join("sys/../home/x.service"), vec![]),
            Refusal::Fragment(dir.join("sys/../home/x.service")),
        ),
    ];
    for (what, definition, want) in cases {
        let got = t.rule().judge("x.service", &facts(definition, all));
        assert_eq!(got, Err(want), "{what}");
    }
}

#[test]
fn a_failed_lookup_and_a_missing_file_are_refused() {
    let t = Tree::new();
    let gone = t.0.path().join("sys/gone.service");
    let got = t.rule().judge(
        "x.service",
        &facts(stored(gone.clone(), vec![]), &LOADER_VARIABLES),
    );
    assert_eq!(got, Err(Refusal::Fragment(gone)));
    assert!(Refusal::Unknown.to_string().contains("did not describe"));
}

#[test]
fn a_unit_must_clear_every_loader_variable() {
    let t = Tree::new();
    let unit = || stored(t.file("sys/x.service"), vec![]);
    let (last, rest) = LOADER_VARIABLES.split_last().expect("names");
    assert_eq!(
        t.rule().judge("x.service", &facts(unit(), rest)),
        Err(Refusal::Unset(vec![(*last).to_owned()]))
    );
    assert_eq!(
        t.rule().judge("x.service", &facts(unit(), &[])),
        Err(Refusal::Unset(
            LOADER_VARIABLES.iter().map(|n| (*n).to_owned()).collect()
        ))
    );
    assert_eq!(
        t.rule()
            .judge("x.service", &facts(unit(), &LOADER_VARIABLES)),
        Ok(())
    );
}

#[test]
fn drop_in_directories_follow_systemds_names() {
    let cases = [
        ("a-b.service.d", "a-b.service", true),
        ("service.d", "a-b.service", true),
        ("a-.service.d", "a-b.service", true),
        ("b-.service.d", "a-b.service", false),
        ("foo@.service.d", "foo@bar.service", true),
        ("other.service.d", "a-b.service", false),
        ("a-b.service", "a-b.service", false),
    ];
    for (dir, unit, want) in cases {
        assert_eq!(drop_in_dir_serves(dir, unit), want, "{dir} for {unit}");
    }
}

#[test]
fn a_fixture_states_its_facts_in_lines() {
    let got = facts_from_text(
        "invocation=ab\nfragment=/f\ndrop_in=/d1\ndrop_in=/d2\nunset=A B\nunset=C\n",
    );
    assert_eq!(got.unset, ["A", "B", "C"]);
    assert_eq!(got.invocation, "ab");
    assert_eq!(
        got.definition,
        stored(PathBuf::from("/f"), vec!["/d1".into(), "/d2".into()])
    );
    assert_eq!(
        facts_from_text("transient\n").definition,
        Definition::Transient
    );
}
