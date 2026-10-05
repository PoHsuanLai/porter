use super::*;
use crate::testkit::{Scratch, models};
use porter_core::consent::Usage;
use porter_core::{DataClass, Permille, Tier};
use std::io::Write;

fn chat_model() -> (Scratch, LocalModel) {
    let scratch = Scratch::new("bridge");
    let model = models(&scratch).remove(0);
    (scratch, model)
}

fn control() -> pi::ChatControl {
    pi::ChatControl {
        tool_choice: pi::ToolChoice::Auto,
        tool_calls: pi::ToolParallelism::Many,
        max_output: pi::Knob::Off,
        reasoning: pi::Reasoning::EngineDefault,
        sampling: pi::Knob::Off,
        stop: vec![],
    }
}

fn request(messages: Vec<pi::ChatMessage>) -> pi::ChatRequest {
    pi::ChatRequest {
        messages,
        shape: pi::ReplyShape::Text,
        tier: Tier::Balanced,
        class: DataClass::Notes,
        usage: Usage::Interactive,
        tools: vec![],
        control: control(),
    }
}

fn user(parts: Vec<pi::MessagePart>) -> pi::ChatMessage {
    pi::ChatMessage {
        role: pi::Role::User,
        parts,
    }
}

fn text(text: &str) -> pi::MessagePart {
    pi::MessagePart::Text(text.into())
}

