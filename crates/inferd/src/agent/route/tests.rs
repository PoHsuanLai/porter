use super::*;
use crate::agent::refusal::Cause;
use porter_infer::{ClassFloor, Floor, Policy};

fn arg(kind: &str, id: &str, mode: &str, ids: &[&str]) -> RouteArg {
    (
        kind.into(),
        id.into(),
        mode.into(),
        ids.iter().map(|s| (*s).to_owned()).collect(),
    )
}

#[test]
fn routes_read_by_kind_and_by_which_ids_they_serve() {
    let account =
        Route::parse(arg("account", "anthropic", "listed", &["claude-x"])).expect("route");
    assert_eq!(
        account.target,
        Target::Account(AccountId::parse("anthropic").expect("id"))
    );
    assert!(account.serves("claude-x"));
    assert!(!account.serves("claude-y"));
    let any = Route::parse(arg("account", "openai", "any", &[])).expect("route");
    assert!(any.serves("whatever"));
    let local = Route::parse(arg("model", "qwen3", "listed", &[])).expect("route");
    assert_eq!(
        local.models,
        Models::Listed(vec!["qwen3".into()]),
        "its own id by default"
    );
    let aliased =
        Route::parse(arg("model", "qwen3", "listed", &["claude-sonnet-4"])).expect("route");
    assert!(aliased.serves("claude-sonnet-4") && !aliased.serves("qwen3"));
}

#[test]
fn routes_that_say_nothing_clear_are_refused_invalid() {
    let bad = [
        arg("planet", "x", "any", &[]),
        arg("account", "Not An Id", "any", &[]),
        arg("account", "anthropic", "listed", &[]),
        arg("account", "anthropic", "any", &["x"]),
        arg("account", "anthropic", "some", &[]),
        arg("model", "", "any", &[]),
        arg("model", "m", "listed", &[""]),
    ];
    for one in bad {
        let refusal = Route::parse(one.clone()).expect_err(&format!("{one:?}"));
        assert_eq!(refusal.cause(), Cause::InvalidArgs, "{one:?}");
    }
}

#[test]
fn a_program_is_an_app_named_for_it() {
    let program = AgentProgram::parse("claude-code").expect("program");
    let app = agent_app(&program);
    assert_eq!(app.name.as_str(), "org.quire.Agent.claude-code");
    assert_eq!(app.isolation, Isolation::Unsandboxed);
}

#[test]
fn the_cloud_needs_local_only_off_and_a_floor_of_anywhere() {
    let cloud = Locality::Cloud { region: None };
    let mut settings = Settings::default();
    assert!(
        !admits(&settings, DataClass::Files, &cloud),
        "local-only is on by default"
    );
    assert!(admits(&settings, DataClass::Files, &Locality::OnDevice));
    settings.policy = Policy {
        local_only: LocalOnly::Off,
        floors: vec![ClassFloor {
            class: DataClass::Files,
            floor: Floor::OnDevice,
        }],
    };
    assert!(
        !admits(&settings, DataClass::Files, &cloud),
        "the floor says this computer"
    );
    assert!(
        admits(&settings, DataClass::Public, &cloud),
        "a class without a row goes anywhere"
    );
    assert!(!admits(
        &settings,
        DataClass::Files,
        &Locality::LocalNetwork
    ));
    settings.my_network = crate::settings::MyNetwork::On;
    assert!(admits(&settings, DataClass::Files, &Locality::LocalNetwork));
    assert!(
        !admits(&settings, DataClass::Files, &cloud),
        "my_network does not open the cloud"
    );
}
