use super::*;
use crate::app_id::{AppId, AppName, Isolation};
use crate::capability::CapabilityKind;
use crate::consent::grant::Usage;
use crate::consent::{Grant, GrantKey};
use crate::data_class::DataClass;
use crate::id::AccountId;
use crate::space::{SpaceId, SpaceScope};
use crate::units::UnixSeconds;

fn key(app: &str, usage: Usage) -> GrantKey {
    GrantKey {
        app: AppId {
            name: AppName::parse(app).expect("app name"),
            isolation: Isolation::Flatpak,
        },
        account: AccountId::parse("cloud").expect("account id"),
        kind: CapabilityKind::Storage,
        class: DataClass::Photos,
        usage,
        space: SpaceScope::Any,
    }
}

fn grant(id: &str, key: GrantKey, decision: Decision, scope: GrantScope, at: i64) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("grant id"),
        key,
        decision,
        scope,
        at: UnixSeconds(at),
    }
}

fn in_space(mut key: GrantKey, space: &str) -> GrantKey {
    key.space = SpaceScope::Only(SpaceId::parse(space).expect("space id"));
    key
}

fn granted(id: &str, scope: GrantScope) -> Verdict {
    Verdict::Granted {
        grant: GrantId::parse(id).expect("grant id"),
        scope,
    }
}

#[test]
fn decide_reads_the_newest_grant_for_the_exact_key() {
    use Decision::{Allow, Deny};
    use GrantScope::{Always, Once};
    let photos = || key("org.quire.Photos", Usage::Interactive);
    let cases: Vec<(&str, Vec<Grant>, GrantKey, Verdict)> = vec![
        ("nothing stored asks", vec![], photos(), Verdict::Ask),
        (
            "an allowance grants",
            vec![grant("g1", photos(), Allow, Always, 1)],
            photos(),
            granted("g1", Always),
        ),
        (
            "a once allowance says once",
            vec![grant("g1", photos(), Allow, Once, 1)],
            photos(),
            granted("g1", Once),
        ),
        (
            "a denial denies",
            vec![grant("g1", photos(), Deny, Always, 1)],
            photos(),
            Verdict::Denied,
        ),
        (
            "a newer allowance replaces a denial",
            vec![
                grant("g1", photos(), Deny, Always, 1),
                grant("g2", photos(), Allow, Always, 2),
            ],
            photos(),
            granted("g2", Always),
        ),
        (
            "a newer denial replaces an allowance",
            vec![
                grant("g2", photos(), Deny, Always, 3),
                grant("g1", photos(), Allow, Always, 2),
            ],
            photos(),
            Verdict::Denied,
        ),
        (
            "a denial wins a tie",
            vec![
                grant("g1", photos(), Allow, Always, 5),
                grant("g2", photos(), Deny, Always, 5),
            ],
            photos(),
            Verdict::Denied,
        ),
        (
            "another app's grant does not count",
            vec![grant(
                "g1",
                key("org.quire.Files", Usage::Interactive),
                Allow,
                Always,
                1,
            )],
            photos(),
            Verdict::Ask,
        ),
        (
            "an interactive grant does not cover background use",
            vec![grant("g1", photos(), Allow, Always, 1)],
            key("org.quire.Photos", Usage::Background),
            Verdict::Ask,
        ),
        (
            "a grant for one Space does not cover another",
            vec![grant("g1", in_space(photos(), "work"), Allow, Always, 1)],
            in_space(photos(), "home"),
            Verdict::Ask,
        ),
        (
            "a grant for one Space does not cover any",
            vec![grant("g1", in_space(photos(), "work"), Allow, Always, 1)],
            photos(),
            Verdict::Ask,
        ),
    ];
    for (name, grants, asked, expected) in cases {
        assert_eq!(decide(&grants, &asked), expected, "{name}");
    }
}

#[test]
fn another_apps_own_space_is_refused_and_shared_spaces_pass() {
    use Decision::Allow;
    use GrantScope::Always;
    let photos = || key("org.quire.Photos", Usage::Interactive);
    let files = || key("org.quire.Files", Usage::Interactive);
    // Photos' own Space 3, and a grant for Files over it slipped into the store.
    let theirs = || in_space(files(), "app:org.quire.Photos:3");
    let stored = vec![
        grant("g1", theirs(), Allow, Always, 1),
        grant(
            "g2",
            in_space(photos(), "app:org.quire.Photos:3"),
            Allow,
            Always,
            1,
        ),
        grant("g3", in_space(files(), "work"), Allow, Always, 1),
        grant("g4", in_space(files(), "desktop"), Allow, Always, 1),
        grant("g5", files(), Allow, Always, 1),
    ];
    let cases: Vec<(&str, GrantKey, Verdict)> = vec![
        (
            "another app's own Space is refused, even with a grant stored",
            theirs(),
            Verdict::Denied,
        ),
        (
            "and never asked",
            in_space(
                key("org.quire.Mail", Usage::Interactive),
                "app:org.quire.Photos:3",
            ),
            Verdict::Denied,
        ),
        (
            "the owner's own Space passes",
            in_space(photos(), "app:org.quire.Photos:3"),
            granted("g2", Always),
        ),
        (
            "a desktop-wide Space passes",
            in_space(files(), "work"),
            granted("g3", Always),
        ),
        (
            "outside any Space passes",
            in_space(files(), "desktop"),
            granted("g4", Always),
        ),
        ("any Space passes", files(), granted("g5", Always)),
    ];
    for (name, asked, expected) in cases {
        assert_eq!(decide_key(&stored, &asked), expected, "{name}");
    }
    assert!(!theirs().space_is_open());
    assert!(in_space(files(), "work").space_is_open());
}

#[test]
fn decide_is_generic_over_the_key() {
    let grants = vec![Grant {
        id: GrantId::parse("g1").expect("grant id"),
        key: "mail.thread.archive",
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }];
    assert_eq!(
        decide(&grants, &"mail.thread.archive"),
        granted("g1", GrantScope::Always)
    );
    assert_eq!(decide(&grants, &"mail.thread.delete"), Verdict::Ask);
}

#[test]
fn availability_reveals_only_the_best_state() {
    let yes = granted("g1", GrantScope::Always);
    let cases: Vec<(&str, Vec<Verdict>, Catalog, Availability)> = vec![
        (
            "a granted account",
            vec![Verdict::Ask, yes.clone()],
            Catalog::Offers,
            Availability::Granted,
        ),
        (
            "an undecided account",
            vec![Verdict::Denied, Verdict::Ask],
            Catalog::Offers,
            Availability::AvailableNeedsConsent,
        ),
        (
            "only refused accounts",
            vec![Verdict::Denied],
            Catalog::Offers,
            Availability::Denied,
        ),
        (
            "no account, a provider could",
            vec![],
            Catalog::Offers,
            Availability::NeedsAccount,
        ),
        (
            "no account, no provider",
            vec![],
            Catalog::Offerless,
            Availability::Unsupported,
        ),
    ];
    for (name, fitting, catalog, expected) in cases {
        assert_eq!(availability(&fitting, catalog), expected, "{name}");
    }
}
