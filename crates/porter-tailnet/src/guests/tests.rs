use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a node id")
}

const NOW: UnixSeconds = UnixSeconds(1_790_000_000);

/// A new empty directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "porter-tailnet-guests-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

#[test]
fn the_first_ask_is_raised_and_the_next_is_only_pending() {
    let guests = Guests::in_memory();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    guests.on_event(move |event| log.lock().unwrap().push(event.clone()));
    assert_eq!(guests.ask(&node("nPI"), "pi", NOW), Ok(AskOutcome::Raised));
    assert_eq!(guests.ask(&node("nPI"), "pi", NOW), Ok(AskOutcome::Pending));
    let told = seen.lock().unwrap().clone();
    assert_eq!(
        told,
        vec![
            GuestEvent::Asked(Ask {
                node: node("nPI"),
                name: "pi".into(),
                since: NOW
            }),
            GuestEvent::Changed
        ],
        "the person is told once"
    );
    assert_eq!(
        guests.state_of(&node("nPI")),
        None,
        "asking is not saying yes"
    );
}

#[test]
fn an_answer_is_kept_by_the_id_and_a_no_is_not_asked_again() {
    let guests = Guests::in_memory();
    guests.ask(&node("nPI"), "pi", NOW).unwrap();
    guests.ask(&node("nOLD"), "old-laptop", NOW).unwrap();
    guests
        .answer(&node("nPI"), GuestAnswer::Allow, NOW)
        .unwrap();
    guests
        .answer(&node("nOLD"), GuestAnswer::Deny, NOW)
        .unwrap();
    assert_eq!(guests.state_of(&node("nPI")), Some(State::Approved));
    assert_eq!(guests.state_of(&node("nOLD")), Some(State::Denied));
    assert!(guests.asking().is_empty());
    // Another computer that takes the first one's name is not the first one.
    assert_eq!(guests.state_of(&node("nOTHER")), None);
}

#[test]
fn only_a_computer_that_is_waiting_can_be_answered() {
    let guests = Guests::in_memory();
    assert_eq!(
        guests.answer(&node("nPI"), GuestAnswer::Allow, NOW),
        Err(GuestError::NotAsking)
    );
    assert_eq!(guests.state_of(&node("nPI")), None);
}

#[test]
fn forgetting_a_record_makes_the_computer_a_stranger_again() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nPI"), "pi", GuestAnswer::Allow, NOW)
        .unwrap();
    assert_eq!(guests.forget(&node("nPI")), Ok(true));
    assert_eq!(guests.state_of(&node("nPI")), None);
    assert_eq!(
        guests.forget(&node("nPI")),
        Ok(false),
        "nothing left to forget"
    );
    assert_eq!(guests.ask(&node("nPI"), "pi", NOW), Ok(AskOutcome::Raised));
}

#[test]
fn forgetting_also_withdraws_a_question() {
    let guests = Guests::in_memory();
    guests.ask(&node("nPI"), "pi", NOW).unwrap();
    assert_eq!(guests.forget(&node("nPI")), Ok(true));
    assert!(guests.asking().is_empty());
}

#[test]
fn the_questions_waiting_at_once_are_limited() {
    let guests = Guests::in_memory();
    for n in 0..MOST_ASKS {
        guests.ask(&node(&format!("n{n}")), "x", NOW).unwrap();
    }
    assert_eq!(
        guests.ask(&node("nLAST"), "x", NOW),
        Err(GuestError::TooManyAsking)
    );
    // One that already waits is not a new question.
    assert_eq!(guests.ask(&node("n0"), "x", NOW), Ok(AskOutcome::Pending));
    guests.answer(&node("n0"), GuestAnswer::Deny, NOW).unwrap();
    assert_eq!(guests.ask(&node("nLAST"), "x", NOW), Ok(AskOutcome::Raised));
}

#[test]
fn the_person_may_allow_a_computer_that_never_asked() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nFRIEND"), "  friends-pc ", GuestAnswer::Allow, NOW)
        .unwrap();
    let rows = guests.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "friends-pc");
    assert_eq!(rows[0].state, RowState::Approved);
    guests
        .set(&node("nNAMELESS"), "   ", GuestAnswer::Deny, NOW)
        .unwrap();
    assert!(
        guests.rows().iter().any(|r| r.name == "nNAMELESS"),
        "a computer given no name is shown by its id"
    );
}

