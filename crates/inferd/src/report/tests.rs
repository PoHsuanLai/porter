use super::*;
use crate::probe::testing::chat;
use porter_core::capability::LlmFeature;
use zbus::fdo;

#[test]
fn a_runtime_is_ok_or_offline_on_the_bus() {
    assert_eq!(state_slug(Standing::Online), "ok");
    assert_eq!(state_slug(Standing::Offline), "offline");
}

#[test]
fn a_claim_goes_on_the_bus_as_its_kinds_slug_and_its_fields_by_name() {
    let model = chat("llama3.2-3b", "llama3.2:3b", 8192, &[LlmFeature::Chat]);
    let (kind, fields) = claim_to_dbus(&model.claims[0]).expect("a vardict");
    assert_eq!(kind, "llm");
    let mut keys: Vec<_> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["offer", "provenance", "subject"]);
    let provenance =
        String::try_from(fields["provenance"].try_clone().expect("clone")).expect("text");
    assert_eq!(provenance, "discovered");
}

#[test]
fn an_error_name_says_whether_to_ask_again() {
    let table = [
        (
            fdo::Error::AccessDenied("not a daemon".into()),
            ReportFault::Refused,
        ),
        (
            fdo::Error::InvalidArgs("no such provider".into()),
            ReportFault::Rejected,
        ),
        (
            fdo::Error::ServiceUnknown("no accountd".into()),
            ReportFault::Unreachable,
        ),
        (fdo::Error::Failed("busy".into()), ReportFault::Unreachable),
    ];
    for (error, want) in table {
        assert_eq!(fault_of(&zbus::Error::FDO(Box::new(error))), want);
    }
    assert_eq!(
        fault_of(&zbus::Error::InterfaceNotFound),
        ReportFault::Unreachable
    );
}

/// What a bus error comes to: the one classification (porter-client's `PeerError`), then what
/// inferd does about it.
fn fault_of(error: &zbus::Error) -> ReportFault {
    ReportFault::from(PeerError::from(error))
}

#[tokio::test]
async fn with_no_accountd_every_report_is_unreachable() {
    let claims = chat("m", "m", 2048, &[LlmFeature::Chat]).claims;
    assert_eq!(
        NoReports
            .report(Runtime::Ollama, &claims, Standing::Online)
            .await,
        Err(ReportFault::Unreachable)
    );
}
