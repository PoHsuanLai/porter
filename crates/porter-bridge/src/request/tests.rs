use super::*;
use porter_core::consent::Usage;
use porter_core::{DataClass, Permille, Tier};

fn target(sampling: DefaultSampling) -> Target {
    Target {
        name: sp::ModelName("llama3.2:3b".into()),
        sampling,
        max_output: sp::Tokens(512),
        flavor: Some(Flavor::LlamaServer),
    }
}

fn chat(parts: Vec<pi::MessagePart>) -> pi::ChatRequest {
    pi::ChatRequest {
        messages: vec![pi::ChatMessage {
            role: pi::Role::User,
            parts,
        }],
        shape: pi::ReplyShape::Text,
        tier: Tier::Balanced,
        class: DataClass::Notes,
        usage: Usage::Interactive,
        tools: vec![],
        control: pi::ChatControl {
            tool_choice: pi::ToolChoice::Auto,
            tool_calls: pi::ToolParallelism::Many,
            max_output: pi::Knob::Off,
            reasoning: pi::Reasoning::EngineDefault,
            sampling: pi::Knob::Off,
            stop: vec![],
            scores: pi::Knob::Off,
        },
    }
}

#[test]
fn a_temperature_past_what_a_milli_holds_saturates() {
    assert_eq!(milli(Permille(70_000)), sp::Milli(u16::MAX));
    assert_eq!(milli(Permille(700)), sp::Milli(700));
}

#[test]
fn a_target_names_the_model_the_limit_and_the_dialect_extras() {
    let turn = chat_turn_for(
        &target(DefaultSampling::Provider),
        &chat(vec![pi::MessagePart::Text("hi".into())]),
        &Frames::default(),
    )
    .expect("turn");
    assert_eq!(turn.model, sp::ModelName("llama3.2:3b".into()));
    assert_eq!(turn.limits.max_output, sp::Tokens(512));
    assert_eq!(turn.sampling, PROVIDER_DEFAULT);
    assert_eq!(
        turn.engine,
        sp::EngineExtras::LlamaServer(sp::LlamaExtras {
            cache_prompt: sp::PromptCache::Reuse,
            slot: sp::Knob::Off
        })
    );
}

#[test]
fn an_entry_that_writes_no_sampling_cannot_express_an_open_one() {
    assert_eq!(
        chat_turn_for(
            &target(DefaultSampling::Entry(None)),
            &chat(vec![pi::MessagePart::Text("hi".into())]),
            &Frames::default(),
        ),
        Err(BridgeError::Unsupported)
    );
}

#[test]
fn an_attached_image_with_no_descriptors_is_an_attachment_error() {
    let image = pi::MessagePart::Image(pi::ImagePart {
        media_type: "image/png".into(),
        source: pi::ImageSource::Attached(pi::AttachIndex(0)),
    });
    assert_eq!(
        chat_turn_for(
            &target(DefaultSampling::Provider),
            &chat(vec![image]),
            &Frames::default()
        ),
        Err(BridgeError::Attachment)
    );
}

#[test]
fn an_inline_image_of_a_type_with_no_encoder_is_unsupported() {
    let image = pi::MessagePart::Image(pi::ImagePart {
        media_type: "image/gif".into(),
        source: pi::ImageSource::Inline(pi::Base64Bytes(vec![1])),
    });
    assert_eq!(
        chat_turn_for(
            &target(DefaultSampling::Provider),
            &chat(vec![image]),
            &Frames::default()
        ),
        Err(BridgeError::Unsupported)
    );
}

#[test]
fn a_task_is_its_instruction_then_the_text() {
    let task = pi::TaskRequest {
        task: pi::Task::Summarise,
        input: "the long text".into(),
        class: DataClass::Notes,
        usage: Usage::Interactive,
    };
    let turn = task_turn_for(&target(DefaultSampling::Provider), &task, Tier::Fast).expect("turn");
    assert_eq!(turn.messages.len(), 2);
    assert_eq!(turn.messages[0].role, sp::Role::System);
    assert_eq!(
        turn.messages[1].parts,
        vec![sp::Part::Text("the long text".into())]
    );
    assert_eq!(turn.tool_choice, sp::ToolChoice::Never);
}