fn memfd(bytes: &[u8]) -> OwnedFd {
    let fd = rustix::fs::memfd_create("t", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd");
    std::fs::File::from(fd.try_clone().expect("dup"))
        .write_all(bytes)
        .expect("fill");
    fd
}

fn tool(name: &str) -> pi::ToolDecl {
    pi::ToolDecl {
        name: pi::ToolName::parse(name).expect("name"),
        description: "does a thing".into(),
        params: pi::JsonSchemaText(pi::JsonText::parse(r#"{"type":"object"}"#).expect("json")),
    }
}

#[test]
fn a_plain_turn_takes_the_models_name_defaults_and_flavor_extras() {
    let (_scratch, model) = chat_model();
    let turn = chat_turn(
        &model,
        &request(vec![user(vec![text("hi")])]),
        &Frames::default(),
    )
    .expect("turn");
    assert_eq!(turn.model.0, "tiny-chat");
    assert_eq!(
        turn.messages,
        vec![sp::Message {
            role: sp::Role::User,
            parts: vec![sp::Part::Text("hi".into())]
        }]
    );
    // Reasoning left open stays the engine's own; the entry's `reasoning_default` (off) picks the
    // greedy sampling.
    assert_eq!(turn.reasoning, sp::Reasoning::EngineDefault);
    assert_eq!(turn.sampling.temperature, sp::Milli(0));
    // The output limit is the entry's.
    assert_eq!(turn.limits.max_output, sp::Tokens(1024));
    assert_eq!(turn.output, sp::OutputShape::Free);
    assert_eq!(turn.tool_choice, sp::ToolChoice::Auto);
    assert_eq!(turn.tool_calls, sp::ToolParallelism::Many);
    assert_eq!(
        turn.engine,
        sp::EngineExtras::LlamaServer(sp::LlamaExtras {
            cache_prompt: sp::PromptCache::Reuse,
            slot: sp::Knob::Off
        })
    );
}

#[test]
fn reasoning_picks_the_catalog_sampling_set_and_an_explicit_sampling_wins() {
    let (_scratch, model) = chat_model();
    let mut on = request(vec![user(vec![text("hi")])]);
    on.control.reasoning = pi::Reasoning::On(pi::Effort::High);
    let turn = chat_turn(&model, &on, &Frames::default()).expect("turn");
    assert_eq!(turn.reasoning, sp::Reasoning::On(sp::Effort::High));
    assert_eq!(turn.sampling.temperature, sp::Milli(600));

    let mut own = request(vec![user(vec![text("hi")])]);
    own.control.sampling = pi::Knob::Set(pi::Sampling {
        temperature: Permille(300),
        top_p: pi::Knob::Set(Permille(900)),
        top_k: pi::Knob::Set(porter_core::Count(40)),
        min_p: pi::Knob::Off,
        seed: pi::Knob::Set(pi::Seed(7)),
    });
    own.control.max_output = pi::Knob::Set(porter_core::Tokens(1));
    own.control.stop = vec!["END".into()];
    let turn = chat_turn(&model, &own, &Frames::default()).expect("turn");
    assert_eq!(
        turn.sampling,
        sp::Sampling {
            temperature: sp::Milli(300),
            top_p: sp::Knob::Set(sp::Milli(900)),
            top_k: sp::Knob::Set(sp::Count(40)),
            min_p: sp::Knob::Off,
            repeat_penalty: sp::Knob::Off,
            seed: sp::Knob::Set(sp::Seed(7)),
        }
    );
    assert_eq!(
        turn.limits,
        sp::Limits {
            max_output: sp::Tokens(1),
            stop: vec!["END".into()]
        }
    );
}

#[test]
fn a_temperature_past_what_a_milli_holds_saturates() {
    assert_eq!(milli(Permille(70_000)), sp::Milli(u16::MAX));
    assert_eq!(milli(Permille(700)), sp::Milli(700));
}

#[test]
fn tools_shapes_and_tool_choices_map_one_to_one() {
    let (_scratch, model) = chat_model();
    let cases = [
        (pi::ToolChoice::Never, sp::ToolChoice::Never),
        (pi::ToolChoice::Required, sp::ToolChoice::Required),
        (
            pi::ToolChoice::Named(pi::ToolName::parse("mail.send").expect("name")),
            sp::ToolChoice::Named(sp::ToolName::new("mail.send").expect("name")),
        ),
    ];
    for (given, expected) in cases {
        let mut chat = request(vec![user(vec![text("go")])]);
        chat.tools = vec![tool("mail.send")];
        chat.control.tool_choice = given;
        let turn = chat_turn(&model, &chat, &Frames::default()).expect("turn");
        assert_eq!(turn.tool_choice, expected);
        assert!(
            matches!(&turn.tools[..], [sp::ToolSpec::Function { name, .. }] if name.as_str() == "mail.send")
        );
    }
    let shapes = [
        (pi::ReplyShape::Text, sp::OutputShape::Free),
        (
            pi::ReplyShape::Choice(vec!["yes".into(), "no".into()]),
            sp::OutputShape::Choice(vec!["yes".into(), "no".into()]),
        ),
    ];
    for (given, expected) in shapes {
        let mut chat = request(vec![user(vec![text("go")])]);
        chat.shape = given;
        assert_eq!(
            chat_turn(&model, &chat, &Frames::default())
                .expect("turn")
                .output,
            expected
        );
    }
    let mut json = request(vec![user(vec![text("go")])]);
    json.shape = pi::ReplyShape::Json(r#"{"type":"object"}"#.into());
    assert!(matches!(
        chat_turn(&model, &json, &Frames::default())
            .expect("turn")
            .output,
        sp::OutputShape::JsonSchema(_)
    ));
    json.shape = pi::ReplyShape::Json("{not json".into());
    assert_eq!(
        chat_turn(&model, &json, &Frames::default()),
        Err(BridgeError::Unsupported)
    );
}

#[test]
fn history_with_calls_results_thoughts_and_roles_maps_part_by_part() {
    let (_scratch, model) = chat_model();
    let call = pi::ToolCallPart {
        id: pi::ToolCallId("c1".into()),
        name: pi::ToolName::parse("lookup").expect("name"),
        args: pi::JsonText::parse(r#"{"q":"x"}"#).expect("json"),
    };
    let messages = vec![
        pi::ChatMessage {
            role: pi::Role::System,
            parts: vec![text("be brief")],
        },
        user(vec![text("find x")]),
        pi::ChatMessage {
            role: pi::Role::Assistant,
            parts: vec![
                pi::MessagePart::Thought(pi::ThoughtPart {
                    text: "hmm".into(),
                    seal: pi::ThoughtSeal::Signed(pi::SignatureText("sig".into())),
                }),
                pi::MessagePart::ToolCall(call),
            ],
        },
        user(vec![pi::MessagePart::ToolResult(pi::ToolResultPart {
            id: pi::ToolCallId("c1".into()),
            status: pi::ToolStatus::Error,
            parts: vec![text("not found")],
        })]),
    ];
    let turn = chat_turn(&model, &request(messages), &Frames::default()).expect("turn");
    let roles: Vec<sp::Role> = turn.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            sp::Role::System,
            sp::Role::User,
            sp::Role::Assistant,
            sp::Role::User
        ]
    );
    assert!(matches!(
        &turn.messages[2].parts[..],
        [
            sp::Part::Thought { seal: sp::ThoughtSeal::Signed(_), .. },
            sp::Part::ToolCall(sp::ToolCall { id, .. })
        ] if id.0 == "c1"
    ));
    assert!(matches!(
        &turn.messages[3].parts[..],
        [sp::Part::ToolResult(sp::ToolResult { status: sp::ToolStatus::Error, parts, .. })] if parts.len() == 1
    ));
}

#[test]
fn images_come_inline_or_from_the_descriptors_by_index() {
    let (_scratch, model) = chat_model();
    let frames = Frames::read(vec![memfd(b"first"), memfd(b"second")]).expect("frames");
    let image = |source| {
        user(vec![pi::MessagePart::Image(pi::ImagePart {
            media_type: "image/png".into(),
            source,
        })])
    };
    let attached = chat_turn(
        &model,
        &request(vec![image(pi::ImageSource::Attached(pi::AttachIndex(1)))]),
        &frames,
    )
    .expect("turn");
    let sp::Part::Image(got) = &attached.messages[0].parts[0] else {
        panic!("image");
    };
    assert_eq!(
        (got.media, got.bytes.0.as_slice()),
        (vision_prep::MediaType::Png, &b"second"[..])
    );

    let inline = chat_turn(
        &model,
        &request(vec![image(pi::ImageSource::Inline(pi::Base64Bytes(
            b"inline".to_vec(),
        )))]),
        &Frames::default(),
    )
    .expect("turn");
    assert!(matches!(&inline.messages[0].parts[0], sp::Part::Image(i) if i.bytes.0 == b"inline"));

    // An index nobody attached, and a type with no encoder.
    assert_eq!(
        chat_turn(
            &model,
            &request(vec![image(pi::ImageSource::Attached(pi::AttachIndex(2)))]),
            &frames
        ),
        Err(BridgeError::Attachment)
    );
    let gif = user(vec![pi::MessagePart::Image(pi::ImagePart {
        media_type: "image/gif".into(),
        source: pi::ImageSource::Attached(pi::AttachIndex(0)),
    })]);
    assert_eq!(
        chat_turn(&model, &request(vec![gif]), &frames),
        Err(BridgeError::Unsupported)
    );
}

#[test]
fn a_descriptor_is_read_from_its_start_whatever_offset_the_sender_left() {
    // `write_all` leaves the offset at the end, which is where a received descriptor shares it.
    let frames = Frames::read(vec![memfd(b"whole picture")]).expect("frames");
    assert_eq!(
        frames
            .resolve(&pi::ImageSource::Attached(pi::AttachIndex(0)))
            .expect("bytes"),
        b"whole picture"
    );
    assert_eq!(Frames::read(vec![]).expect("none"), Frames::default());
}

#[test]
fn a_task_is_its_instruction_then_the_text_with_no_tools() {
    let (_scratch, model) = chat_model();
    for task in [
        pi::Task::Summarise,
        pi::Task::Rewrite,
        pi::Task::Extract,
        pi::Task::Classify,
    ] {
        let turn = task_turn(
            &model,
            &pi::TaskRequest {
                task,
                input: "the text".into(),
                class: DataClass::Notes,
                usage: Usage::Background,
            },
            Tier::Fast,
        )
        .expect("turn");
        assert_eq!(turn.messages.len(), 2);
        assert_eq!(turn.messages[0].role, sp::Role::System);
        assert!(matches!(&turn.messages[0].parts[0], sp::Part::Text(t) if !t.is_empty()));
        assert_eq!(
            turn.messages[1].parts,
            vec![sp::Part::Text("the text".into())]
        );
        assert!(turn.tools.is_empty());
        assert_eq!(turn.tool_choice, sp::ToolChoice::Never);
        assert_eq!(turn.reasoning, sp::Reasoning::Off);
    }
}

fn embed_model() -> (Scratch, LocalModel) {
    let scratch = Scratch::new("bridge-embed");
    let model = models(&scratch).remove(1);
    (scratch, model)
}

fn embed(inputs: &[&str], role: pi::EmbedRole, dims: DimsNeed) -> pi::EmbedRequest {
    pi::EmbedRequest {
        inputs: inputs.iter().map(|s| (*s).into()).collect(),
        role,
        dims,
        class: DataClass::Notes,
        usage: Usage::Background,
    }
}

#[test]
fn embedding_texts_carry_the_prefix_of_their_role_and_are_cut_into_batches() {
    let (_scratch, model) = embed_model();
    let turns = embed_turns(
        &model,
        &embed(
            &["a", "b", "c"],
            pi::EmbedRole::Document,
            DimsNeed::Exactly(porter_core::Dims(4)),
        ),
    )
    .expect("turns");
    let inputs: Vec<Vec<&str>> = turns
        .iter()
        .map(|turn| turn.inputs.iter().map(String::as_str).collect())
        .collect();
    // The model's batch limit is two.
    assert_eq!(
        inputs,
        vec![
            vec!["search_document: a", "search_document: b"],
            vec!["search_document: c"]
        ]
    );
    assert!(
        turns
            .iter()
            .all(|t| t.role == sp::EmbedRole::Document && t.model.0 == "tiny-embed")
    );
    assert!(turns.iter().all(|t| t.dims == sp::Knob::Set(sp::Dims(4))));

    let query =
        embed_turns(&model, &embed(&["q"], pi::EmbedRole::Query, DimsNeed::Any)).expect("turns");
    assert_eq!(query[0].inputs, vec!["search_query: q"]);
    assert_eq!(query[0].dims, sp::Knob::Off);
    assert_eq!(
        embed_turns(&model, &embed(&[], pi::EmbedRole::Query, DimsNeed::Any)).expect("none"),
        vec![]
    );
}

#[test]
fn a_model_without_embedding_details_cannot_embed() {
    let scratch = Scratch::new("bridge-noembed");
    let chat = models(&scratch).remove(0);
    assert_eq!(
        embed_turns(&chat, &embed(&["a"], pi::EmbedRole::Query, DimsNeed::Any)),
        Err(BridgeError::Unsupported)
    );
}
