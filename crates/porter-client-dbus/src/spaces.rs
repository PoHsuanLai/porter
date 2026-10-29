//! `Spaces1` over the bus.

use crate::Bus;
use crate::failure::{link_error, spaces_error};
use futures_util::StreamExt;
use porter_client::{Changes, LinkError, Malformed, SpacesError, SpacesLink};
use porter_core::{DesktopSpace, DesktopSpaceRecord, SpaceChange, SpaceLook, SpaceName};
use porter_dbus::Details;
use porter_dbus::accounts::{
    SPACE_KEY_CREATED, SPACE_KEY_LOOK, SPACE_KEY_NAME, SpaceChanged, SpacesProxy,
};
use porter_ids::UnixSeconds;
use porter_ids::word::from_word;

impl Bus {
    async fn spaces_proxy(&self) -> Result<SpacesProxy<'static>, SpacesError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| SpacesError::Link(link_error(&e)))
    }
}

fn malformed(what: Malformed) -> SpacesError {
    SpacesError::Link(LinkError::Malformed(what))
}

fn text(details: &Details, key: &str) -> Result<String, SpacesError> {
    details
        .get(key)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| String::try_from(v).ok())
        .ok_or_else(|| malformed(Malformed::Row))
}

fn record(id: &str, details: &Details) -> Result<DesktopSpaceRecord, SpacesError> {
    let created = details
        .get(SPACE_KEY_CREATED)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| malformed(Malformed::Row))?;
    Ok(DesktopSpaceRecord {
        id: DesktopSpace::parse(id).map_err(|_| malformed(Malformed::Id))?,
        name: SpaceName::parse(&text(details, SPACE_KEY_NAME)?)
            .map_err(|_| malformed(Malformed::Row))?,
        look: SpaceLook::parse(&text(details, SPACE_KEY_LOOK)?)
            .map_err(|_| malformed(Malformed::Row))?,
        created: UnixSeconds(created),
    })
}

fn change(signal: SpaceChanged) -> Result<(DesktopSpace, SpaceChange), Malformed> {
    let args = signal.args().map_err(|_| Malformed::Signal)?;
    let what = from_word::<SpaceChange>(args.what).ok_or(Malformed::Signal)?;
    Ok((
        DesktopSpace::parse(args.id).map_err(|_| Malformed::Id)?,
        what,
    ))
}

impl SpacesLink for Bus {
    async fn list(&self) -> Result<Vec<DesktopSpaceRecord>, SpacesError> {
        self.spaces_proxy()
            .await?
            .list()
            .await
            .map_err(|e| spaces_error(&e))?
            .iter()
            .map(|(id, details)| record(id, details))
            .collect()
    }

    async fn create(
        &self,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> Result<DesktopSpace, SpacesError> {
        let id = self
            .spaces_proxy()
            .await?
            .create(name.as_str(), look.as_str())
            .await
            .map_err(|e| spaces_error(&e))?;
        DesktopSpace::parse(&id).map_err(|_| malformed(Malformed::Id))
    }

    async fn register(
        &self,
        id: &DesktopSpace,
        name: &SpaceName,
        look: &SpaceLook,
    ) -> Result<(), SpacesError> {
        self.spaces_proxy()
            .await?
            .register(id.as_str(), name.as_str(), look.as_str())
            .await
            .map_err(|e| spaces_error(&e))
    }

    async fn rename(&self, id: &DesktopSpace, name: &SpaceName) -> Result<(), SpacesError> {
        self.spaces_proxy()
            .await?
            .rename(id.as_str(), name.as_str())
            .await
            .map_err(|e| spaces_error(&e))
    }

    async fn set_look(&self, id: &DesktopSpace, look: &SpaceLook) -> Result<(), SpacesError> {
        self.spaces_proxy()
            .await?
            .set_look(id.as_str(), look.as_str())
            .await
            .map_err(|e| spaces_error(&e))
    }

    async fn remove(&self, id: &DesktopSpace) -> Result<(), SpacesError> {
        self.spaces_proxy()
            .await?
            .remove(id.as_str())
            .await
            .map_err(|e| spaces_error(&e))
    }

    async fn watch(&self) -> Result<Changes<(DesktopSpace, SpaceChange)>, SpacesError> {
        let proxy = self.spaces_proxy().await?;
        let stream = proxy
            .receive_changed()
            .await
            .map_err(|e| spaces_error(&e))?;
        // Reading the list once is how accountd counts this connection among those it tells.
        proxy.list().await.map_err(|e| spaces_error(&e))?;
        Ok(Changes::new(stream.map(change)))
    }
}
