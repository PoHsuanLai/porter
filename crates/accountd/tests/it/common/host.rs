//! A sheet host written by hand: the `org.quire.AccountsSheet1` interface on the private bus,
//! recording what accountd opened, updated and closed, and answering from a script.

use porter_core::sheet::SheetInput;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use zbus::Connection;
use zbus::object_server::SignalEmitter;

/// What the host was asked, shared with the test.
#[derive(Debug, Clone, Default)]
pub struct HostLog(Arc<Mutex<Calls>>);

#[derive(Debug, Default)]
pub struct Calls {
    pub opened: Vec<(String, String, String)>,
    pub updated: Vec<(String, String)>,
    pub closed: Vec<String>,
}

impl HostLog {
    pub fn calls(&self) -> MutexGuard<'_, Calls> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// What the host answers after an `Open`.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Nothing: the test sends `Input` itself.
    Nothing,
    /// This input at once.
    With(SheetInput),
}

#[derive(Debug)]
pub struct SheetHost {
    log: HostLog,
    reply: Reply,
    /// How long `Open` takes to return once the sheet is shown (logged).
    open_returns_after: Duration,
}

impl SheetHost {
    pub fn quiet() -> Self {
        Self {
            log: HostLog::default(),
            reply: Reply::Nothing,
            open_returns_after: Duration::ZERO,
        }
    }

    pub fn answering(input: SheetInput) -> Self {
        Self {
            reply: Reply::With(input),
            ..Self::quiet()
        }
    }

    /// The same host, whose `Open` returns `after` this long once the sheet is shown: the window
    /// in which accountd has sent `Open` and the person already sees the sheet.
    pub fn slow_to_return(self, after: Duration) -> Self {
        Self {
            open_returns_after: after,
            ..self
        }
    }

    pub fn log(&self) -> HostLog {
        self.log.clone()
    }
}

/// Sends an `Input` signal from `connection` as the host would.
pub async fn send_input(connection: &Connection, handle: &str, input: &SheetInput) {
    let emitter = SignalEmitter::new(connection, porter_dbus::SHEET_PATH).expect("emitter");
    let json = serde_json::to_string(input).expect("json");
    emitter
        .emit(
            "org.quire.AccountsSheet1",
            "Input",
            &(handle, json.as_str()),
        )
        .await
        .expect("emit");
}

#[zbus::interface(name = "org.quire.AccountsSheet1")]
impl SheetHost {
    async fn open(
        &self,
        handle: String,
        parent_window: String,
        view: String,
        #[zbus(connection)] connection: &Connection,
    ) {
        self.log
            .calls()
            .opened
            .push((handle.clone(), parent_window, view));
        if let Reply::With(input) = self.reply.clone() {
            let connection = connection.clone();
            tokio::spawn(async move { send_input(&connection, &handle, &input).await });
        }
        tokio::time::sleep(self.open_returns_after).await;
    }

    async fn update(&self, handle: String, view: String) {
        self.log.calls().updated.push((handle, view));
    }

    async fn close(&self, handle: String) {
        self.log.calls().closed.push(handle);
    }

    #[zbus(signal)]
    async fn input(emitter: &SignalEmitter<'_>, handle: &str, input: &str) -> zbus::Result<()>;
}
