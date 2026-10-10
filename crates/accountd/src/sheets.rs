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

use crate::app_names::AppNames;
use crate::callers::Callers;
use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::sheet::{ReviewView, SheetInput, SheetView};
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
    names: AppNames,
}

impl<C: Callers> BusSheets<C> {
    /// Sheets on `connection`, the host checked through `callers`. Apps are named by their ids
    /// alone until [`BusSheets::with_names`] says better.
    pub fn new(connection: Connection, callers: Arc<C>) -> Self {
        Self {
            connection,
            callers,
            next: AtomicU64::new(0),
            names: AppNames::default(),
        }
    }

    /// The same sheets naming the apps they show from `names` (the ones Settings reads): the
    /// consent sheet and the review's "Add, and allow ..." carry the resolved name beside the
    /// app id, so the host does not guess one from the id.
    #[must_use]
    pub fn with_names(self, names: AppNames) -> Self {
        Self { names, ..self }
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
        let view = json(&labelled(open.view, &self.names))?;
        // Taken down if this is dropped from here on, `Open`'s answer come or not: the host shows
        // the sheet once it has the call, and a caller that closes its Request then (the flow is
        // dropped while `Open` is on its way back) must not leave it up.
        let shown = Shown { proxy, handle };
        shown
            .proxy
            .open(&shown.handle, window, &view)
            .await
            .map_err(|_| SheetFault::Closed)?;
        Ok(BusLink {
            shown,
            owner,
            inputs,
            leaving,
            names: self.names.clone(),
        })
    }
}

fn json(view: &SheetView) -> Result<String, SheetFault> {
    serde_json::to_string(view).map_err(|_| SheetFault::Unavailable)
}

/// `view` with the names of the apps it shows beside their ids: the consent ask's app, and the
/// app a review adds the account for. An app `names` has no name for stays without one.
pub(crate) fn labelled(view: SheetView, names: &AppNames) -> SheetView {
    match view {
        SheetView::Consent(mut ask) => {
            ask.app_label = names.label_of(&ask.app.name);
            SheetView::Consent(ask)
        }
        SheetView::Review(review) => SheetView::Review(ReviewView {
            allow_label: review
                .allow
                .as_ref()
                .and_then(|app| names.label_of(&app.name)),
            ..review
        }),
        other => other,
    }
}

/// One open handle on the host. Dropping it takes the sheet down.
#[derive(Debug)]
pub struct BusLink {
    shown: Shown,
    owner: UniqueName<'static>,
    inputs: zbus::MessageStream,
    leaving: NameOwnerChangedStream,
    names: AppNames,
}

/// A handle the host may be showing, from the moment `Open` is sent: dropping it tells the host
/// to take the sheet down. After an `Open` the host refused that `Close` names a handle it may not
/// have; its answer is not read.
#[derive(Debug)]
struct Shown {
    proxy: AccountsSheetProxy<'static>,
    handle: String,
}

impl Drop for Shown {
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
        self.shown
            .proxy
            .update(&self.shown.handle, &json(&labelled(view, &self.names))?)
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
                    if !from_owner || handle != self.shown.handle {
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

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::consent::{AccountChoice, Usage};
    use porter_core::{
        AccountLabel, AppId, AppLabel, AppName, CapabilityKind, DataClass, Isolation, ProviderId,
    };
    use porter_dbus::{AppTitle, CallerRole, CallerRow, CallerTable};

    fn app(name: &str) -> AppId {
        AppId {
            name: AppName::parse(name).expect("name"),
            isolation: Isolation::Flatpak,
        }
    }

    fn ask(app: AppId) -> ConsentAsk {
        ConsentAsk::new(
            app,
            CapabilityKind::Llm,
            DataClass::Prompt,
            Usage::Interactive,
            vec![AccountChoice::new(
                porter_core::AccountId::parse("anthropic").expect("id"),
                AccountLabel("Anthropic".into()),
                ProviderId::parse("anthropic").expect("id"),
            )],
        )
    }

    fn names() -> AppNames {
        let table = CallerTable {
            callers: vec![CallerRow {
                app: AppName::parse("org.quire.Mail").expect("name"),
                unit: None,
                role: CallerRole::App,
                name: Some(AppTitle("Mail".to_owned())),
            }],
        };
        AppNames::new(table, Vec::new()).with_agents(&porter_provider::shipped_specs())
    }

    fn label_of(view: SheetView) -> Option<AppLabel> {
        match labelled(view, &names()) {
            SheetView::Consent(ask) => ask.app_label,
            other => panic!("a consent: {other:?}"),
        }
    }

    #[test]
    fn a_consent_for_an_agent_app_carries_the_agents_provider_label() {
        let view = SheetView::Consent(ask(app("org.quire.Agent.claude-code")));
        assert_eq!(label_of(view), Some(AppLabel("Claude Code".into())));
    }

    #[test]
    fn a_consent_for_a_plain_app_carries_its_table_name_and_an_unnamed_app_none() {
        assert_eq!(
            label_of(SheetView::Consent(ask(app("org.quire.Mail")))),
            Some(AppLabel("Mail".into()))
        );
        assert_eq!(
            label_of(SheetView::Consent(ask(app("org.example.Ghost")))),
            None,
            "the id is not a name: the host keeps its own guess"
        );
        // An ask without a label leaves it out of the JSON the host reads.
        let json = serde_json::to_string(&SheetView::Consent(ask(app("org.example.Ghost"))))
            .expect("json");
        assert!(!json.contains("app_label"), "{json}");
    }

    #[test]
    fn a_review_that_adds_and_allows_an_app_carries_that_apps_name() {
        let review = |allow: Option<AppId>| ReviewView {
            provider: ProviderId::parse("anthropic").expect("id"),
            row: None,
            review: porter_core::sheet::Review {
                label: AccountLabel("x".into()),
                services: Vec::new(),
                endpoints: Vec::new(),
            },
            allow,
            allow_label: None,
        };
        let names = names();
        let SheetView::Review(with) = labelled(
            SheetView::Review(review(Some(app("org.quire.Agent.claude-code")))),
            &names,
        ) else {
            panic!("a review");
        };
        assert_eq!(with.allow_label, Some(AppLabel("Claude Code".into())));
        let SheetView::Review(without) = labelled(SheetView::Review(review(None)), &names) else {
            panic!("a review");
        };
        assert_eq!(without.allow_label, None);
    }
}
