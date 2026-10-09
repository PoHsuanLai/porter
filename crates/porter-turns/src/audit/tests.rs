use super::*;
use porter_core::clock::FixedClock;
use porter_core::{AccountId, AppName, Count, Isolation, Locality, ModelId, UnixSeconds};
use porter_infer::{ChatReply, EmbedReply, InferRefusal, ModelError, StopReason, Why};
use porter_router::testkit::Scratch;

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Test").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("m").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn usage(input: u32, output: u32) -> TokenUsage {
    TokenUsage {
        input: Tokens(input),
        output: Tokens(output),
        cached: Tokens(1),
    }
}

fn chat(usage: TokenUsage) -> InferReply {
    InferReply::Chat(ChatReply {
        text: "secret words".into(),
        tool_calls: vec![],
        stop: StopReason::EndTurn,
        thought: None,
        scores: None,
        usage,
        served: served(),
    })
}

fn nothing() -> Carried {
    Carried {
        images: Count(0),
        audio_ms: Count(0),
    }
}

fn spec() -> SessionSpec {
    SessionSpec {
        need: porter_core::Need::Llm(porter_core::need::LlmNeed {
            features: Default::default(),
            context: Tokens(1),
        }),
        class: porter_core::DataClass::Notes,
        tier: porter_core::Tier::Fast,
        usage: porter_core::consent::Usage::Interactive,
    }
}

#[test]
fn a_finished_turn_is_one_entry_with_who_what_and_how_much_and_no_content() {
    let memory = Memory::default();
    let audit = SessionAudit::new(
        app(),
        memory.clone(),
        FixedClock(UnixSeconds(1_700_000_000)),
    );
    audit.record_why(
        &spec(),
        &served(),
        &chat(usage(12, 5)),
        &nothing(),
        &Why::Named,
    );
    let entries = memory.entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.at, UnixSeconds(1_700_000_000));
    assert_eq!(entry.app, app());
    assert_eq!(
        (entry.account.as_str(), entry.model.as_str()),
        ("local", "m")
    );
    assert_eq!(entry.class, porter_core::DataClass::Notes);
    assert_eq!(entry.locality, Locality::OnDevice);
    assert_eq!(entry.usage, usage(12, 5));
    assert_eq!(entry.bytes_out, Bytes(0));
    let text = serde_json::to_string(entry).expect("json");
    assert!(
        !text.contains("secret words"),
        "the entry holds no content: {text}"
    );
}

#[test]
fn what_a_reply_spent_depends_on_what_it_is() {
    let zero = usage_of(&InferReply::Cancelled);
    assert_eq!(
        (zero.input, zero.output, zero.cached),
        (Tokens(0), Tokens(0), Tokens(0))
    );
    let embed = InferReply::Embed(EmbedReply {
        vectors: vec![],
        usage: usage(9, 0),
        served: served(),
    });
    let cases = [
        (chat(usage(3, 4)), usage(3, 4)),
        (embed, usage(9, 0)),
        (InferReply::Refused(InferRefusal::Denied), zero),
        (InferReply::Failed(ModelError::NotReady), zero),
        (InferReply::Cancelled, zero),
    ];
    for (reply, expected) in cases {
        assert_eq!(usage_of(&reply), expected, "{reply:?}");
    }
}

#[test]
fn lines_are_appended_as_json_and_read_back_unchanged() {
    let dir = Scratch::new("audit");
    let path = dir.path().join("state").join("audit.jsonl");
    let lines = JsonLines::new(path.clone());
    let audit = SessionAudit::new(app(), lines, FixedClock(UnixSeconds(5)));
    audit.record_why(
        &spec(),
        &served(),
        &chat(usage(1, 1)),
        &nothing(),
        &Why::Named,
    );
    audit.record_why(
        &spec(),
        &served(),
        &chat(usage(2, 2)),
        &nothing(),
        &Why::Named,
    );
    let text = std::fs::read_to_string(&path).expect("the file and its directory were created");
    let entries: Vec<AuditEntry> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("a line of AuditEntry"))
        .collect();
    assert_eq!(
        entries.iter().map(|e| e.usage.input).collect::<Vec<_>>(),
        [Tokens(1), Tokens(2)]
    );
}

