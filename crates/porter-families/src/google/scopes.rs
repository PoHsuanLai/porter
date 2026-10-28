//! Which scopes a sign-in asks for, one set per capability, and how Google classes each.
//!
//! The scope names and their classes are from Google's "OAuth 2.0 Scopes for Google APIs" page
//! and its sensitive and restricted scope lists, written down without network access: they are
//! unverified (FINDINGS), and docs/google.md, which tells the owner what to put on the consent
//! screen, is checked against this table by a test.

use porter_capability::{Albums, Ask, Capability, CapabilityKind, LibraryRead, Offered};
use porter_core::{Family, GrantedScopes};
use porter_oauth::MailRights;

/// Gmail over IMAP and SMTP (XOAUTH2). Restricted.
pub(super) const MAIL: &str = "https://mail.google.com/";
const OPENID: &str = "openid";
const EMAIL: &str = "https://www.googleapis.com/auth/userinfo.email";
const PROFILE: &str = "https://www.googleapis.com/auth/userinfo.profile";
const CALENDAR: &str = "https://www.googleapis.com/auth/calendar";
const CONTACTS: &str = "https://www.googleapis.com/auth/contacts";
const TASKS: &str = "https://www.googleapis.com/auth/tasks";
const DRIVE_APP_FOLDER: &str = "https://www.googleapis.com/auth/drive.appdata";
/// The whole of the person's Drive. Restricted, like Mail's: asked only on use.
const DRIVE: &str = "https://www.googleapis.com/auth/drive";
const PHOTOS_UPLOAD: &str = "https://www.googleapis.com/auth/photoslibrary.appendonly";
const PHOTOS_PICKER: &str = "https://www.googleapis.com/auth/photospicker.mediaitems.readonly";

/// How Google treats a scope: what its consent screen and verification ask of the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Sensitivity {
    /// No verification: the identity scopes and the Drive app folder.
    NonSensitive,
    /// Needs the consent screen verified (a warning screen and a 100 user cap until it is).
    Sensitive,
    /// Needs verification and a yearly third-party security assessment: only a person's own
    /// client in testing mode can use it.
    Restricted,
}

/// What is always asked: who the account is.
pub(super) const IDENTITY: &[&str] = &[OPENID, EMAIL, PROFILE];

/// The scopes the capability `kind` needs, none for a kind Google has no service for.
pub fn scopes_of(kind: CapabilityKind) -> &'static [&'static str] {
    match kind {
        CapabilityKind::Mail => &[MAIL],
        CapabilityKind::Calendar => &[CALENDAR],
        CapabilityKind::Contacts => &[CONTACTS],
        CapabilityKind::Tasks => &[TASKS],
        CapabilityKind::Storage => &[DRIVE_APP_FOLDER],
        CapabilityKind::Photos => &[PHOTOS_UPLOAD, PHOTOS_PICKER],
        _ => &[],
    }
}

/// The scopes of the capability `kind`'s on-use row (`ask = "on_use"`), which a sign-in never
/// asks: only a sign-in for that access alone (`SignInMode::Extend`) does. None for a kind with
/// no such row.
pub fn on_use_scopes_of(kind: CapabilityKind) -> &'static [&'static str] {
    match kind {
        CapabilityKind::Storage => &[DRIVE],
        _ => &[],
    }
}

/// The scopes a token for a grant of `kind`, used against the API of `family`, is refreshed
/// with: the kind's own, and for Photos only the one its API takes (a person may have ticked one
/// of the two). Never the identity scopes or another kind's. `ask` is how the kind's rows are
/// asked (`ProviderSpec::ask_for`): an on-use kind's token carries its on-use scopes. Empty for a
/// kind Google has no service for.
pub(super) fn grant_scopes(
    kind: CapabilityKind,
    family: Family,
    ask: Ask,
) -> &'static [&'static str] {
    match (kind, family, ask) {
        (_, _, Ask::OnUse) => on_use_scopes_of(kind),
        (CapabilityKind::Photos, Family::GooglePhotosUpload, _) => &[PHOTOS_UPLOAD],
        (CapabilityKind::Photos, Family::GooglePhotosPicker, _) => &[PHOTOS_PICKER],
        _ => scopes_of(kind),
    }
}

