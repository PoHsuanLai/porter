//! `Peer.ReportLocal`: a probed local runtime becoming an account of its provider file, going
//! offline and never being deleted for it. Only a porter daemon may report.

mod common;

use common::*;
use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::{
    AccountId, AccountState, Claim, ModelId, Offer, Provenance, Subject, Tokens, effective,
};
use porter_dbus::{CallerRole, Details, PeerProxy, to_vardict};

fn model_claim(model: &str, context: u32, provenance: Provenance) -> Claim {
    Claim {
        subject: Subject::Model(ModelId::parse(model).expect("model id")),
        offer: Offer::Present(Capability::Llm(LlmCap {
            features: [LlmFeature::Chat, LlmFeature::Tools].into(),
            context: Tokens(context),
            max_output: Tokens(context),
            wire: LlmWire::ChatCompletions,
        })),
        provenance,
    }
}

fn on_the_bus(claim: &Claim) -> (String, Details) {
    let kind = serde_json::to_value(claim.offer.kind()).expect("kind");
    let serde_json::Value::Object(fields) = serde_json::to_value(claim).expect("claim") else {
        panic!("a claim is a record");
    };
    (kind.as_str().expect("slug").to_owned(), to_vardict(&fields))
}

async fn daemon(rig: &Rig) -> PeerProxy<'static> {
    let connection = rig
        .client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
        .await;
    PeerProxy::new(&connection).await.expect("proxy")
}

fn account(rig: &Rig, id: &str) -> Option<porter_core::Account> {
    rig.service
        .registry()
        .accounts
        .into_iter()
        .find(|a| a.id.as_str() == id)
}

