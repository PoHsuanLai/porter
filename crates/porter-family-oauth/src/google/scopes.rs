//! What a Google token answer says was granted. Google lets a person untick some of what was
//! asked (granular consent), and says so in the token answer; an answer that does not say leaves
//! it unknown and everything sign-in asks is taken as granted (nothing asked on use is). The
//! scopes themselves, and which capability kind each serves, are Google's row of `issuers.toml`.

use porter_capability::{Albums, Ask, Capability, CapabilityKind, LibraryRead};
use porter_core::{Family, GrantedScopes};
use porter_provider::Issuer;

/// What a token's granted scopes say.
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

    /// Whether the scopes `kind` needs were granted: one of its rows is held whole (either one
    /// of Photos' two is enough: the picker alone still imports, an upload alone still uploads).
    pub(super) fn covers(&self, kind: CapabilityKind) -> bool {
        let Some(granted) = &self.0 else {
            return true;
        };
        let held = |scope: &String| granted.iter().any(|g| g == scope);
        let mut rows = Issuer::Google.data().rows_of(kind, Ask::SignIn).peekable();
        rows.peek().is_none() || rows.any(|row| row.scopes.iter().all(held))
    }

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
        self.holds_all(&Issuer::Google.data().family_scopes(family))
    }

    /// `cap` as far as the granted scopes reach: Photos without the append-only scope does not
    /// upload (nor make albums), without the picker scope reads nothing.
    pub(super) fn narrow(&self, cap: &Capability) -> Capability {
        match cap {
            Capability::Photos(photos) => {
                let mut photos = photos.clone();
                if !self.allows(Family::GooglePhotosUpload) {
                    photos.upload = false;
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

    const OPENID: &str = "openid";
    const CALENDAR: &str = "https://www.googleapis.com/auth/calendar";
    const TASKS: &str = "https://www.googleapis.com/auth/tasks";
    const DRIVE_APP_FOLDER: &str = "https://www.googleapis.com/auth/drive.appdata";
    const DRIVE: &str = "https://www.googleapis.com/auth/drive";
    const PHOTOS_UPLOAD: &str = "https://www.googleapis.com/auth/photoslibrary.appendonly";
    const PHOTOS_PICKER: &str = "https://www.googleapis.com/auth/photospicker.mediaitems.readonly";

    fn granted(scopes: &[&str]) -> Granted {
        Granted::of(scopes.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn what_a_grant_holds_decides_a_refresh_and_the_whole_drive_is_never_taken_unasked() {
        let some = granted(&[OPENID, CALENDAR, PHOTOS_PICKER]);
        assert!(some.holds_all(&[CALENDAR]));
        assert!(some.holds_all(&[PHOTOS_PICKER]));
        assert!(!some.holds_all(&[PHOTOS_UPLOAD]));
        assert!(!some.holds_all(&[CALENDAR, TASKS]));
        assert!(granted(&[]).holds_all(&[TASKS]), "unknown is not a refusal");

        let unknown = Granted::of(Vec::new());
        assert!(unknown.admits(&[DRIVE_APP_FOLDER], Ask::SignIn));
        assert!(!unknown.admits(&[DRIVE], Ask::OnUse));
        let without = granted(&[DRIVE_APP_FOLDER]);
        assert!(!without.admits(&[DRIVE], Ask::OnUse));
        let with = granted(&[DRIVE_APP_FOLDER, DRIVE]);
        assert!(with.admits(&[DRIVE], Ask::OnUse));
        assert_eq!(Granted::recorded(&with.record()), with);
        assert!(unknown.record().is_unknown());
    }

    #[test]
    fn granular_consent_drops_what_the_person_unticked() {
        let some = granted(&[OPENID, CALENDAR, PHOTOS_PICKER]);
        assert!(some.covers(K::Calendar));
        assert!(!some.covers(K::Tasks));
        assert!(some.covers(K::Photos), "the picker alone is still photos");
        assert!(!some.covers(K::Storage));
        assert!(
            Granted::of(Vec::new()).covers(K::Tasks),
            "an answer that does not say is not a refusal"
        );
    }
}