/// How Google classes `scope`; a scope this table does not know is taken as the most demanding.
pub fn scope_sensitivity(scope: &str) -> Sensitivity {
    match scope {
        OPENID | EMAIL | PROFILE | DRIVE_APP_FOLDER => Sensitivity::NonSensitive,
        CALENDAR | CONTACTS | TASKS | PHOTOS_UPLOAD | PHOTOS_PICKER => Sensitivity::Sensitive,
        MAIL | DRIVE => Sensitivity::Restricted,
        _ => Sensitivity::Restricted,
    }
}

/// The scopes for the kinds that are on: identity always, a kind's own when it is on, and mail's
/// restricted scope only for a person's own client.
pub(super) fn scopes_for(kinds: &[CapabilityKind], mail: MailRights) -> Vec<String> {
    let mut scopes: Vec<String> = IDENTITY.iter().map(|s| (*s).to_owned()).collect();
    for kind in kinds {
        let allowed = *kind != CapabilityKind::Mail || mail == MailRights::Byo;
        if allowed {
            scopes.extend(scopes_of(*kind).iter().map(|s| (*s).to_owned()));
        }
    }
    scopes
}

/// What a token's granted scopes say. Google lets a person untick some of what was asked
/// (granular consent), and says so in the token answer; an answer that does not say leaves it
/// unknown and everything sign-in asks is taken as granted (nothing asked on use is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Granted(Option<Vec<String>>);

impl Granted {
    pub(super) fn of(scopes: Vec<String>) -> Self {
        Self((!scopes.is_empty()).then_some(scopes))
    }

    /// What a credential's record says.
    pub(super) fn recorded(record: &GrantedScopes) -> Self {
        Self(record.scopes().map(<[String]>::to_vec))
    }

    /// The record a credential keeps of this.
    pub(super) fn record(&self) -> GrantedScopes {
        match &self.0 {
            Some(scopes) => GrantedScopes::of(scopes.iter().cloned()),
            None => GrantedScopes::unknown(),
        }
    }

    /// Whether the scopes `kind` needs were granted (either one of Photos' two is enough: the
    /// picker alone still imports, an upload alone still uploads).
    pub(super) fn covers(&self, kind: CapabilityKind) -> bool {
        match &self.0 {
            None => true,
            Some(granted) => {
                let needs = scopes_of(kind);
                let held = |scope: &&str| granted.iter().any(|g| g == scope);
                match kind {
                    CapabilityKind::Photos => needs.iter().any(held),
                    _ => needs.iter().all(held),
                }
            }
        }
    }
}

impl Granted {
    /// Whether every one of `scopes` was granted (all of them, when the answer did not say):
    /// a refresh may only narrow the grant, never ask beyond it.
    pub(super) fn holds_all(&self, scopes: &[&str]) -> bool {
        scopes.iter().all(|scope| self.holds(scope))
    }

    /// Whether a grant whose rows are asked as `ask` may be refreshed with `scopes`: all of them
    /// were granted, or the answer did not say and they are what sign-in asks. An on-use scope
    /// is never taken as granted unasked.
    pub(super) fn admits(&self, scopes: &[&str], ask: Ask) -> bool {
        match (&self.0, ask) {
            (None, Ask::OnUse) => false,
            _ => self.holds_all(scopes),
        }
    }

    fn holds(&self, scope: &str) -> bool {
        self.0
            .as_ref()
            .is_none_or(|granted| granted.iter().any(|g| g == scope))
    }

