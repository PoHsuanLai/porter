//! The first frame of a session on the latchkey socket: pinned JSON, and told apart from an
//! accountd call by its kind.

use porter_core::need::LlmNeed;
use porter_core::{AccountsRequest, DataClass, Need, Tier, Tokens};
use porter_infer::{LinkHello, OpenFrame, OpenOptions};

fn hello() -> LinkHello {
    LinkHello::Open(OpenFrame::new(
        Need::Llm(LlmNeed::new(Default::default(), Tokens(8))),
        DataClass::Notes,
        Tier::Fast,
        OpenOptions::default(),
    ))
}

#[test]
fn the_hello_is_pinned_and_round_trips() {
    let text = serde_json::to_string(&hello()).expect("json");
    assert_eq!(
        text,
        r#"{"kind":"open","v":{"need":{"kind":"llm","v":{"features":[],"context":8}},"class":"notes","tier":"fast","options":{"traceparent":null}}}"#
    );
    assert_eq!(
        serde_json::from_str::<LinkHello>(&text).expect("back"),
        hello()
    );
}

#[test]
fn an_agent_can_tell_a_hello_from_an_accountd_call_by_its_first_frame() {
    let hello_text = serde_json::to_string(&hello()).expect("json");
    let call_text = serde_json::to_string(&AccountsRequest::ListGrants).expect("json");
    assert!(serde_json::from_str::<AccountsRequest>(&hello_text).is_err());
    assert!(serde_json::from_str::<LinkHello>(&call_text).is_err());
}
