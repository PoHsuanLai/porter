//! The real accountd front end (`accountd::serve`) on a private bus, over the real
//! `AccountService` with the fake providers and a scripted consent sheet. Clients are
//! connections introduced to it by unique name, as a host that knows its clients would.

use super::bus::PrivateBus;
use accountd::TableCallers;
use porter_core::{AppId, AppName, Isolation};
use porter_fake::{AskLog, FakeService, Scripted, ScriptedSheets, fake_service};
use std::sync::Arc;

/// The app every test client is, unless it says otherwise.
pub fn photos() -> AppId {
    named("org.quire.Photos")
}

/// A Flatpak app of this name.
pub fn named(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

/// accountd, serving.
#[derive(Debug)]
pub struct Daemon {
    pub bus: PrivateBus,
    /// The daemon's own connection (it owns the name).
    pub connection: zbus::Connection,
    pub callers: Arc<TableCallers>,
    pub service: Arc<FakeService>,
    pub asked: AskLog,
}

impl Daemon {
    /// Starts accountd on a fresh private bus; the consent sheet answers `script`.
    pub async fn start(script: impl IntoIterator<Item = Scripted>) -> Self {
        let bus = PrivateBus::start();
        let prompter = ScriptedSheets::answering(script);
        let asked = prompter.log();
        let service = Arc::new(fake_service(prompter).await);
        let callers = Arc::new(TableCallers::new());
        let connection = bus.connect().await;
        accountd::serve(&connection, Arc::clone(&service), Arc::clone(&callers))
            .await
            .expect("accountd serves");
        Self {
            bus,
            connection,
            callers,
            service,
            asked,
        }
    }

    /// A new connection to the bus that accountd knows as `app`.
    pub async fn client_as(&self, app: AppId) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let name = connection.unique_name().expect("unique name").to_string();
        self.callers.introduce(&name, app);
        connection
    }

    /// A new connection to the bus that accountd knows as Photos.
    pub async fn client(&self) -> zbus::Connection {
        self.client_as(photos()).await
    }

    /// A new connection to the bus that accountd does not know.
    pub async fn stranger(&self) -> zbus::Connection {
        self.bus.connect().await
    }
}
