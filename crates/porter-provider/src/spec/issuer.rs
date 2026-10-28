//! What an OAuth issuer publishes, as data (`issuers.toml`): its endpoints, who answered at its
//! userinfo, the scopes it knows and for which capability kind, how it classes them, and where
//! its APIs are when a provider file names none. [`Issuer::data`] is the one way in; the code that
//! signs an account in reads it and holds none of it.

use super::auth::{Issuer, IssuerEndpoints};
use porter_capability::{Ask, CapabilityKind};
use porter_core::{EndpointUrl, Family};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// One thing a sign-in can ask, in the order it is asked (see `issuers.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeRow {
    /// What is asked.
    pub scopes: Vec<String>,
    /// The capability kind it serves; a row with none is asked every time.
    #[serde(default)]
    pub kind: Option<CapabilityKind>,
    /// The API it is for, when a kind reaches more than one.
    #[serde(default)]
    pub family: Option<Family>,
    /// When it is asked: with the sign-in, or only by a sign-in for it alone.
    #[serde(default)]
    pub ask: Ask,
    /// Asked only of a client that is the person's own.
    #[serde(default)]
    pub byo_only: bool,
    /// Asked in a sign-in for one more access too, to show who answered.
    #[serde(default)]
    pub identity: bool,
    /// The scope whose token shows who the account is.
    #[serde(default)]
    pub reads_who: bool,
}

/// An API's base where the provider file names none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiRow {
    /// The API.
    pub family: Family,
    /// Where it is.
    pub url: EndpointUrl,
}

/// How an issuer treats a scope: what its consent screen and verification ask of the client. The
/// data lists the first two; every other scope is restricted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopeClass {
    /// No verification.
    NonSensitive,
    /// Needs the consent screen verified (a warning screen and a user cap until it is).
    Sensitive,
    /// Needs verification and a yearly third-party security assessment: only a person's own
    /// client in testing mode can use it.
    Restricted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Classes {
    #[serde(default)]
    non_sensitive: Vec<String>,
    #[serde(default)]
    sensitive: Vec<String>,
}

/// An issuer's row of `issuers.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuerData {
    authorize: EndpointUrl,
    token: EndpointUrl,
    #[serde(default)]
    revoke: Option<EndpointUrl>,
    /// OpenID Connect's userinfo endpoint, where the account's address is read, if the issuer
    /// publishes one.
    #[serde(default)]
    pub userinfo: Option<EndpointUrl>,
    /// The page where a person ends an app's access, for an issuer with no revoke endpoint.
    #[serde(default)]
    pub manual_revoke: Option<EndpointUrl>,
    /// The scope that names every permission already consented to.
    #[serde(default)]
    pub all_scope: Option<String>,
    #[serde(default)]
    scope: Vec<ScopeRow>,
    #[serde(default)]
    api: Vec<ApiRow>,
    #[serde(default)]
    class: Classes,
}

impl IssuerData {
    /// Where the issuer's pages and token endpoint are, and its revoke endpoint if it has one.
    pub fn endpoints(&self) -> IssuerEndpoints {
        IssuerEndpoints {
            authorize: self.authorize.clone(),
            token: self.token.clone(),
            revoke: self.revoke.clone(),
        }
    }

    /// Whether the issuer has a revoke endpoint (RFC 7009).
    pub fn has_revoke(&self) -> bool {
        self.revoke.is_some()
    }

    /// The scopes a sign-in asks for the capability `kinds`, in the order the issuer's data lists
    /// them: the rows asked every time, and each kind's own. A row for a person's own client
    /// (`byo`) only when the client is.
    pub fn sign_in_scopes(&self, kinds: &[CapabilityKind], byo: bool) -> Vec<String> {
        self.scope
            .iter()
            .filter(|row| row.ask == Ask::SignIn && (byo || !row.byo_only))
            .filter(|row| row.kind.is_none_or(|kind| kinds.contains(&kind)))
            .flat_map(|row| row.scopes.iter().cloned())
            .collect()
    }

