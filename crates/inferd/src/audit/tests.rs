use super::*;
use crate::clock::FixedClock;
use crate::testkit::Scratch;
use porter_core::{AccountId, AppName, Isolation, Locality, ModelId, UnixSeconds};
use porter_infer::{ChatReply, EmbedReply, InferRefusal, ModelError, StopReason};

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
        usage,
        served: served(),
    })
}

fn spec() -> SessionSpec {
    SessionSpec {
        need: porter_core::Need::Llm(porter_core::need::LlmNeed {
            features: Default::default(),
            context: Tokens(1),
        }),
        class: porter_core::DataClass::Notes,
        tier: porter_core::Tier::Fast,
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
    audit.record(&spec(), &served(), &chat(usage(12, 5)));
    let entries = memory.entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.at, UnixSeconds(1_700_000_000));
    assert_eq!(entry.app, app());
    assert_eq!(
        (entry.account.as_str(), entry.model.as_str()),
        ("local", "m")
    );
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
    audit.record(&spec(), &served(), &chat(usage(1, 1)));
    audit.record(&spec(), &served(), &chat(usage(2, 2)));
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
        &SessionAudit::new(app(), Discard, FixedClock(UnixSeconds(0)))
            .entry(&served(), &InferReply::Cancelled),
    );
    Discard.append(
        &SessionAudit::new(app(), Discard, FixedClock(UnixSeconds(0)))
            .entry(&served(), &InferReply::Cancelled),
    );
}

#[test]
fn a_shared_destination_is_one_destination() {
    let memory = Arc::new(Memory::default());
    let one = SessionAudit::new(app(), Arc::clone(&memory), FixedClock(UnixSeconds(0)));
    let two = SessionAudit::new(app(), Arc::clone(&memory), FixedClock(UnixSeconds(0)));
    one.record(&spec(), &served(), &InferReply::Cancelled);
    two.record(&spec(), &served(), &InferReply::Cancelled);
    assert_eq!(memory.entries().len(), 2);
}
