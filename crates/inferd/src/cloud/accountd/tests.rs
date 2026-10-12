use super::*;

#[test]
fn what_inferd_does_about_each_way_the_key_store_can_refuse() {
    let table = [
        (Refusal::Unavailable, AccountdFault::Unreachable),
        (Refusal::Denied, AccountdFault::Refused),
        (Refusal::NeedsReauth, AccountdFault::Refused),
        (Refusal::UnknownGrant, AccountdFault::Refused),
        (Refusal::AudienceNotGranted, AccountdFault::Refused),
    ];
    for (refusal, want) in table {
        assert_eq!(AccountdFault::from(refusal), want, "{refusal:?}");
    }
}
