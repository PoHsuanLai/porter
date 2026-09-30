//! The fakes behave as the seams they stand in for.

use porter_core::consent::Usage;
use porter_core::{AccountId, DataClass, ModelId, Provenance, Tier};
use porter_fake::{FakeModel, cloud_provider, llm_account, storage_account};
use porter_infer::{ChatMessage, ChatRequest, MessagePart, Model, ReplyShape, Role};
use porter_provider::{Presented, Provider, ProviderError};

#[tokio::test]
async fn discovery_reports_every_declared_row_as_discovered() {
    let provider = cloud_provider();
    let account = storage_account();
    let claims = provider
        .discover(&account.id, &Presented::Anonymous)
        .await
        .expect("discovers");
    let kinds: Vec<_> = claims
        .iter()
        .map(|c| (c.offer.kind(), c.provenance))
        .collect();
    let declared: Vec<_> = account
        .capabilities
        .iter()
        .map(|c| (c.offer.kind(), Provenance::Discovered))
        .collect();
    assert_eq!(kinds, declared);
}

#[tokio::test]
async fn an_account_that_needs_a_secret_cannot_open_without_one() {
    let provider = cloud_provider();
    let opened = provider
        .open(&storage_account().id, Presented::Anonymous)
        .await;
    assert_eq!(opened.err(), Some(ProviderError::Unauthorized));
}

#[tokio::test]
async fn the_fake_model_echoes_the_last_text() {
    let model = FakeModel::on_device(
        llm_account().id,
        ModelId::parse("echo").expect("id"),
        vec![],
    );
    let request = ChatRequest {
        messages: vec![
            ChatMessage {
                role: Role::System,
                parts: vec![MessagePart::Text("be brief".into())],
            },
            ChatMessage {
                role: Role::User,
                parts: vec![MessagePart::Text("hello".into())],
            },
        ],
        shape: ReplyShape::Text,
        tier: Tier::Fast,
        class: DataClass::Public,
        usage: Usage::Interactive,
    };
    let reply = model.chat(&request).await.expect("chat");
    assert_eq!(reply.text, "hello");
    assert_eq!(
        reply.served.account,
        AccountId::parse("fake-llm").expect("id")
    );
}
