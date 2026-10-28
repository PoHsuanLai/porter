//! The fake model echoes the last text.

use porter_capability::ModelId;
use porter_core::AccountId;
use porter_core::consent::Usage;
use porter_core::{DataClass, Tier};
use porter_fake::llm_account;
use porter_infer::testing::FakeModel;
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ChatSink, Flow, InferEvent, MessagePart, Model, Role,
    ToolParallelism,
};

#[tokio::test]
async fn the_fake_model_echoes_the_last_text() {
    let model = FakeModel::on_device(
        llm_account().id,
        ModelId::parse("echo").expect("id"),
        vec![],
    );
    let request = ChatRequest::new(
        vec![
            ChatMessage {
                role: Role::System,
                parts: vec![MessagePart::Text("be brief".into())],
            },
            ChatMessage {
                role: Role::User,
                parts: vec![MessagePart::Text("hello".into())],
            },
        ],
        Tier::Fast,
        DataClass::Public,
        Usage::Interactive,
    )
    .with_control(ChatControl::new().with_tool_calls(ToolParallelism::Many));
    let mut events = Collect::default();
    let reply = model.chat(&request, &mut events).await.expect("chat");
    assert_eq!(reply.text, "hello");
    assert_eq!(events.0[0], InferEvent::TextDelta("hello".into()));
    assert_eq!(
        reply.served.account,
        AccountId::parse("fake-llm").expect("id")
    );
}

#[derive(Default)]
struct Collect(Vec<InferEvent>);

impl ChatSink for Collect {
    fn event(&mut self, event: InferEvent) -> Flow {
        self.0.push(event);
        Flow::Continue
    }
}