    /// The scopes of a sign-in that asks for the on-use access of `kinds` alone: theirs, and then
    /// who the person is (granted at sign-in already), so the answer can be told to be theirs.
    pub fn extend_scopes(&self, kinds: &[CapabilityKind]) -> Vec<String> {
        let who = self.scope.iter().filter(|row| row.identity);
        self.on_use_scopes(kinds)
            .into_iter()
            .map(str::to_owned)
            .chain(who.flat_map(|row| row.scopes.iter().cloned()))
            .collect()
    }

    /// The scopes of the on-use rows of `kinds`.
    pub fn on_use_scopes(&self, kinds: &[CapabilityKind]) -> Vec<&str> {
        self.scope
            .iter()
            .filter(|row| row.ask == Ask::OnUse)
            .filter(|row| row.kind.is_some_and(|kind| kinds.contains(&kind)))
            .flat_map(|row| row.scopes.iter().map(String::as_str))
            .collect()
    }

    /// The rows of `kind` asked as `ask` says.
    pub fn rows_of(&self, kind: CapabilityKind, ask: Ask) -> impl Iterator<Item = &ScopeRow> {
        self.scope
            .iter()
            .filter(move |row| row.kind == Some(kind) && row.ask == ask)
    }

    /// The scopes a sign-in asks for `kind`, none for a kind the issuer has no service for.
    pub fn kind_scopes(&self, kind: CapabilityKind) -> Vec<&str> {
        self.rows_of(kind, Ask::SignIn)
            .flat_map(|row| row.scopes.iter().map(String::as_str))
            .collect()
    }

    /// The scopes a token for a grant of `kind`, used against the API of `family`, is refreshed
    /// with: the rows of the kind that are asked as `ask` says, and are for that API or for every
    /// one. Never the rows asked of everyone (who the person is). Empty for a kind the issuer has
    /// no service for.
    pub fn grant_scopes(&self, kind: CapabilityKind, family: Family, ask: Ask) -> Vec<&str> {
        self.rows_of(kind, ask)
            .filter(|row| row.family.is_none_or(|f| f == family))
            .flat_map(|row| row.scopes.iter().map(String::as_str))
            .collect()
    }

    /// The scopes only the API of `family` needs: the rows that name it.
    pub fn family_scopes(&self, family: Family) -> Vec<&str> {
        self.scope
            .iter()
            .filter(|row| row.family == Some(family))
            .flat_map(|row| row.scopes.iter().map(String::as_str))
            .collect()
    }

    /// The scope whose token shows who the account is, if one is singled out.
    pub fn who_scope(&self) -> Option<&str> {
        self.scope
            .iter()
            .find(|row| row.reads_who)
            .and_then(|row| row.scopes.first())
            .map(String::as_str)
    }

    /// Where the API of `family` is when the provider file names none.
    pub fn api(&self, family: Family) -> Option<EndpointUrl> {
        self.api
            .iter()
            .find(|row| row.family == family)
            .map(|row| row.url.clone())
    }

    /// How the issuer classes `scope`; a scope its data does not name is taken as the most
    /// demanding.
    pub fn class_of(&self, scope: &str) -> ScopeClass {
        let named = |list: &[String]| list.iter().any(|s| s == scope);
        if named(&self.class.non_sensitive) {
            ScopeClass::NonSensitive
        } else if named(&self.class.sensitive) {
            ScopeClass::Sensitive
        } else {
            ScopeClass::Restricted
        }
    }
}

const ISSUERS: &str = include_str!("../../issuers.toml");

