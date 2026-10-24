//! The fake model echoes the last text, and the unbuilt broker refuses instead of panicking.

use porter_core::consent::Usage;
use porter_core::{AccountId, DataClass, ModelId, Tier};
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

/// The broker body is not built: a request is refused (nothing panics) and no event is sent.
#[tokio::test]
async fn the_unbuilt_broker_refuses_instead_of_panicking() {
    use porter_core::{AppId, AppName, Isolation};
    use porter_infer::{Broker, InferRefusal, InferReply, InferRequest, Policy};
    let broker: Broker<FakeModel> = Broker::new(Policy::proposed(), Vec::new(), Vec::new());
    let app = AppId {
        name: AppName::parse("org.quire.Mail").expect("name"),
        isolation: Isolation::Unsandboxed,
    };
    let request = InferRequest::Chat(
        ChatRequest::new(
            vec![ChatMessage {
                role: Role::User,
                parts: vec![MessagePart::Text("hi".into())],
            }],
            Tier::Fast,
            DataClass::Public,
            Usage::Interactive,
        )
        .with_control(ChatControl::new().with_tool_calls(ToolParallelism::Many)),
    );
    let mut events = Collect::default();
    let reply = broker.infer(&app, request, &mut events).await;
    assert_eq!(reply, InferReply::Refused(InferRefusal::Unavailable));
    assert!(events.0.is_empty());
}

#[derive(Default)]
struct Collect(Vec<InferEvent>);

impl ChatSink for Collect {
    fn event(&mut self, event: InferEvent) -> Flow {
        self.0.push(event);
        Flow::Continue
    }
}
