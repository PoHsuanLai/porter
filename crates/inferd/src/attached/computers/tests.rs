use super::*;
use crate::catalog::parse_entry_text;
use crate::entries;
use crate::testkit::Scratch;
use std::os::unix::fs::PermissionsExt;

const KEY: &str = "sk-lab-S3CRET-0123456789";

struct Rig {
    scratch: Scratch,
    book: AttachedBook,
}

impl Rig {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let entry = parse_entry_text(&entries::attached()).expect("entry");
        let book =
            AttachedBook::default().with_catalogue(vec![entry], scratch.path().join("sockets"));
        Self { scratch, book }
    }

    fn file(&self) -> PathBuf {
        self.scratch.path().join("state").join("computers.toml")
    }

    fn keys(&self) -> PathBuf {
        self.scratch.path().join("state").join("computer-keys")
    }

    fn computers(&self, hand: &[Attached]) -> Computers {
        Computers::new(
            self.file(),
            self.keys(),
            self.book.clone(),
            AddedFile::default(),
            hand,
        )
    }
}

fn model(id: &str, reach: NewReach, key: Option<&str>) -> NewModel {
    NewModel {
        id: id.to_owned(),
        reach,
        key: key.map(SecretText::new),
    }
}

fn studio(key: Option<&str>) -> NewComputer {
    NewComputer {
        label: "Studio PC".to_owned(),
        models: vec![model(entries::ATTACHED, NewReach::Port(8000), key)],
    }
}

fn by_hand(computer: Option<&str>) -> Attached {
    AttachedEntry {
        socket: Some("/run/user/1000/lab.sock".into()),
        place: Some(Place::MyNetwork),
        computer: computer.map(str::to_owned),
        ..AttachedEntry::default()
    }
    .check(entries::ATTACHED)
    .expect("a hand-written engine")
}

fn place(text: &str) -> PlaceId {
    PlaceId::parse(text).expect("place")
}

#[test]
fn a_computer_is_added_to_the_file_the_keys_and_the_book_and_reads_back_as_the_same_engines() {
    let rig = Rig::new("add");
    let computers = rig.computers(&[]);
    let added = computers.add(studio(Some(KEY)));
    assert_eq!(added, Ok(place("computer:studio-pc")));

    // The book: the engine, under the name the person gave.
    let models = rig.book.models();
    assert_eq!(models.len(), 1);
    let target = models[0].attached.as_ref().expect("attached");
    assert_eq!(
        target.computer.as_ref().map(ComputerName::as_str),
        Some("studio-pc")
    );
    assert_eq!(target.place, Place::MyNetwork);
    assert_eq!(
        rig.book
            .label_of(&ComputerName::parse("studio-pc").expect("name")),
        Some("Studio PC".to_owned())
    );

    // The file holds the label, how it is reached and where the key is; not the key.
    let text = std::fs::read_to_string(rig.file()).expect("file");
    assert!(
        text.contains("Studio PC") && text.contains("port = 8000"),
        "{text}"
    );
    assert!(!text.contains(KEY), "{text}");
    // The key is in a file of its own that only its owner can read.
    let key_path = rig
        .keys()
        .join(format!("studio-pc--{}.key", entries::ATTACHED));
    assert!(text.contains(&key_path.display().to_string()), "{text}");
    assert_eq!(std::fs::read_to_string(&key_path).expect("key"), KEY);
    let mode = std::fs::metadata(&key_path)
        .expect("meta")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    // The engine reads that key, as a hand-written key file is read.
    assert!(target.key.as_ref().expect("key").read().is_ok());

    // At the next start the file gives the same engine, and nothing is refused.
    let file = AddedFile::read(&rig.file()).expect("reads");
    let (engines, refused) = file.attached();
    assert_eq!(refused, Vec::<String>::new());
    assert_eq!(engines.len(), 1);
    assert_eq!(engines[0].computer, target.computer);
    assert_eq!(engines[0].reach, target.reach);
    assert_eq!(file.labels().len(), 1);
}

