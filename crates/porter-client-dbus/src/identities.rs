//! `Identities1` over the bus.

use crate::Bus;
use crate::failure::{identities_error, link_error};
use futures_util::StreamExt;
use porter_client::{Changes, IdentitiesError, IdentitiesLink, LinkError, Malformed, Picked};
use porter_core::SecretText;
use porter_core::{AppName, Identity, IdentityAddress, IdentityId, ProviderHint, ProviderId};
use porter_dbus::accounts::{IdentitiesProxy, IdentityRemoved, identity_from_dbus};
use porter_wire::ParentWindow;
use std::io::Read;

impl Bus {
    async fn identities_proxy(&self) -> Result<IdentitiesProxy<'static>, IdentitiesError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| IdentitiesError::Link(link_error(&e)))
    }
}

fn malformed(what: Malformed) -> IdentitiesError {
    IdentitiesError::Link(LinkError::Malformed(what))
}

fn removed(signal: IdentityRemoved) -> Result<IdentityId, Malformed> {
    let args = signal.args().map_err(|_| Malformed::Signal)?;
    IdentityId::parse(args.identity_id).map_err(|_| Malformed::Id)
}

impl IdentitiesLink for Bus {
    async fn list(&self) -> Result<Vec<Identity>, IdentitiesError> {
        let proxy = self.identities_proxy().await?;
        proxy
            .list()
            .await
            .map_err(|e| identities_error(&e))?
            .iter()
            .map(|row| identity_from_dbus(row).map_err(|_| malformed(Malformed::Row)))
            .collect()
    }

    async fn offer(
        &self,
        hint: &ProviderHint,
        window: &ParentWindow,
    ) -> Result<Picked, IdentitiesError> {
        let proxy = self.identities_proxy().await?;
        let word = match hint {
            ProviderHint::Provider(provider) => provider.as_str(),
            _ => "",
        };
        let parent = match window {
            ParentWindow::Handle(handle) => handle.as_str(),
            _ => "",
        };
        let (id, address, provider) = proxy
            .offer(word, parent)
            .await
            .map_err(|e| identities_error(&e))?;
        Ok(Picked::new(
            IdentityId::parse(&id).map_err(|_| malformed(Malformed::Id))?,
            IdentityAddress(address),
            ProviderId::parse(&provider).map_err(|_| malformed(Malformed::Id))?,
        ))
    }

    async fn take_password(&self, id: &IdentityId) -> Result<SecretText, IdentitiesError> {
        let proxy = self.identities_proxy().await?;
        let fd = proxy
            .take_password(id.as_str())
            .await
            .map_err(|e| identities_error(&e))?;
        let mut text = String::new();
        std::fs::File::from(std::os::fd::OwnedFd::from(fd))
            .read_to_string(&mut text)
            .map_err(|_| malformed(Malformed::Descriptor))?;
        Ok(SecretText::new(text))
    }

    async fn register_use(&self, id: &IdentityId) -> Result<(), IdentitiesError> {
        self.identities_proxy()
            .await?
            .register_use(id.as_str())
            .await
            .map_err(|e| identities_error(&e))
    }

    async fn uses(&self, id: &IdentityId) -> Result<Vec<AppName>, IdentitiesError> {
        self.identities_proxy()
            .await?
            .uses(id.as_str())
            .await
            .map_err(|e| identities_error(&e))?
            .iter()
            .map(|name| AppName::parse(name).map_err(|_| malformed(Malformed::Id)))
            .collect()
    }

    async fn watch_removals(&self) -> Result<Changes<IdentityId>, IdentitiesError> {
        let proxy = self.identities_proxy().await?;
        let stream = proxy
            .receive_identity_removed()
            .await
            .map_err(|e| identities_error(&e))?;
        // Reading the list once is how accountd counts this connection among those it tells.
        proxy.list().await.map_err(|e| identities_error(&e))?;
        Ok(Changes::new(stream.map(removed)))
    }
}
