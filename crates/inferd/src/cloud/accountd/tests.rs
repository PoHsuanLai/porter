use super::*;

#[test]
fn what_inferd_does_about_each_way_accountd_can_fail() {
    let table = [
        (PeerError::Unreachable, AccountdFault::Unreachable),
        (PeerError::Denied("no".into()), AccountdFault::Refused),
        (PeerError::Unreadable, AccountdFault::Unreadable),
        (
            PeerError::Rejected("args".into()),
            AccountdFault::Unreachable,
        ),
        (
            PeerError::Malformed("id".into()),
            AccountdFault::Unreachable,
        ),
        (PeerError::Failed("busy".into()), AccountdFault::Unreachable),
    ];
    for (error, want) in table {
        assert_eq!(AccountdFault::from(&error), want, "{error:?}");
    }
}