#[test]
fn an_unwritable_destination_loses_the_entry_and_never_the_turn() {
    // The "directory" is a file, so nothing can be created under it.
    let dir = Scratch::new("audit-bad");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").expect("file");
    JsonLines::new(blocker.join("audit.jsonl")).append(
        &SessionAudit::new(app(), Discard, FixedClock(UnixSeconds(0))).entry(
            &spec(),
            &served(),
            &InferReply::Cancelled,
            &nothing(),
            Some(&Why::Named),
        ),
    );
    Discard.append(
        &SessionAudit::new(app(), Discard, FixedClock(UnixSeconds(0))).entry(
            &spec(),
            &served(),
            &InferReply::Cancelled,
            &nothing(),
            Some(&Why::Named),
        ),
    );
}

#[test]
fn a_shared_destination_is_one_destination() {
    let memory = Arc::new(Memory::default());
    let one = SessionAudit::new(app(), Arc::clone(&memory), FixedClock(UnixSeconds(0)));
    let two = SessionAudit::new(app(), Arc::clone(&memory), FixedClock(UnixSeconds(0)));
    one.record_why(
        &spec(),
        &served(),
        &InferReply::Cancelled,
        &nothing(),
        &Why::Named,
    );
    two.record_why(
        &spec(),
        &served(),
        &InferReply::Cancelled,
        &nothing(),
        &Why::Named,
    );
    assert_eq!(memory.entries().len(), 2);
}

#[test]
fn what_the_request_carried_is_in_the_entry_as_counts_and_nothing_else() {
    let memory = Memory::default();
    let audit = SessionAudit::new(app(), memory.clone(), FixedClock(UnixSeconds(1)));
    let carried = Carried {
        images: Count(3),
        audio_ms: Count(1_250),
    };
    audit.record_why(
        &spec(),
        &served(),
        &chat(usage(1, 1)),
        &carried,
        &Why::Named,
    );
    let entry = &memory.entries()[0];
    assert_eq!((entry.images, entry.audio_ms), (Count(3), Count(1_250)));
    let text = serde_json::to_string(entry).expect("json");
    assert!(text.contains(r#""images":3"#) && text.contains(r#""audio_ms":1250"#));
}

#[test]
fn the_json_shape_is_pinned_and_carries_the_class_never_content() {
    let memory = Memory::default();
    let audit = SessionAudit::new(app(), memory.clone(), FixedClock(UnixSeconds(7)));
    audit.record_why(
        &spec(),
        &served(),
        &chat(usage(12, 5)),
        &nothing(),
        &Why::Named,
    );
    let value = serde_json::to_value(&memory.entries()[0]).expect("json");
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "account",
            "app",
            "at",
            "audio_ms",
            "bytes_out",
            "class",
            "images",
            "locality",
            "model",
            "usage",
            "why"
        ]
    );
    assert_eq!(value["class"], "notes");
}

#[test]
fn the_entry_records_why_the_route_chose_the_model_and_an_old_line_still_reads() {
    let memory = Memory::default();
    let audit = SessionAudit::new(app(), memory.clone(), FixedClock(UnixSeconds(0)));
    let why = Why::Evicted {
        model: porter_infer::ModelRef {
            account: AccountId::parse("local").expect("id"),
            model: ModelId::parse("old").expect("id"),
        },
    };
    audit.record_why(&spec(), &served(), &chat(usage(1, 1)), &nothing(), &why);
    let entry = memory.entries().remove(0);
    assert_eq!(entry.why, Some(why));
    // A line written before the router said why has no `why` key.
    let mut json = serde_json::to_value(&entry).expect("json");
    json.as_object_mut().expect("object").remove("why");
    let old: AuditEntry = serde_json::from_value(json).expect("reads");
    assert_eq!(old.why, None);
}

#[test]
fn a_sink_asked_without_a_reason_records_none() {
    let memory = Memory::default();
    let audit = SessionAudit::new(app(), memory.clone(), FixedClock(UnixSeconds(0)));
    audit.record(&spec(), &served(), &chat(usage(1, 1)), &nothing());
    assert_eq!(memory.entries()[0].why, None);
}
