//! `org.quire.Identities1` at `/org/quire/Identities1`, on accountd's connection: the identities
//! signed in on this machine, so an app can say "Continue as ada@example.org" without holding
//! the sign-in itself. An identity is an account of a sign-in kind (OAuth, a password, an app
//! password); local runtimes, Tailscale, AI keys and agent logins are not identities. Nothing
//! here carries a token. The one secret that may leave is a password, on a descriptor, after the
//! person picked that identity in the sheet (`TakePassword`).
//!
//! Who may call: any identified app (and the porter daemons) may `List`, `Offer` and
//! `RegisterUse`; `TakePassword` the app whose `Offer` the person just answered with that
//! identity, once; `Uses` only Settings and the shell (`CallerRole::Settings`,
//! `CallerRole::SheetHost`). The agent, computer use, the agent launcher, the terminal and the
//! compositor are `AccessDenied`, as is a sender accountd does not know.

use crate::args::Details;
use crate::json_value::bad;
use porter_core::ProviderId;
use porter_core::{CoreError, Identity, IdentityAddress, IdentityId, IdentityKind, IdentityName};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedFd, OwnedValue, Value};

/// The vardict key of an identity's address (`s`).
pub const IDENTITY_KEY_ADDRESS: &str = "address";
/// The vardict key of an identity's provider file id (`s`).
pub const IDENTITY_KEY_PROVIDER: &str = "provider";
/// The vardict key of what a person calls the provider (`s`).
pub const IDENTITY_KEY_NAME: &str = "name";
/// The vardict key of how it signs in (`s`: `oauth` or `password`).
pub const IDENTITY_KEY_KIND: &str = "kind";

/// An identity as its id and a vardict of the rest (`(sa{sv})`).
pub fn identity_to_dbus(identity: &Identity) -> (String, Details) {
    let fields = [
        (
            IDENTITY_KEY_ADDRESS,
            Value::from(identity.address.0.clone()),
        ),
        (
            IDENTITY_KEY_PROVIDER,
            Value::from(identity.provider.as_str().to_owned()),
        ),
        (IDENTITY_KEY_NAME, Value::from(identity.name.0.clone())),
        (
            IDENTITY_KEY_KIND,
            Value::from(identity.kind.slug().to_owned()),
        ),
    ];
    let details = fields
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), OwnedValue::try_from(value).ok()?)))
        .collect();
    (identity.id.as_str().to_owned(), details)
}

fn text(details: &Details, key: &str) -> Option<String> {
    String::try_from(details.get(key)?.try_clone().ok()?).ok()
}

