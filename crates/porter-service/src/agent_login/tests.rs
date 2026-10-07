use super::*;
use porter_core::capability::AgentCap;
use porter_core::{AccountLabel, AccountState, Claim, Provenance, Restriction};

fn agent_claim(program: &str) -> Claim {
    Claim {
        subject: Subject::Agent(AgentProgram::parse(program).expect("program")),
        offer: Offer::Present(Capability::Agent(Box::new(AgentCap {
            program: AgentProgram::parse(program).expect("program"),
            key_env: None,
            base_url_env: None,
            protocols: Default::default(),
        }))),
        provenance: Provenance::Declared,
    }
}

fn account(claims: Vec<Claim>) -> Account {
    Account {
        id: AccountId::parse("claude-code").expect("id"),
        provider: ProviderId::parse("claude-code").expect("provider"),
        label: AccountLabel("Claude Code".to_owned()),
        state: AccountState::NeedsLogin,
        auth: AuthKind::AgentLogin,
        capabilities: claims,
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

#[test]
fn an_account_runs_the_program_its_claim_is_about() {
    let program = |claims| program_of(&account(claims)).map(|p| p.to_string());
    assert_eq!(
        program(vec![agent_claim("claude-code")]).as_deref(),
        Some("claude-code")
    );
    assert_eq!(program(Vec::new()), None);
}

#[test]
fn every_end_has_a_word_the_sheet_shows() {
    let table = [
        (
            LoginEnd::Reported(LoginOutcome::Cancelled),
            SignInFault::Cancelled,
        ),
        (
            LoginEnd::Reported(LoginOutcome::Failed(LoginFault::Refused)),
            SignInFault::Refused,
        ),
        (
            LoginEnd::Reported(LoginOutcome::Failed(LoginFault::Unreachable)),
            SignInFault::Unreachable,
        ),
        (
            LoginEnd::Reported(LoginOutcome::Failed(LoginFault::NotInstalled)),
            SignInFault::NotInstalled,
        ),
        (
            LoginEnd::Reported(LoginOutcome::Failed(LoginFault::TimedOut)),
            SignInFault::TimedOut,
        ),
        (
            LoginEnd::Reported(LoginOutcome::Failed(LoginFault::Other)),
            SignInFault::Unreadable,
        ),
        (LoginEnd::Expired, SignInFault::Expired),
        (LoginEnd::LauncherGone, SignInFault::NoLauncher),
    ];
    for (end, fault) in table {
        assert_eq!(end.fault(), fault, "{end:?}");
    }
}