fn models_of(account: &porter_core::Account) -> Vec<String> {
    account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.subject {
            Subject::Model(id) => Some(id.to_string()),
            Subject::Account | Subject::Agent(_) => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_is_reported_is_an_account_holding_its_models_and_is_saved() {
    let rig = Rig::start().await;
    // The runtime's account is not there yet: a first report makes it.
    rig.service
        .remove_account(&AccountId::parse("fake-llm").expect("id"))
        .await
        .expect("removed");
    assert!(account(&rig, "fake-llm").is_none());

    let peer = daemon(&rig).await;
    let claims: Vec<_> = [
        model_claim("llama3.2-3b", 8192, Provenance::Discovered),
        model_claim("qwen2.5-7b", 32768, Provenance::Discovered),
    ]
    .iter()
    .map(on_the_bus)
    .collect();
    let id = peer
        .report_local("fake-llm", claims, "ok")
        .await
        .expect("reported");
    assert_eq!(id, "fake-llm");

    let made = account(&rig, "fake-llm").expect("the account");
    assert_eq!(made.provider.as_str(), "fake-llm");
    assert_eq!(made.label.0, "Fake Runtime");
    assert_eq!(made.state, AccountState::Ok);
    assert_eq!(made.auth, porter_core::AuthKind::LocalRuntime);
    assert_eq!(models_of(&made), ["llama3.2-3b", "qwen2.5-7b"]);
    assert!(
        made.endpoints.is_empty(),
        "a runtime on this computer has no endpoint to relay to"
    );
    // Written to the store, not only held.
    let stored = rig.store.stored().expect("saved");
    assert!(stored.accounts.iter().any(|a| a.id.as_str() == "fake-llm"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_stops_goes_offline_with_its_models_kept_and_comes_back() {
    let rig = Rig::start().await;
    let peer = daemon(&rig).await;
    let claims = vec![on_the_bus(&model_claim(
        "llama3.2-3b",
        8192,
        Provenance::Discovered,
    ))];
    peer.report_local("fake-llm", claims.clone(), "ok")
        .await
        .expect("up");

    // Offline, with nothing to say of models: the account stays and keeps what it ran.
    peer.report_local("fake-llm", vec![], "offline")
        .await
        .expect("down");
    let down = account(&rig, "fake-llm").expect("never deleted for going offline");
    assert_eq!(down.state, AccountState::Offline);
    assert_eq!(models_of(&down), ["llama3.2-3b"]);

    // Back, with a different list: the new list replaces the old, and the state is ok.
    let other = vec![on_the_bus(&model_claim(
        "phi-4",
        16384,
        Provenance::Discovered,
    ))];
    peer.report_local("fake-llm", other, "ok")
        .await
        .expect("up again");
    let up = account(&rig, "fake-llm").expect("account");
    assert_eq!(up.state, AccountState::Ok);
    assert_eq!(models_of(&up), ["phi-4"]);
    assert_eq!(rig.store.stored().expect("saved").accounts.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_person_turned_off_stays_off_across_reports() {
    use porter_core::{CapabilityKind, Toggle};
    let rig = Rig::start().await;
    let id = AccountId::parse("fake-llm").expect("id");
    rig.service
        .set_toggle(&id, CapabilityKind::Llm, Toggle::Off)
        .await
        .expect("toggled");
    let peer = daemon(&rig).await;
    peer.report_local(
        "fake-llm",
        vec![on_the_bus(&model_claim(
            "llama3.2-3b",
            8192,
            Provenance::Discovered,
        ))],
        "ok",
    )
    .await
    .expect("reported");
    let held = account(&rig, "fake-llm").expect("account");
    let want = effective(
        &[model_claim("llama3.2-3b", 8192, Provenance::Discovered)],
        &[porter_core::KindToggle {
            kind: CapabilityKind::Llm,
            toggle: Toggle::Off,
        }],
    );
    assert_eq!(held.capabilities, want);
    assert!(matches!(held.capabilities[0].offer, Offer::Absent { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reported_model_is_in_the_verdicts_the_daemon_asks_for() {
    let rig = Rig::start().await;
    let peer = daemon(&rig).await;
    peer.report_local(
        "fake-llm",
        vec![on_the_bus(&model_claim(
            "llama3.2-3b",
            8192,
            Provenance::Discovered,
        ))],
        "ok",
    )
    .await
    .expect("reported");
    let need = porter_dbus::need_to_dbus(&porter_core::Need::Llm(porter_core::need::LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    }));
    let rows = peer
        .verdicts(
            &("org.quire.Mail".to_owned(), "flatpak".to_owned()),
            &need,
            "mail",
            "interactive",
        )
        .await
        .expect("verdicts");
    let row = rows
        .iter()
        .find(|row| row.0 == "fake-llm")
        .expect("the runtime's account");
    assert_eq!(row.1, "ask", "the person has not given Mail a grant on it");
    assert_eq!(text_of(&row.2, "provider").as_deref(), Some("fake-llm"));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_porter_daemon_may_report() {
    let rig = Rig::start().await;
    for refused in [
        rig.client_as(caller("org.example.App", CallerRole::App))
            .await,
        rig.client_as(caller("org.quire.Companion", CallerRole::Agent))
            .await,
        rig.client_as(caller("org.quire.Settings", CallerRole::Settings))
            .await,
        rig.stranger().await,
    ] {
        let err = PeerProxy::new(&refused)
            .await
            .expect("proxy")
            .report_local("fake-llm", vec![], "offline")
            .await
            .expect_err("refused");
        assert_eq!(error_name(&err), ACCESS_DENIED);
    }
    assert_eq!(
        account(&rig, "fake-llm").expect("untouched").state,
        AccountState::Ok
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_that_is_not_what_a_probe_can_say_is_refused_and_changes_nothing() {
    let rig = Rig::start().await;
    let peer = daemon(&rig).await;
    let good = model_claim("llama3.2-3b", 8192, Provenance::Discovered);
    let mut absent = good.clone();
    absent.offer = Offer::Absent {
        kind: porter_core::CapabilityKind::Llm,
        reason: porter_core::AbsentReason::TurnedOff,
    };
    let mut about_account = good.clone();
    about_account.subject = Subject::Account;
    let curated = model_claim("llama3.2-3b", 8192, Provenance::Curated);
    let wrong_kind = ("embeddings".to_owned(), on_the_bus(&good).1);
    let not_a_claim = ("llm".to_owned(), Details::new());
    let many: Vec<_> = (0..300)
        .map(|n| on_the_bus(&model_claim(&format!("m{n}"), 2048, Provenance::Discovered)))
        .collect();

    let table = vec![
        ("no such provider", "no-such", vec![], "ok"),
        ("a provider that signs in", "fake-cloud", vec![], "ok"),
        (
            "a state a runtime has no word for",
            "fake-llm",
            vec![],
            "needs_reauth",
        ),
        ("a state that is not one", "fake-llm", vec![], "sleeping"),
        (
            "an absent offer",
            "fake-llm",
            vec![on_the_bus(&absent)],
            "ok",
        ),
        (
            "a claim about the account",
            "fake-llm",
            vec![on_the_bus(&about_account)],
            "ok",
        ),
        (
            "a claim a probe cannot make",
            "fake-llm",
            vec![on_the_bus(&curated)],
            "ok",
        ),
        (
            "a slug that is another kind",
            "fake-llm",
            vec![wrong_kind],
            "ok",
        ),
        (
            "a record that is no claim",
            "fake-llm",
            vec![not_a_claim],
            "ok",
        ),
        ("too many claims", "fake-llm", many, "ok"),
        ("a provider that is not an id", "Not An Id", vec![], "ok"),
    ];
    for (what, provider, claims, state) in table {
        let err = peer
            .report_local(provider, claims, state)
            .await
            .expect_err(what);
        assert!(
            error_name(&err).ends_with("InvalidArgs"),
            "{what}: {}",
            error_name(&err)
        );
    }
    let kept = account(&rig, "fake-llm").expect("untouched");
    assert_eq!(kept.state, AccountState::Ok);
    assert_eq!(
        rig.store.saves(),
        0,
        "nothing was saved for a refused report"
    );
}