impl Issuer {
    /// What this issuer publishes (`issuers.toml`).
    pub fn data(self) -> &'static IssuerData {
        static TABLE: OnceLock<BTreeMap<Issuer, IssuerData>> = OnceLock::new();
        TABLE
            .get_or_init(|| toml::from_str(ISSUERS).unwrap_or_else(|e| panic!("issuers.toml: {e}")))
            .get(&self)
            .unwrap_or_else(|| panic!("issuers.toml has no row for {self:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CapabilityKind as K;

    const ALL: [Issuer; 8] = [
        Issuer::Google,
        Issuer::Microsoft,
        Issuer::Dropbox,
        Issuer::Box,
        Issuer::Fastmail,
        Issuer::OpenRouter,
        Issuer::OpenAi,
        Issuer::Yahoo,
    ];

    #[test]
    fn every_issuer_has_a_row_and_the_rows_name_no_other_issuer() {
        let table: BTreeMap<Issuer, IssuerData> = toml::from_str(ISSUERS).expect("parses");
        assert_eq!(table.len(), ALL.len());
        for issuer in ALL {
            assert!(table.contains_key(&issuer), "{issuer:?}");
            let _ = issuer.data();
        }
    }

    #[test]
    fn a_row_may_not_say_a_scope_it_does_not_ask_belongs_to_a_kind_with_a_family_it_lacks() {
        for issuer in ALL {
            for row in &issuer.data().scope {
                assert!(!row.scopes.is_empty(), "{issuer:?}: a row asks nothing");
                if row.reads_who {
                    assert!(
                        row.identity,
                        "{issuer:?}: the scope that reads who is asked"
                    );
                }
                if row.kind.is_none() {
                    assert!(
                        row.ask == Ask::SignIn && row.family.is_none() && !row.byo_only,
                        "{issuer:?}: a row of no kind is asked of everyone, always"
                    );
                }
            }
        }
    }

    #[test]
    fn microsoft_asks_exchange_first_so_a_code_is_redeemed_for_it() {
        let data = Issuer::Microsoft.data();
        let asked = data.sign_in_scopes(&[K::Mail, K::Calendar], false);
        assert_eq!(
            asked,
            [
                "https://outlook.office.com/IMAP.AccessAsUser.All",
                "https://outlook.office.com/SMTP.Send",
                "https://graph.microsoft.com/User.Read",
                "https://graph.microsoft.com/Calendars.ReadWrite",
                "offline_access",
                "openid",
            ]
        );
        assert_eq!(
            data.sign_in_scopes(&[K::Storage, K::Notes], false),
            [
                "https://graph.microsoft.com/User.Read",
                "https://graph.microsoft.com/Notes.ReadWrite",
                "https://graph.microsoft.com/Files.ReadWrite.AppFolder",
                "offline_access",
                "openid",
            ]
        );
        assert_eq!(
            data.sign_in_scopes(&[], false),
            [
                "https://graph.microsoft.com/User.Read",
                "offline_access",
                "openid"
            ]
        );
        assert_eq!(
            data.who_scope(),
            Some("https://graph.microsoft.com/User.Read")
        );
    }

    #[test]
    fn the_whole_drive_is_asked_alone_and_only_an_on_use_grant_carries_it() {
        const WHOLE: &str = "https://graph.microsoft.com/Files.ReadWrite";
        let data = Issuer::Microsoft.data();
        assert!(
            !data
                .sign_in_scopes(&[K::Storage, K::Calendar], false)
                .contains(&WHOLE.to_owned()),
            "a sign-in never asks the whole drive"
        );
        assert_eq!(
            data.extend_scopes(&[K::Storage]),
            [
                WHOLE,
                "https://graph.microsoft.com/User.Read",
                "offline_access",
                "openid"
            ]
        );
        assert_eq!(
            data.grant_scopes(K::Storage, Family::Graph, Ask::OnUse),
            [WHOLE]
        );
        assert!(
            data.grant_scopes(K::Calendar, Family::Graph, Ask::OnUse)
                .is_empty()
        );
        assert!(
            data.grant_scopes(K::Mail, Family::Imap, Ask::OnUse)
                .is_empty()
        );
        let google = Issuer::Google.data();
        assert_eq!(
            google.grant_scopes(K::Storage, Family::GoogleDrive, Ask::OnUse),
            ["https://www.googleapis.com/auth/drive"]
        );
        assert!(
            !google
                .sign_in_scopes(
                    &[
                        K::Mail,
                        K::Calendar,
                        K::Contacts,
                        K::Tasks,
                        K::Storage,
                        K::Photos
                    ],
                    true
                )
                .contains(&"https://www.googleapis.com/auth/drive".to_owned())
        );
    }

    #[test]
    fn a_grant_is_refreshed_with_its_own_kinds_scopes_only() {
        const MAIL: &str = "https://mail.google.com/";
        let data = Issuer::Google.data();
        type Case = (CapabilityKind, Family, &'static [&'static str]);
        let cases: &[Case] = &[
            (
                K::Calendar,
                Family::GoogleCalendar,
                &["https://www.googleapis.com/auth/calendar"],
            ),
            (
                K::Contacts,
                Family::GooglePeople,
                &["https://www.googleapis.com/auth/contacts"],
            ),
            (
                K::Tasks,
                Family::GoogleTasks,
                &["https://www.googleapis.com/auth/tasks"],
            ),
            (
                K::Storage,
                Family::GoogleDrive,
                &["https://www.googleapis.com/auth/drive.appdata"],
            ),
            (
                K::Photos,
                Family::GooglePhotosUpload,
                &["https://www.googleapis.com/auth/photoslibrary.appendonly"],
            ),
            (
                K::Photos,
                Family::GooglePhotosPicker,
                &["https://www.googleapis.com/auth/photospicker.mediaitems.readonly"],
            ),
            (K::Mail, Family::Imap, &[MAIL]),
            (K::Mail, Family::Smtp, &[MAIL]),
            (K::Notes, Family::GoogleDrive, &[]),
        ];
        for (kind, family, want) in cases {
            let got = data.grant_scopes(*kind, *family, Ask::SignIn);
            assert_eq!(got, *want, "{kind:?} {family:?}");
            let who = data.sign_in_scopes(&[], false);
            assert!(
                got.iter().all(|s| !who.iter().any(|w| w == s)),
                "{kind:?}: no identity scope in an app's token"
            );
        }
    }

    #[test]
    fn google_asks_mail_of_a_persons_own_client_only_and_classes_every_scope() {
        let data = Issuer::Google.data();
        let identity = data.sign_in_scopes(&[], false);
        assert_eq!(identity.len(), 3, "who the person is is always asked");
        assert_eq!(data.sign_in_scopes(&[K::Mail], false), identity);
        assert_eq!(
            data.sign_in_scopes(&[K::Mail], true),
            identity
                .iter()
                .cloned()
                .chain(["https://mail.google.com/".to_owned()])
                .collect::<Vec<_>>()
        );
        assert_eq!(
            data.sign_in_scopes(&[K::Agent, K::Notes], true),
            identity,
            "kinds the issuer has no service for ask nothing"
        );
        let class = |s: &str| data.class_of(s);
        assert_eq!(class("openid"), ScopeClass::NonSensitive);
        assert_eq!(
            class("https://www.googleapis.com/auth/calendar"),
            ScopeClass::Sensitive
        );
        assert_eq!(class("https://mail.google.com/"), ScopeClass::Restricted);
        assert_eq!(class("something-new"), ScopeClass::Restricted);
        let restricted: Vec<&str> = [
            K::Mail,
            K::Calendar,
            K::Contacts,
            K::Tasks,
            K::Storage,
            K::Photos,
        ]
        .into_iter()
        .flat_map(|k| data.kind_scopes(k))
        .filter(|s| class(s) == ScopeClass::Restricted)
        .collect();
        assert_eq!(
            restricted,
            ["https://mail.google.com/"],
            "of what sign-in asks, only mail is restricted"
        );
    }
}
