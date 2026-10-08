use super::*;
use crate::account::AccountLabel;
use crate::capability::CapabilityKind;
use crate::effective::Toggle;
use crate::sheet::fields::{Entry, FieldKind, FieldSpec, Presence};
use crate::sheet::progress::{ServiceRow, ServiceState};

fn provider(text: &str) -> ProviderId {
    ProviderId::parse(text).expect("provider id")
}

fn rows() -> Vec<ProviderRow> {
    vec![ProviderRow {
        id: provider("nextcloud"),
        label: "Nextcloud".into(),
        mark: "nextcloud".into(),
        kind: crate::sheet::view::RowKind::Provider,
        auth: crate::sheet::view::ProviderKind::Service,
        mark_face: None,
        group: None,
    }]
}

fn review() -> Review {
    Review {
        label: AccountLabel("ada@cloud.example.org".into()),
        services: vec![ServiceRow {
            kind: CapabilityKind::Storage,
            state: ServiceState::Offered(Toggle::On),
            limit: None,
        }],
        endpoints: vec![],
    }
}

#[test]
fn a_new_sheet_starts_where_the_purpose_says() {
    let cases = [
        (
            "no hint: the list",
            Purpose::Add {
                hint: ProviderHint::Any,
                allow: None,
            },
            Stage::Choosing(rows()),
        ),
        (
            "a hint: working on it",
            Purpose::Add {
                hint: ProviderHint::Provider(provider("nextcloud")),
                allow: None,
            },
            Stage::Working(provider("nextcloud")),
        ),
        (
            "re-sign-in: working on it",
            Purpose::Reauthenticate {
                account: AccountId::parse("cloud").expect("id"),
                provider: provider("nextcloud"),
            },
            Stage::Working(provider("nextcloud")),
        ),
    ];
    for (name, purpose, stage) in cases {
        assert_eq!(Sheet::new(purpose, rows()).stage, stage, "{name}");
    }
}

#[test]
fn every_stage_has_its_view() {
    let nextcloud = provider("nextcloud");
    let url = EndpointUrl::parse("https://cloud.example.org/login/v2/flow").expect("url");
    let page = WebUrl::parse("https://login.example.org/authorize?state=b").expect("url");
    let field = FieldSpec {
        kind: FieldKind::Password,
        entry: Entry::Secret,
        presence: Presence::Required,
        prefill: None,
    };
    let sheet = |stage| Sheet {
        purpose: Purpose::Add {
            hint: ProviderHint::Any,
            allow: None,
        },
        stage,
        providers: rows(),
    };
    let row = rows().first().cloned();
    let cases: Vec<(&str, Stage, SheetView)> = vec![
        (
            "list",
            Stage::Choosing(rows()),
            SheetView::Providers(rows()),
        ),
        (
            "form",
            Stage::Asking {
                provider: nextcloud.clone(),
                fields: vec![field.clone()],
                problem: None,
            },
            SheetView::SignIn(SignInView {
                provider: nextcloud.clone(),
                row: row.clone(),
                fields: vec![field],
                problem: None,
            }),
        ),
        (
            "working",
            Stage::Working(nextcloud.clone()),
            SheetView::Working {
                provider: nextcloud.clone(),
                row: row.clone(),
            },
        ),
        (
            "browser",
            Stage::Browser {
                provider: nextcloud.clone(),
                url: page.clone(),
            },
            SheetView::BrowserWait {
                provider: nextcloud.clone(),
                row: row.clone(),
                url: page.clone(),
            },
        ),
        (
            "code",
            Stage::Code {
                provider: nextcloud.clone(),
                user_code: UserCode("ABCD-EFGH".into()),
                url: url.clone(),
            },
            SheetView::ShowCode {
                provider: nextcloud.clone(),
                row: row.clone(),
                user_code: UserCode("ABCD-EFGH".into()),
                url,
            },
        ),
        (
            "review",
            Stage::Reviewing {
                provider: nextcloud.clone(),
                review: review(),
            },
            SheetView::Review(ReviewView {
                provider: nextcloud.clone(),
                row: row.clone(),
                review: review(),
                allow: None,
                allow_label: None,
            }),
        ),
        ("added", Stage::Added, SheetView::Done),
        (
            "failed",
            Stage::Failed {
                provider: nextcloud.clone(),
                fault: SignInFault::Refused,
            },
            SheetView::Failed {
                provider: nextcloud,
                row,
                fault: SignInFault::Refused,
            },
        ),
    ];
    for (name, stage, view) in cases {
        assert_eq!(sheet(stage).view(), view, "{name}");
    }
}

#[test]
fn the_review_names_the_app_to_allow_when_a_chooser_started_the_sheet() {
    let app = AppId {
        name: crate::AppName::parse("org.quire.Mail").expect("name"),
        isolation: crate::Isolation::Flatpak,
    };
    let sheet = Sheet {
        purpose: Purpose::Add {
            hint: ProviderHint::Any,
            allow: Some(app.clone()),
        },
        stage: Stage::Reviewing {
            provider: provider("nextcloud"),
            review: review(),
        },
        providers: rows(),
    };
    match sheet.view() {
        SheetView::Review(view) => assert_eq!(view.allow, Some(app)),
        other => panic!("not the review: {other:?}"),
    }
}

#[test]
fn the_machine_closes_on_dismiss() {
    let sheet = Sheet::new(
        Purpose::Add {
            hint: ProviderHint::Any,
            allow: None,
        },
        rows(),
    );
    let (_, effects) = step(sheet, SheetEvent::Input(SheetInput::Dismiss));
    assert_eq!(effects, vec![SheetEffect::Close(SheetEnd::Dismissed)]);
}

#[test]
fn every_view_of_an_agent_provider_carries_its_display_name_and_kind() {
    let agent = ProviderRow {
        id: provider("claude-code"),
        label: "Claude Code".into(),
        mark: "claude-code".into(),
        kind: crate::sheet::view::RowKind::Provider,
        auth: crate::sheet::view::ProviderKind::AgentLogin,
        mark_face: None,
        group: None,
    };
    let sheet = Sheet {
        purpose: Purpose::Reauthenticate {
            account: crate::AccountId::parse("claude-code").expect("id"),
            provider: agent.id.clone(),
        },
        stage: Stage::Working(agent.id.clone()),
        providers: vec![agent.clone()],
    };
    assert_eq!(
        sheet.view(),
        SheetView::Working {
            provider: agent.id.clone(),
            row: Some(agent),
        }
    );
}