#[test]
fn the_list_shows_answers_and_questions_by_name() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nB"), "bravo", GuestAnswer::Allow, NOW)
        .unwrap();
    guests
        .set(&node("nA"), "alpha", GuestAnswer::Deny, NOW)
        .unwrap();
    guests.ask(&node("nC"), "charlie", UnixSeconds(5)).unwrap();
    let rows: Vec<(String, RowState)> = guests
        .rows()
        .into_iter()
        .map(|r| (r.name, r.state))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("alpha".into(), RowState::Denied),
            ("bravo".into(), RowState::Approved),
            ("charlie".into(), RowState::Asking)
        ]
    );
}

#[test]
fn answers_survive_a_restart_in_a_file_only_the_owner_reads() {
    let dir = scratch("restart");
    let path = dir.join("state").join("guests.toml");
    let (guests, said) = Guests::open(&path);
    assert_eq!(said, None);
    guests
        .set(&node("nPI"), "pi", GuestAnswer::Allow, NOW)
        .unwrap();
    guests.ask(&node("nOLD"), "old", NOW).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let (again, said) = Guests::open(&path);
    assert_eq!(said, None);
    assert_eq!(again.state_of(&node("nPI")), Some(State::Approved));
    assert!(
        again.asking().is_empty(),
        "a question waiting is not kept across a restart"
    );
}

#[test]
fn a_file_that_is_not_a_list_is_put_aside_and_the_list_starts_empty() {
    let dir = scratch("unusable");
    let path = dir.join("guests.toml");
    std::fs::write(&path, "this is [not a list").unwrap();
    let (guests, said) = Guests::open(&path);
    assert!(said.unwrap().contains("started empty"));
    assert!(guests.rows().is_empty());
    assert!(dir.join("guests.toml.unusable").exists());
    assert!(!path.exists());
}

#[test]
fn a_record_that_names_no_computer_is_dropped() {
    let dir = scratch("badid");
    let path = dir.join("guests.toml");
    std::fs::write(
        &path,
        "[guests.\"not an id!\"]\nname = \"x\"\nstate = \"approved\"\nsince = 1\n\n[guests.nPI]\nname = \"pi\"\nstate = \"approved\"\nsince = 1\n",
    )
    .unwrap();
    let (guests, _) = Guests::open(&path);
    let nodes: Vec<String> = guests.rows().iter().map(|r| r.node.to_string()).collect();
    assert_eq!(nodes, ["nPI"]);
}

#[test]
fn an_answer_that_cannot_be_saved_changes_nothing() {
    let dir = scratch("unsaved");
    // The file's directory is a file, so nothing can be written there.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let (guests, _) = Guests::open(&blocker.join("guests.toml"));
    let told = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&told);
    guests.on_event(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(
        guests.set(&node("nPI"), "pi", GuestAnswer::Allow, NOW),
        Err(GuestError::NotSaved)
    );
    assert_eq!(guests.state_of(&node("nPI")), None);
    assert_eq!(
        told.load(Ordering::SeqCst),
        0,
        "nothing changed, nothing to tell"
    );
    // A question waiting stays waiting when its answer could not be kept.
    guests.ask(&node("nOLD"), "old", NOW).unwrap();
    let before = told.load(Ordering::SeqCst);
    assert_eq!(
        guests.answer(&node("nOLD"), GuestAnswer::Allow, NOW),
        Err(GuestError::NotSaved)
    );
    assert_eq!(guests.asking().len(), 1);
    assert_eq!(told.load(Ordering::SeqCst), before);
}

#[test]
fn the_answers_have_words_that_read_back_and_no_other_word_is_one() {
    for answer in GuestAnswer::ALL {
        assert_eq!(GuestAnswer::from_slug(answer.slug()), Some(answer));
    }
    assert_eq!(GuestAnswer::Allow.slug(), "allow");
    assert_eq!(GuestAnswer::Deny.slug(), "deny");
    for word in ["", "yes", "true", "Allow", " allow", "allow ", "deny\n"] {
        assert_eq!(GuestAnswer::from_slug(word), None, "{word:?}");
    }
}

#[test]
fn every_sentence_is_plain() {
    for error in [
        GuestError::NotAsking,
        GuestError::TooManyAsking,
        GuestError::TooMany,
        GuestError::NotSaved,
    ] {
        let text = error.to_string();
        assert!(text.ends_with('.'), "{text}");
        let lower = text.to_lowercase();
        for word in ["porter", "inferd", "node", "socket", "tailnet"] {
            assert!(!lower.contains(word), "{text}");
        }
    }
}
