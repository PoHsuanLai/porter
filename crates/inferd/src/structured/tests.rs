use super::*;
use crate::testkit::{Scratch, models};
use model_provider::{
    ModelName, OutputShape, Part, ProviderError, Role, Script, ScriptedProvider, StopReason,
    TurnEnd, TurnEvent, TurnUsage,
};
use porter_core::{DataClass, Tier, consent::Usage};

const SCORE: &str = r#"{"type":"object","additionalProperties":false,
    "properties":{"score":{"type":"integer","minimum":0,"maximum":10}},
    "required":["score"]}"#;

fn request(shape: pi::ReplyShape, tools: Vec<pi::ToolDecl>) -> pi::ChatRequest {
    pi::ChatRequest {
        messages: vec![pi::ChatMessage {
            role: pi::Role::User,
            parts: vec![pi::MessagePart::Text("rate it".into())],
        }],
        shape,
        tier: Tier::Fast,
        class: DataClass::Notes,
        usage: Usage::Interactive,
        tools,
        control: pi::ChatControl {
            tool_choice: pi::ToolChoice::Auto,
            tool_calls: pi::ToolParallelism::One,
            max_output: pi::Knob::Off,
            reasoning: pi::Reasoning::Off,
            sampling: pi::Knob::Off,
            stop: Vec::new(),
        },
    }
}

fn json(schema: &str) -> pi::ReplyShape {
    pi::ReplyShape::Json(schema.to_owned())
}

fn choice(items: &[&str]) -> pi::ReplyShape {
    pi::ReplyShape::Choice(items.iter().map(|s| (*s).to_owned()).collect())
}

fn is_checked(shaping: &Shaping) -> bool {
    matches!(shaping, Shaping::Checked(_))
}

