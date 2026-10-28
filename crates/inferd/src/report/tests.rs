use super::*;
use crate::probe::testing::chat;
use porter_capability::LlmFeature;

#[tokio::test]
async fn with_no_key_store_every_report_is_unreachable() {
    let claims = chat("m", "m", 2048, &[LlmFeature::Chat]).claims;
    assert_eq!(
        NoReports
            .report(Runtime::Ollama, &claims, Standing::Online)
            .await,
        Err(ReportFault::Unreachable)
    );
}
