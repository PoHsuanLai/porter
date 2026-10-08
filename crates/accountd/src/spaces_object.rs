//! `org.quire.Spaces1` at `/org/quire/Spaces1`: the desktop-wide Spaces (`spaces`), on accountd's
//! connection.
//!
//! - Any identified caller may `List` and `Create`; `Rename`, `SetLook` and `Remove` only the
//!   Settings and sheet-host roles (the Settings app and the shell). Anyone else, and a sender
//!   accountd does not know, is `AccessDenied`, as `Accounts1`'s roles are checked.
//! - `Changed(id, what)` goes to each connection accountd knows (one that has called it), one by
//!   one, as the manager's signals do; a client calls `List` once to be told.
//! - `Remove` ends every grant whose scope is that Space alone, each as a Settings revoke does
//!   (audited `revoked`, `GrantChanged` to its holder). An app whose Space linked to it learns by
//!   `Changed(id, "removed")` and falls back to its own Space: that is the app's choice, and
//!   porter moves or deletes no memory (almanac's and docket's).

use crate::callers::Callers;
use crate::core::{Core, Host, Standing};
use crate::errors::RefusedError;
use crate::spaces::{CREATES_PER_WINDOW, SpaceFault, unix_now};
use porter_core::audit::AuditEvent;
use porter_core::{DesktopSpace, SpaceChange, SpaceId, SpaceLook, SpaceName, SpaceScope};
use porter_dbus::{CallerRole, Details, SPACES_PATH};
use std::sync::Arc;
use std::time::Instant;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;

/// The Spaces object.
#[derive(Debug)]
pub(crate) struct SpacesObject<H, C>(Arc<Core<H, C>>);

impl<H, C> SpacesObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// What a refused change is told, in plain words.
fn fault(fault: SpaceFault) -> RefusedError {
    match fault {
        SpaceFault::NotThere => RefusedError::invalid("There is no such Space."),
        SpaceFault::TooMany => RefusedError::busy(format!(
            "Too many new Spaces at once: at most {CREATES_PER_WINDOW} a minute. Try again in a \
             minute."
        )),
        SpaceFault::Unsaved => RefusedError::failed("The Spaces could not be saved."),
    }
}

fn space_arg(id: &str) -> Result<DesktopSpace, RefusedError> {
    DesktopSpace::parse(id).map_err(|_| RefusedError::invalid("There is no such Space."))
}

fn name_arg(text: &str) -> Result<SpaceName, RefusedError> {
    SpaceName::parse(text).map_err(|_| {
        RefusedError::invalid("A Space's name must have 1 to 64 characters and no line breaks.")
    })
}

fn look_arg(text: &str) -> Result<SpaceLook, RefusedError> {
    SpaceLook::parse(text)
        .map_err(|_| RefusedError::invalid("A Space's look must be at most 1024 bytes."))
}

impl<H: Host, C: Callers> SpacesObject<H, C> {
    /// The caller, which must be Settings or the shell.
    async fn manager(&self, header: &Header<'_>) -> Result<porter_dbus::Caller, RefusedError> {
        let caller = self.0.identify(header, Standing::Any).await?;
        match caller.role {
            CallerRole::Settings | CallerRole::SheetHost => Ok(caller),
            _ => Err(RefusedError::access_denied(
                "only Settings and the shell may change a Space",
            )),
        }
    }

    /// Tells every connection accountd knows that `id` changed.
    async fn tell(&self, id: &DesktopSpace, change: SpaceChange) {
        for name in self.0.known() {
            let Ok(destination) = BusName::try_from(name) else {
                continue;
            };
            let Ok(emitter) = SignalEmitter::new(&self.0.connection, SPACES_PATH) else {
                continue;
            };
            let emitter = emitter.set_destination(destination);
            let _ = Self::changed(&emitter, id.as_str(), change.word()).await;
        }
    }
}

#[zbus::interface(name = "org.quire.Spaces1")]
impl<H: Host, C: Callers> SpacesObject<H, C> {
    async fn list(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<(String, Details)>, RefusedError> {
        self.0.identify(&header, Standing::Any).await?;
        let book = self.0.spaces.lock().await;
        Ok(book
            .list()
            .iter()
            .map(|record| {
                let fields = [
                    ("name", Value::from(record.name.as_str())),
                    ("look", Value::from(record.look.as_str())),
                    ("created", Value::from(record.created.0)),
                ];
                let details: Details = fields
                    .into_iter()
                    .filter_map(|(key, v)| Some((key.to_owned(), v.try_to_owned().ok()?)))
                    .collect();
                (record.id.to_string(), details)
            })
            .collect())
    }

    async fn create(
        &self,
        #[zbus(header)] header: Header<'_>,
        name: String,
        look: String,
    ) -> Result<String, RefusedError> {
        let caller = self.0.identify(&header, Standing::Any).await?;
        let (name, look) = (name_arg(&name)?, look_arg(&look)?);
        let made = self
            .0
            .spaces
            .lock()
            .await
            .create(&caller.app.name, name, look, (Instant::now(), unix_now()))
            .await
            .map_err(fault)?;
        self.0.host.audit_app(
            &caller.app,
            AuditEvent::SpaceCreated {
                space: made.clone(),
            },
        );
        self.tell(&made, SpaceChange::Created).await;
        Ok(made.to_string())
    }

    async fn rename(
        &self,
        #[zbus(header)] header: Header<'_>,
        id: String,
        name: String,
    ) -> Result<(), RefusedError> {
        let caller = self.manager(&header).await?;
        let (id, name) = (space_arg(&id)?, name_arg(&name)?);
        self.0
            .spaces
            .lock()
            .await
            .rename(&id, name)
            .await
            .map_err(fault)?;
        self.0
            .host
            .audit_app(&caller.app, AuditEvent::SpaceRenamed { space: id.clone() });
        self.tell(&id, SpaceChange::Renamed).await;
        Ok(())
    }

    async fn set_look(
        &self,
        #[zbus(header)] header: Header<'_>,
        id: String,
        look: String,
    ) -> Result<(), RefusedError> {
        self.manager(&header).await?;
        let (id, look) = (space_arg(&id)?, look_arg(&look)?);
        self.0
            .spaces
            .lock()
            .await
            .set_look(&id, look)
            .await
            .map_err(fault)?;
        self.tell(&id, SpaceChange::Look).await;
        Ok(())
    }

    async fn remove(
        &self,
        #[zbus(header)] header: Header<'_>,
        id: String,
    ) -> Result<(), RefusedError> {
        let caller = self.manager(&header).await?;
        let id = space_arg(&id)?;
        self.0
            .spaces
            .lock()
            .await
            .remove(&id)
            .await
            .map_err(fault)?;
        // Every grant scoped to this Space alone ends, as a revoke from Settings does.
        let scope = SpaceScope::Only(SpaceId::linked(&id));
        let ending: Vec<_> = self
            .0
            .host
            .registry()
            .grants
            .into_iter()
            .filter(|g| g.key.space == scope)
            .map(|g| g.id)
            .collect();
        for grant in &ending {
            let _ = self.0.host.revoke_grant(grant).await;
        }
        if !ending.is_empty() {
            self.0.publish().await;
        }
        self.0
            .host
            .audit_app(&caller.app, AuditEvent::SpaceRemoved { space: id.clone() });
        self.tell(&id, SpaceChange::Removed).await;
        Ok(())
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>, id: &str, what: &str) -> zbus::Result<()>;
}