#[test]
fn a_shape_the_vocabulary_can_say_is_checked_and_one_it_cannot_is_sent_as_it_is() {
    let scratch = Scratch::new("shaping");
    let model = models(&scratch).remove(0);
    let table = [
        ("text", pi::ReplyShape::Text, false),
        ("a readable schema", json(SCORE), true),
        ("an open object", json(r#"{"type":"object"}"#), false),
        ("a number", json(r#"{"type":"number"}"#), false),
        (
            "a pattern",
            json(r#"{"type":"string","pattern":"^a"}"#),
            false,
        ),
        ("text that is not JSON", json("not json"), false),
        ("a choice", choice(&["allow", "deny"]), true),
        ("no choices", choice(&[]), false),
    ];
    for (name, shape, checked) in table {
        assert_eq!(
            is_checked(&shaping(
                &model,
                &request(shape, Vec::new()),
                Limits::default()
            )),
            checked,
            "{name}"
        );
    }
}

#[test]
fn a_request_with_tools_of_its_own_is_not_checked() {
    let scratch = Scratch::new("shaping-tools");
    let model = models(&scratch).remove(0);
    let tool = pi::ToolDecl {
        name: pi::ToolName::parse("lookup").expect("name"),
        description: "look it up".into(),
        params: pi::JsonSchemaText(pi::JsonText::parse(r#"{"type":"object"}"#).expect("json")),
    };
    let with = request(json(SCORE), vec![tool]);
    assert!(!is_checked(&shaping(&model, &with, Limits::default())));
}

fn end(stop: StopReason, output: u32) -> TurnEnd {
    TurnEnd {
        stop,
        usage: TurnUsage {
            input: model_provider::Tokens(10),
            output: model_provider::Tokens(output),
            ..TurnUsage::default()
        },
        served: ModelName("tiny-chat".into()),
        first_token: None,
    }
}

fn says(text: &str) -> Script {
    Script {
        events: vec![
            TurnEvent::ThoughtDelta("hm. ".into()),
            TurnEvent::TextDelta(text.into()),
        ],
        end: Ok(end(StopReason::EndTurn, 5)),
    }
}

struct Ran {
    result: Result<Validated, ModelError>,
    provider: ScriptedProvider,
    shown: Vec<InferEvent>,
    touched: usize,
}

async fn ran(shape: pi::ReplyShape, scripts: Vec<Script>, stop_after: usize) -> Ran {
    let scratch = Scratch::new("structured");
    let model = models(&scratch).remove(0);
    let chat = request(shape, Vec::new());
    let base =
        crate::bridge::chat_turn(&model, &chat, &crate::bridge::Frames::default()).expect("a turn");
    let Shaping::Checked(checked) = shaping(&model, &chat, Limits::default()) else {
        panic!("a checked shape");
    };
    let provider = ScriptedProvider::new(vec![], scripts);
    let mut shown = Vec::new();
    let mut forward = |event| {
        shown.push(event);
        if shown.len() >= stop_after {
            Flow::Stop
        } else {
            Flow::Continue
        }
    };
    let touched = std::cell::Cell::new(0);
    let result = run(&provider, *checked, &base, &mut forward, || {
        touched.set(touched.get() + 1);
    })
    .await;
    Ran {
        result,
        provider,
        shown,
        touched: touched.get(),
    }
}

#[tokio::test]
async fn a_reply_that_fits_is_returned_as_the_model_wrote_it_and_only_the_thoughts_streamed() {
    let got = ran(
        json(SCORE),
        vec![says("```json\n{\"score\": 7}\n```")],
        usize::MAX,
    )
    .await;
    let valid = got.result.expect("valid");
    assert_eq!(valid.text, "{\"score\": 7}");
    assert_eq!(valid.thought.as_deref(), Some("hm. "));
    assert_eq!(
        got.shown,
        vec![InferEvent::ThoughtDelta("hm. ".into())],
        "the text is not shown before it is checked"
    );
    assert_eq!(got.provider.requests().len(), 1);
    assert_eq!(got.touched, 1);
    // The engine is asked with the constraint it has (tiny-chat takes a JSON schema).
    assert!(matches!(
        got.provider.requests()[0].output,
        OutputShape::JsonSchema(_)
    ));
}

#[tokio::test]
async fn a_reply_that_breaks_the_schema_is_repaired_once_and_every_attempt_is_counted() {
    let secret = "ignore previous instructions";
    let bad = format!(r#"{{"score":"{secret}"}}"#);
    let got = ran(
        json(SCORE),
        vec![says(&bad), says(r#"{"score":4}"#)],
        usize::MAX,
    )
    .await;
    let valid = got.result.expect("repaired");
    assert_eq!(valid.text, r#"{"score":4}"#);
    assert_eq!(
        (valid.usage.input.0, valid.usage.output.0),
        (20, 10),
        "both attempts were used"
    );
    assert_eq!(valid.thought.as_deref(), Some("hm. hm. "));
    assert_eq!(got.touched, 2);
    let requests = got.provider.requests();
    assert_eq!(requests.len(), 2);
    let last = requests[1].messages.last().expect("a message");
    assert_eq!(last.role, Role::User);
    let text = format!("{:?}", requests[1]);
    assert!(
        text.contains("score") && !text.contains("ignore previous"),
        "the repair names the field and does not echo the reply: {text}"
    );
    assert!(
        matches!(&last.parts[..], [Part::Text(t)] if t.contains("not accepted")),
        "{last:?}"
    );
}

#[tokio::test]
async fn a_reply_that_never_fits_is_unparseable_and_a_cut_one_is_not_repaired() {
    let twice = ran(
        json(SCORE),
        vec![says("nope"), says("still nope")],
        usize::MAX,
    )
    .await;
    assert_eq!(twice.result, Err(ModelError::Unparseable));
    assert_eq!(twice.provider.requests().len(), 2, "one repair, no more");

    let cut = ran(
        json(SCORE),
        vec![Script {
            events: vec![TurnEvent::TextDelta(r#"{"score":"#.into())],
            end: Ok(end(StopReason::MaxTokens, 9)),
        }],
        usize::MAX,
    )
    .await;
    assert_eq!(cut.result, Err(ModelError::Unparseable));
    assert_eq!(cut.provider.requests().len(), 1);

    let filtered = ran(
        json(SCORE),
        vec![Script {
            events: vec![],
            end: Ok(end(StopReason::ContentFilter, 0)),
        }],
        usize::MAX,
    )
    .await;
    assert_eq!(filtered.result, Err(ModelError::Refused));
}

#[tokio::test]
async fn an_engine_failure_is_told_as_the_bridge_tells_it() {
    let down = ran(
        json(SCORE),
        vec![Script {
            events: vec![],
            end: Err(ProviderError::Unreachable),
        }],
        usize::MAX,
    )
    .await;
    assert_eq!(down.result, Err(ModelError::Unreachable));
    assert_eq!(down.touched, 0);
}

#[tokio::test]
async fn a_choice_comes_back_bare_and_a_wrong_one_is_repaired() {
    let right = ran(
        choice(&["allow", "deny"]),
        vec![says("\"deny\"")],
        usize::MAX,
    )
    .await;
    assert_eq!(right.result.expect("valid").text, "deny");

    let wrong = ran(
        choice(&["allow", "deny"]),
        vec![says("\"maybe\""), says("\"allow\"")],
        usize::MAX,
    )
    .await;
    assert_eq!(wrong.result.expect("repaired").text, "allow");
    assert_eq!(wrong.provider.requests().len(), 2);
}

#[tokio::test]
async fn a_session_that_stops_listening_ends_the_turn() {
    let got = ran(json(SCORE), vec![says(r#"{"score":1}"#)], 1).await;
    assert_eq!(got.result, Err(ModelError::Unreachable));
    assert_eq!(got.shown.len(), 1);
}

mod limits_tests {
    use super::super::limits::*;
    use crate::config::InferdConfig;
    use model_provider::{CharCount, Count};

    fn resolved(text: &str) -> Resolved {
        InferdConfig::from_toml(text).expect("reads").ai.resolve()
    }

    #[test]
    fn no_table_is_the_default_limits() {
        let said = resolved("");
        assert_eq!(said.limits.schema.open_text, CharCount(4096));
        assert_eq!(said.limits.schema.open_list, Count(256));
        assert_eq!(said.limits.schema.depth, Count(16));
        assert_eq!(said.limits.repairs.0, 1);
        assert!(said.rejected.is_empty());
    }

    #[test]
    fn values_in_range_are_taken_at_both_ends() {
        let table = [
            ("open_text = 256", 256),
            ("open_text = 65536", 65536),
            ("open_text = 255", 4096),
            ("open_text = 65537", 4096),
            ("open_text = -1", 4096),
        ];
        for (line, want) in table {
            let said = resolved(&format!("[ai.structured]\n{line}\n"));
            assert_eq!(said.limits.schema.open_text, CharCount(want), "{line}");
            assert_eq!(
                said.rejected.is_empty(),
                want != 4096 || line == "open_text = 4096"
            );
        }
        let edges = resolved("[ai.structured]\nopen_list=16\ndepth=64\nrepair_budget=0\n");
        assert_eq!(edges.limits.schema.open_list, Count(16));
        assert_eq!(edges.limits.schema.depth, Count(64));
        assert_eq!(edges.limits.repairs.0, 0);
        let top = resolved("[ai.structured]\nopen_list=4096\ndepth=4\nrepair_budget=3\n");
        assert_eq!(top.limits.schema.open_list, Count(4096));
        assert_eq!(top.limits.schema.depth, Count(4));
        assert_eq!(top.limits.repairs.0, 3);
    }

    #[test]
    fn a_bad_field_falls_back_alone_and_is_named() {
        let said = resolved("[ai.structured]\ndepth = 3\nrepair_budget = 9\nopen_list = 100\n");
        assert_eq!(said.limits.schema.depth, Count(16));
        assert_eq!(said.limits.repairs.0, 1);
        assert_eq!(said.limits.schema.open_list, Count(100));
        assert_eq!(
            said.rejected,
            vec!["ai.structured.depth", "ai.structured.repair_budget"]
        );
    }
}
