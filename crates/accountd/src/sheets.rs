//! The sheets drawn by the sheet host (sill) over `org.quire.AccountsSheet1`.
//!
//! accountd is the only caller of `Open`, `Update` and `Close`; the host answers with `Input`
//! signals. Who counts:
//!
//! - the host is the connection that owns `org.quire.AccountsSheet1` **and** whose caller role is
//!   `SheetHost`: a process that merely claimed the name is refused (`Unavailable`), so it can
//!   neither see a sheet nor answer one;
//! - calls go to the owner's unique name, never the well-known one, so the owner cannot change
//!   between `Open` and `Update`;
//! - an `Input` counts only when it comes from that owner's connection and names this handle;
//!   any other (a forgery, another handle, malformed JSON) is ignored;
//! - the owner leaving the bus, or dropping the link, ends the sheet (`Closed`; the host is told
//!   to take it down).

use crate::callers::Callers;
use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::wire::ParentWindow;
use porter_dbus::{AccountsSheetProxy, CallerRole, SHEET_BUS};
use porter_service::{SheetFault, SheetLink, SheetOpen, Sheets};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::Connection;
use zbus::export::futures_core::Stream;
use zbus::fdo::{DBusProxy, NameOwnerChangedStream};
use zbus::names::{BusName, UniqueName};

/// The sheet host reached over the session bus.
#[derive(Debug)]
pub struct BusSheets<C> {
    connection: Connection,
    callers: Arc<C>,
    next: AtomicU64,
}

impl<C: Callers> BusSheets<C> {
    /// Sheets on `connection`, the host checked through `callers`.
    pub fn new(connection: Connection, callers: Arc<C>) -> Self {
        Self {
            connection,
            callers,
            next: AtomicU64::new(0),
        }
    }

    /// The unique name of the verified host.
    async fn host(&self) -> Result<UniqueName<'static>, SheetFault> {
        let bus = DBusProxy::new(&self.connection)
            .await
            .map_err(|_| SheetFault::Unavailable)?;
        let name = BusName::try_from(SHEET_BUS).map_err(|_| SheetFault::Unavailable)?;
        let owner = bus
            .get_name_owner(name)
            .await
            .map_err(|_| SheetFault::Unavailable)?
            .into_inner();
        match self.callers.caller_of(owner.as_str()).await {
            Some(caller) if caller.role == CallerRole::SheetHost => Ok(owner),
            _ => Err(SheetFault::Unavailable),
        }
    }

    async fn open_link(&self, open: SheetOpen) -> Result<BusLink, SheetFault> {
        let owner = self.host().await?;
        let proxy = AccountsSheetProxy::builder(&self.connection)
            .destination(BusName::from(owner.clone()))
            .map_err(|_| SheetFault::Unavailable)?
            .build()
            .await
            .map_err(|_| SheetFault::Unavailable)?;
        // Subscribed before `Open`, so an input sent at once is not missed.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(owner.clone())
            .and_then(|b| b.interface("org.quire.AccountsSheet1"))
            .and_then(|b| b.member("Input"))
            .map(|b| b.build())
            .map_err(|_| SheetFault::Unavailable)?;
        let inputs = zbus::MessageStream::for_match_rule(rule, &self.connection, None)
            .await
            .map_err(|_| SheetFault::Unavailable)?;
        let leaving = DBusProxy::new(&self.connection)
            .await
            .map_err(|_| SheetFault::Unavailable)?
            .receive_name_owner_changed_with_args(&[(0, owner.as_str())])
            .await
            .map_err(|_| SheetFault::Unavailable)?;
        let handle = format!("accountd-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let window = match &open.window {
            ParentWindow::Unparented => "",
            ParentWindow::Handle(handle) => handle.as_str(),
        };
        proxy
            .open(&handle, window, &json(&open.view)?)
            .await
            .map_err(|_| SheetFault::Closed)?;
        Ok(BusLink {
            proxy,
            handle,
            owner,
            inputs,
            leaving,
        })
    }
}

fn json(view: &SheetView) -> Result<String, SheetFault> {
    serde_json::to_string(view).map_err(|_| SheetFault::Unavailable)
}

/// One open handle on the host. Dropping it takes the sheet down.
#[derive(Debug)]
pub struct BusLink {
    proxy: AccountsSheetProxy<'static>,
    handle: String,
    owner: UniqueName<'static>,
    inputs: zbus::MessageStream,
    leaving: NameOwnerChangedStream,
}

impl Drop for BusLink {
    fn drop(&mut self) {
        let (proxy, handle) = (self.proxy.clone(), self.handle.clone());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = proxy.close(&handle).await;
            });
        }
    }
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}

/// Resolves when the host's connection leaves or changes owner.
async fn departed(owners: &mut NameOwnerChangedStream) {
    while let Some(signal) = next(owners).await {
        if signal
            .args()
            .map(|args| args.new_owner().is_none())
            .unwrap_or(true)
        {
            return;
        }
    }
}

impl SheetLink for BusLink {
    async fn update(&mut self, view: SheetView) -> Result<(), SheetFault> {
        self.proxy
            .update(&self.handle, &json(&view)?)
            .await
            .map_err(|_| SheetFault::Closed)
    }

    async fn input(&mut self) -> Result<SheetInput, SheetFault> {
        loop {
            tokio::select! {
                signal = next(&mut self.inputs) => {
                    let Some(Ok(message)) = signal else { return Err(SheetFault::Closed) };
                    let from_owner = message.header().sender() == Some(&self.owner);
                    let Ok((handle, input)) = message.body().deserialize::<(String, String)>() else {
                        continue;
                    };
                    if !from_owner || handle != self.handle {
                        continue;
                    }
                    if let Ok(input) = serde_json::from_str::<SheetInput>(&input) {
                        return Ok(input);
                    }
                }
                () = departed(&mut self.leaving) => return Err(SheetFault::Closed),
            }
        }
    }
}

impl<C: Callers> Sheets for BusSheets<C> {
    type Link = BusLink;

    async fn consent(&self, ask: ConsentAsk, window: &ParentWindow) -> ConsentAnswer {
        let open = SheetOpen {
            window: window.clone(),
            view: SheetView::Consent(ask),
        };
        let Ok(mut link) = self.open_link(open).await else {
            return ConsentAnswer::Dismissed;
        };
        loop {
            match link.input().await {
                Ok(SheetInput::Answer(answer)) => return answer,
                Ok(SheetInput::Dismiss) | Err(_) => return ConsentAnswer::Dismissed,
                Ok(_) => {}
            }
        }
    }

    async fn conversation(&self, open: SheetOpen) -> Result<BusLink, SheetFault> {
        self.open_link(open).await
    }
}