#[test]
fn what_is_refused_changes_nothing_and_says_so_in_plain_words() {
    let rig = Rig::new("refuse");
    let computers = rig.computers(&[]);
    let one = |id: &str, reach: NewReach, key: Option<&str>| NewComputer {
        label: "Bench".to_owned(),
        models: vec![model(id, reach, key)],
    };
    let ok = NewReach::Port(8000);
    let socket = |path: &str| NewReach::Socket(path.into());
    let cases: Vec<(&str, NewComputer, ComputerError)> = vec![
        (
            "a blank name",
            NewComputer {
                label: "   ".into(),
                ..one(entries::ATTACHED, ok.clone(), None)
            },
            ComputerError::BadName,
        ),
        (
            "a name with nothing to name it by",
            NewComputer {
                label: "!!!".into(),
                ..one(entries::ATTACHED, ok.clone(), None)
            },
            ComputerError::BadName,
        ),
        (
            "no models",
            NewComputer {
                label: "Bench".into(),
                models: vec![],
            },
            ComputerError::NoModels,
        ),
        (
            "a model nobody knows",
            one("no-such-model", ok.clone(), None),
            ComputerError::UnknownModel("no-such-model".into()),
        ),
        (
            "a model that is no id",
            one("Not A Model", ok.clone(), None),
            ComputerError::UnknownModel("Not A Model".into()),
        ),
        (
            "port zero",
            one(entries::ATTACHED, NewReach::Port(0), None),
            ComputerError::BadAddress(entries::ATTACHED.into()),
        ),
        (
            "a relative path",
            one(entries::ATTACHED, socket("lab.sock"), None),
            ComputerError::BadAddress(entries::ATTACHED.into()),
        ),
        (
            "a path too long to use",
            one(
                entries::ATTACHED,
                socket(&format!("/{}", "x".repeat(200))),
                None,
            ),
            ComputerError::BadAddress(entries::ATTACHED.into()),
        ),
        (
            "an empty key",
            one(entries::ATTACHED, ok.clone(), Some("  ")),
            ComputerError::BadKey(entries::ATTACHED.into()),
        ),
        (
            "a key of two lines",
            one(entries::ATTACHED, ok.clone(), Some("a\nb")),
            ComputerError::BadKey(entries::ATTACHED.into()),
        ),
    ];
    for (what, new, want) in cases {
        assert_eq!(computers.add(new), Err(want.clone()), "{what}");
        let words = want.to_string();
        for jargon in [
            "socket", "loopback", "toml", "key_file", "TOML", "API", "json",
        ] {
            assert!(!words.contains(jargon), "{what}: {words}");
        }
        assert!(words.ends_with('.'), "{what}: {words}");
    }
    assert!(rig.book.is_empty());
    assert!(!rig.file().exists());
    assert!(!rig.keys().exists());

    // With the settings file naming a computer and its model, neither can be added again.
    let hand = Rig::new("refuse-hand");
    let computers = hand.computers(&[by_hand(Some("lab"))]);
    assert_eq!(
        computers.add(one(entries::ATTACHED, ok, None)),
        Err(ComputerError::ModelTaken(entries::ATTACHED.into()))
    );
    let same_name = NewComputer {
        label: "Lab".into(),
        models: vec![model("other-model", NewReach::Port(1), None)],
    };
    assert_eq!(computers.add(same_name), Err(ComputerError::AlreadyThere));
    assert!(hand.book.is_empty() && !hand.file().exists());
}

#[test]
fn a_model_can_be_on_one_computer_only_and_a_name_is_taken_once() {
    let rig = Rig::new("twice");
    let computers = rig.computers(&[]);
    let twice = NewComputer {
        label: "Bench".into(),
        models: vec![
            model(entries::ATTACHED, NewReach::Port(8000), None),
            model(entries::ATTACHED, NewReach::Port(8001), None),
        ],
    };
    assert_eq!(
        computers.add(twice),
        Err(ComputerError::ModelTaken(entries::ATTACHED.into()))
    );
    assert!(computers.add(studio(None)).is_ok());
    // The same name, however it is typed.
    for label in ["Studio PC", "studio pc", "  STUDIO-pc "] {
        let again = NewComputer {
            label: label.into(),
            ..studio(None)
        };
        assert_eq!(
            computers.add(again),
            Err(ComputerError::AlreadyThere),
            "{label}"
        );
    }
    // Another computer with the same model.
    let other = NewComputer {
        label: "Other".into(),
        ..studio(None)
    };
    assert_eq!(
        computers.add(other),
        Err(ComputerError::ModelTaken(entries::ATTACHED.into()))
    );
}

#[test]
fn a_computer_that_was_added_is_removed_with_its_models_and_its_keys_and_one_by_hand_is_not() {
    let rig = Rig::new("remove");
    let computers = rig.computers(&[by_hand(Some("lab"))]);
    // The hand-written model is the only attached one, so the added one needs another id: this
    // test's catalogue has one attached entry, so add and remove it on a book without the hand one.
    let free = Rig::new("remove2");
    let computers_free = free.computers(&[]);
    assert!(computers_free.add(studio(Some(KEY))).is_ok());
    let key_path = free
        .keys()
        .join(format!("studio-pc--{}.key", entries::ATTACHED));
    assert!(key_path.exists());
    assert_eq!(computers_free.remove("computer:studio-pc"), Ok(()));
    assert!(free.book.is_empty());
    assert!(!key_path.exists());
    assert_eq!(
        free.book
            .label_of(&ComputerName::parse("studio-pc").expect("name")),
        None
    );
    let file = AddedFile::read(&free.file()).expect("reads");
    assert_eq!(file, AddedFile::default());
    // And it can be added again, by the name alone this time.
    assert!(computers_free.add(studio(None)).is_ok());
    assert_eq!(computers_free.remove("studio-pc"), Ok(()));

    assert_eq!(computers.remove("lab"), Err(ComputerError::AddedByHand));
    assert_eq!(
        computers.remove("computer:lab"),
        Err(ComputerError::AddedByHand)
    );
    assert_eq!(computers.remove("nowhere"), Err(ComputerError::NotThere));
    assert_eq!(
        computers.remove("Not A Name!"),
        Err(ComputerError::NotThere)
    );
    assert!(!rig.file().exists(), "a refused removal writes nothing");
}

