use super::*;
use crate::local::Weights;

#[test]
fn a_replay_model_is_routable_and_has_no_weights_to_find() {
    let m = model(
        "scripted",
        Path::new("/c.jsonl"),
        Tokens(4096),
        Path::new("/s"),
    )
    .expect("model");
    assert_eq!(m.card.model.as_str(), "scripted");
    assert!(!m.card.capabilities.is_empty());
    assert_eq!(m.weights(), Weights::Present);
    assert_eq!(m.spec.need.0, 0);
    assert_eq!(m.socket.0, PathBuf::from("/s/replay-scripted.sock"));
    assert_eq!(m.spec.id.0, "replay:scripted");
    assert!(model("Not A Model", Path::new("/c"), Tokens(1), Path::new("/s")).is_none());
}