    /// Whether the scope the API of `family` needs was granted. Only the two Photos APIs differ
    /// by scope within a kind: a person who ticks the picker alone has no upload API, and the
    /// other way round.
    pub(super) fn allows(&self, family: Family) -> bool {
        match family {
            Family::GooglePhotosUpload => self.holds(PHOTOS_UPLOAD),
            Family::GooglePhotosPicker => self.holds(PHOTOS_PICKER),
            _ => true,
        }
    }

    /// `cap` as far as the granted scopes reach: Photos without the append-only scope does not
    /// upload (nor make albums), without the picker scope reads nothing.
    pub(super) fn narrow(&self, cap: &Capability) -> Capability {
        match cap {
            Capability::Photos(photos) => {
                let mut photos = photos.clone();
                if !self.allows(Family::GooglePhotosUpload) {
                    photos.upload = Offered::Absent;
                    photos.albums = Albums::None;
                }
                if !self.allows(Family::GooglePhotosPicker) {
                    photos.library_read = LibraryRead::None;
                }
                Capability::Photos(photos)
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CapabilityKind as K;

    #[test]
    fn scopes_follow_the_kinds_that_are_on_and_mail_only_for_a_persons_own_client() {
        let with = |extra: &[&str]| {
            IDENTITY
                .iter()
                .chain(extra)
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(scopes_for(&[], MailRights::Withheld), with(&[]));
        assert_eq!(
            scopes_for(&[K::Calendar, K::Storage], MailRights::Withheld),
            with(&[CALENDAR, DRIVE_APP_FOLDER])
        );
        assert_eq!(
            scopes_for(&[K::Photos, K::Tasks, K::Contacts], MailRights::Withheld),
            with(&[PHOTOS_UPLOAD, PHOTOS_PICKER, TASKS, CONTACTS])
        );
        assert_eq!(
            scopes_for(&[K::Mail], MailRights::Withheld),
            with(&[]),
            "mail's restricted scope is not asked of a client that is not the person's own"
        );
        assert_eq!(scopes_for(&[K::Mail], MailRights::Byo), with(&[MAIL]));
        assert_eq!(
            scopes_for(&[K::Agent, K::Notes], MailRights::Byo),
            with(&[]),
            "kinds Google has no service for ask nothing"
        );
    }

    #[test]
    fn every_scope_has_a_class_and_only_mail_is_restricted() {
        const CASES: &[(&str, Sensitivity)] = &[
            (OPENID, Sensitivity::NonSensitive),
            (EMAIL, Sensitivity::NonSensitive),
            (PROFILE, Sensitivity::NonSensitive),
            (DRIVE_APP_FOLDER, Sensitivity::NonSensitive),
            (CALENDAR, Sensitivity::Sensitive),
            (CONTACTS, Sensitivity::Sensitive),
            (TASKS, Sensitivity::Sensitive),
            (PHOTOS_UPLOAD, Sensitivity::Sensitive),
            (PHOTOS_PICKER, Sensitivity::Sensitive),
            (MAIL, Sensitivity::Restricted),
            (
                "https://www.googleapis.com/auth/drive",
                Sensitivity::Restricted,
            ),
            ("something-new", Sensitivity::Restricted),
        ];
        for (scope, want) in CASES {
            assert_eq!(scope_sensitivity(scope), *want, "{scope}");
        }
        assert_eq!(on_use_scopes_of(K::Storage), [DRIVE]);
        assert_eq!(scope_sensitivity(DRIVE), Sensitivity::Restricted);
        let restricted: Vec<&str> = [
            K::Mail,
            K::Calendar,
            K::Contacts,
            K::Tasks,
            K::Storage,
            K::Photos,
        ]
        .iter()
        .flat_map(|k| scopes_of(*k).iter().copied())
        .filter(|s| scope_sensitivity(s) == Sensitivity::Restricted)
        .collect();
        assert_eq!(
            restricted,
            [MAIL],
            "of what sign-in asks, only mail is restricted"
        );
    }

    #[test]
    fn a_grant_is_refreshed_with_its_own_kinds_scopes_only() {
        use Family as F;
        const CASES: &[(K, F, &[&str])] = &[
            (K::Calendar, F::GoogleCalendar, &[CALENDAR]),
            (K::Contacts, F::GooglePeople, &[CONTACTS]),
            (K::Tasks, F::GoogleTasks, &[TASKS]),
            (K::Storage, F::GoogleDrive, &[DRIVE_APP_FOLDER]),
            (K::Photos, F::GooglePhotosUpload, &[PHOTOS_UPLOAD]),
            (K::Photos, F::GooglePhotosPicker, &[PHOTOS_PICKER]),
            (K::Mail, F::Imap, &[MAIL]),
            (K::Mail, F::Smtp, &[MAIL]),
            (K::Notes, F::GoogleDrive, &[]),
        ];
        for (kind, family, want) in CASES {
            let got = grant_scopes(*kind, *family, Ask::SignIn);
            assert_eq!(got, *want, "{kind:?} {family:?}");
            assert!(
                got.iter().all(|s| !IDENTITY.contains(s)),
                "{kind:?}: no identity scope in an app's token"
            );
        }
        let granted =
            |scopes: &[&str]| Granted::of(scopes.iter().map(|s| (*s).to_owned()).collect());
        let some = granted(&[OPENID, CALENDAR, PHOTOS_PICKER]);
        assert!(some.holds_all(&[CALENDAR]));
        assert!(some.holds_all(&[PHOTOS_PICKER]));
        assert!(!some.holds_all(&[PHOTOS_UPLOAD]));
        assert!(!some.holds_all(&[CALENDAR, TASKS]));
        assert!(granted(&[]).holds_all(&[TASKS]), "unknown is not a refusal");
    }

    #[test]
    fn the_whole_drive_is_never_asked_at_sign_in_and_never_taken_as_granted_unasked() {
        let every = [
            K::Mail,
            K::Calendar,
            K::Contacts,
            K::Tasks,
            K::Storage,
            K::Photos,
        ];
        assert!(!scopes_for(&every, MailRights::Byo).contains(&DRIVE.to_owned()));
        assert_eq!(
            grant_scopes(K::Storage, Family::GoogleDrive, Ask::OnUse),
            [DRIVE]
        );
        assert_eq!(
            grant_scopes(K::Storage, Family::GoogleDrive, Ask::SignIn),
            [DRIVE_APP_FOLDER]
        );
        let unknown = Granted::of(Vec::new());
        assert!(unknown.admits(&[DRIVE_APP_FOLDER], Ask::SignIn));
        assert!(!unknown.admits(&[DRIVE], Ask::OnUse));
        let without = Granted::of(vec![DRIVE_APP_FOLDER.to_owned()]);
        assert!(!without.admits(&[DRIVE], Ask::OnUse));
        let with = Granted::of(vec![DRIVE_APP_FOLDER.to_owned(), DRIVE.to_owned()]);
        assert!(with.admits(&[DRIVE], Ask::OnUse));
        assert_eq!(Granted::recorded(&with.record()), with);
        assert!(unknown.record().is_unknown());
    }

    #[test]
    fn granular_consent_drops_what_the_person_unticked() {
        let granted =
            |scopes: &[&str]| Granted::of(scopes.iter().map(|s| (*s).to_owned()).collect());
        let some = granted(&[OPENID, CALENDAR, PHOTOS_PICKER]);
        assert!(some.covers(K::Calendar));
        assert!(!some.covers(K::Tasks));
        assert!(some.covers(K::Photos), "the picker alone is still photos");
        assert!(!some.covers(K::Storage));
        let unknown = Granted::of(Vec::new());
        assert!(
            unknown.covers(K::Tasks),
            "an answer that does not say is not a refusal"
        );
    }
}