#[test]
fn an_engine_on_another_machine_with_no_name_is_the_computer_other_computer_to_the_hand_written_file()
 {
    let names = hand_written(&[by_hand(None)]);
    assert_eq!(
        names.iter().map(ComputerName::as_str).collect::<Vec<_>>(),
        vec!["other-computer"]
    );
    let rig = Rig::new("unnamed");
    let computers = rig.computers(&[by_hand(None)]);
    let clash = NewComputer {
        label: "Other computer".into(),
        models: vec![model("other-model", NewReach::Port(1), None)],
    };
    assert_eq!(computers.add(clash), Err(ComputerError::AlreadyThere));
}

#[test]
fn the_settings_file_wins_a_clash_when_the_two_are_merged() {
    let hand = [by_hand(Some("lab"))];
    let mut added = AddedFile::default();
    added.computers.insert(
        "lab".into(),
        AddedComputer {
            label: "Lab".into(),
            models: BTreeMap::from([(
                entries::ATTACHED.to_owned(),
                AddedModel {
                    port: Some(9),
                    ..AddedModel::default()
                },
            )]),
        },
    );
    let (kept, said) = merged(&hand, &added);
    assert_eq!(kept, vec![]);
    assert_eq!(said.len(), 1, "{said:?}");
    let (kept, said) = merged(&[], &added);
    assert_eq!(kept.len(), 1);
    assert_eq!(said, Vec::<String>::new());
}

#[test]
fn a_file_that_is_not_a_list_of_computers_is_kept_aside_and_the_list_starts_empty() {
    let rig = Rig::new("bad");
    std::fs::create_dir_all(rig.file().parent().expect("dir")).expect("dir");
    std::fs::write(rig.file(), "computers = 3\n").expect("write");
    let problem = AddedFile::read(&rig.file()).expect_err("unusable");
    let AddedProblem::Unusable { aside, .. } = &problem else {
        panic!("unusable, got {problem:?}");
    };
    assert_eq!(
        std::fs::read_to_string(aside).expect("kept"),
        "computers = 3\n"
    );
    assert_eq!(AddedFile::read(&rig.file()), Ok(AddedFile::default()));
    // Nothing about a bad entry reaches a start as an error: it is a line, and the engine is left out.
    let mut file = AddedFile::default();
    file.computers
        .insert("Bad Name".into(), AddedComputer::default());
    file.computers.insert(
        "fine".into(),
        AddedComputer {
            label: "Fine".into(),
            models: BTreeMap::from([("Not A Model".into(), AddedModel::default())]),
        },
    );
    let (engines, refused) = file.attached();
    assert_eq!(engines, vec![]);
    assert_eq!(refused.len(), 2, "{refused:?}");
}

#[test]
fn the_key_is_in_no_debug_output_no_error_and_no_file_but_its_own() {
    let rig = Rig::new("secret");
    let computers = rig.computers(&[]);
    let new = studio(Some(KEY));
    assert!(!format!("{new:?}").contains(KEY));
    computers.add(new).expect("added");
    assert!(!format!("{computers:?}").contains(KEY));
    assert!(!format!("{:?}", rig.book).contains(KEY));
    let mut holding = Vec::new();
    for entry in walk(rig.scratch.path()) {
        if std::fs::read_to_string(&entry).is_ok_and(|text| text.contains(KEY)) {
            holding.push(entry);
        }
    }
    let key_path = rig
        .keys()
        .join(format!("studio-pc--{}.key", entries::ATTACHED));
    assert_eq!(holding, vec![key_path]);
    // A refusal that names the key's model does not name the key.
    let bad = NewComputer {
        models: vec![model(entries::ATTACHED, NewReach::Port(0), Some(KEY))],
        label: "Bench".into(),
    };
    let refused = rig.computers(&[]).add(bad).expect_err("bad address");
    assert!(!format!("{refused:?} {refused}").contains(KEY));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