/// The identity a row carries, or why it is not one. A key a newer accountd adds is ignored; the
/// address, the provider and the kind must be there.
pub fn identity_from_dbus(row: &(String, Details)) -> Result<Identity, CoreError> {
    let (id, details) = row;
    let address = text(details, IDENTITY_KEY_ADDRESS).ok_or_else(|| bad("identity: no address"))?;
    let provider =
        text(details, IDENTITY_KEY_PROVIDER).ok_or_else(|| bad("identity: no provider"))?;
    let kind = text(details, IDENTITY_KEY_KIND)
        .and_then(|slug| IdentityKind::from_slug(&slug))
        .ok_or_else(|| bad("identity: no kind"))?;
    let provider = ProviderId::parse(&provider)?;
    // A row without a name for the provider shows the provider's id.
    let name = text(details, IDENTITY_KEY_NAME).unwrap_or_else(|| provider.as_str().to_owned());
    Ok(Identity::new(
        IdentityId::parse(id)?,
        IdentityAddress(address),
        provider,
        IdentityName(name),
        kind,
    ))
}

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Identities1",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Identities1"
)]
pub trait Identities {
    /// The identities signed in on this machine, in the order the registry holds them: the
    /// identity's id, then `address` (`s`), `provider` (`s`, the provider file's id), `name`
    /// (`s`, what a person calls the provider) and `kind` (`s`: `oauth` or `password`). Never a
    /// token or a secret.
    fn list(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// "Continue as": shows the person the identity picker through the sheet host, and answers
    /// the pick as the identity's id, its address and its provider's id. `provider_hint` is a
    /// provider file's id to list only that provider's identities, or empty for all;
    /// `parent_window` is the app's window the sheet sits over, as for `Choose` (empty for none).
    /// It never returns a token. The call waits for the person. Errors: `Dismissed` when the person
    /// closed the sheet, turned every identity down, or no sheet host is reachable (a client
    /// reads it as cancelled, as it reads a `Choose` that was closed);
    /// `NoFittingAccount` when no identity fits (offer to sign one in); `Unavailable` when this
    /// accountd has no way to show the picker; `LimitsExceeded` while this app has a sheet of
    /// this kind open; `InvalidArgs` for a hint that is not a provider id.
    fn offer(
        &self,
        provider_hint: &str,
        parent_window: &str,
    ) -> zbus::Result<(String, String, String)>;
    /// The password (or app password) of the identity the person just picked in this app's
    /// `Offer`, as a sealed file descriptor (`h`) holding the text, read position at the start.
    /// Once per pick, and only for a `password` identity; audited. Errors: `AccessDenied` for
    /// an identity this connection was not just offered, one that signs in with OAuth, or a
    /// second take; `InvalidArgs` for an identity that is not there; `Unavailable` when the
    /// secret store cannot be reached; `NeedsReauth` when no password is filed.
    fn take_password(&self, identity_id: &str) -> zbus::Result<OwnedFd>;
    /// The calling app says it signed in with this identity: it joins the list `Uses` gives.
    /// Saying it twice changes nothing. Errors: `InvalidArgs` for an identity that is not there.
    fn register_use(&self, identity_id: &str) -> zbus::Result<()>;
    /// The apps that said they signed in with this identity, as app ids (`as`), for Settings'
    /// "apps using this identity". Settings and the shell only. Errors: `InvalidArgs` for an
    /// identity that is not there.
    fn uses(&self, identity_id: &str) -> zbus::Result<Vec<String>>;
    /// An identity was removed (Settings' Remove). Sent to each allowed connection that has
    /// called accountd; an app decides what to do with its own copies, accountd reaches into
    /// nothing.
    #[zbus(signal)]
    fn identity_removed(&self, identity_id: &str) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct IdentitiesSkeleton;

#[zbus::interface(name = "org.quire.Identities1")]
impl IdentitiesSkeleton {
    fn list(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::frozen::error())
    }

    fn offer(
        &self,
        provider_hint: String,
        parent_window: String,
    ) -> fdo::Result<(String, String, String)> {
        let _ = (provider_hint, parent_window);
        Err(crate::frozen::error())
    }

    fn take_password(&self, identity_id: String) -> fdo::Result<OwnedFd> {
        let _ = identity_id;
        Err(crate::frozen::error())
    }

    fn register_use(&self, identity_id: String) -> fdo::Result<()> {
        let _ = identity_id;
        Err(crate::frozen::error())
    }

    fn uses(&self, identity_id: String) -> fdo::Result<Vec<String>> {
        let _ = identity_id;
        Err(crate::frozen::error())
    }

    #[zbus(signal)]
    async fn identity_removed(emitter: &SignalEmitter<'_>, identity_id: &str) -> zbus::Result<()>;
}

/// Writes this interface's introspection into `xml`, as an `<interface>` element one level in:
/// the bus's introspection collects each interface's own.
pub(crate) fn introspect(xml: &mut String) {
    zbus::object_server::Interface::introspect_to_writer(&IdentitiesSkeleton, xml, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity::new(
            IdentityId::parse("ada.example").expect("id"),
            IdentityAddress("ada@example.org".into()),
            ProviderId::parse("google").expect("provider"),
            IdentityName("Google".into()),
            IdentityKind::OAuth,
        )
    }

    #[test]
    fn an_identity_survives_the_bus_shape_and_carries_no_secret_field() {
        let row = identity_to_dbus(&identity());
        assert_eq!(row.0, "ada.example");
        let mut keys: Vec<_> = row.1.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["address", "kind", "name", "provider"]);
        assert_eq!(identity_from_dbus(&row), Ok(identity()));
    }

    #[test]
    fn a_row_missing_what_an_identity_needs_is_refused_and_a_missing_name_shows_the_provider() {
        let (id, details) = identity_to_dbus(&identity());
        for key in [
            IDENTITY_KEY_ADDRESS,
            IDENTITY_KEY_PROVIDER,
            IDENTITY_KEY_KIND,
        ] {
            let mut cut = details.clone();
            cut.remove(key);
            assert!(identity_from_dbus(&(id.clone(), cut)).is_err(), "{key}");
        }
        assert!(identity_from_dbus(&("Not An Id".to_owned(), details.clone())).is_err());
        let mut unnamed = details;
        unnamed.remove(IDENTITY_KEY_NAME);
        let read = identity_from_dbus(&(id, unnamed)).expect("identity");
        assert_eq!(read.name, IdentityName("google".into()));
    }
}
