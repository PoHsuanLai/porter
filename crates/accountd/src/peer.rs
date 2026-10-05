//! `org.quire.Accounts1.Peer`: what inferd and syncd ask on behalf of an app (porter PLAN G3).
//! Only a connection whose caller role is `PorterDaemon` may call; every other sender is
//! `AccessDenied`. The app is named by the daemon from its own connection, never by the app.
//!
//! `Verdicts` is served. `ResolveKey` (a sealed memfd of an API key) and `ReportLocal` (a probed
//! runtime becoming an account) belong to the cloud-AI and local-runtime lanes and answer
//! `NotSupported` until they land (FINDINGS).

use crate::callers::Callers;
use crate::core::{Core, Host, slug};
use crate::errors::RefusedError;
use porter_core::consent::{GrantKey, Verdict, decide};
use porter_core::{AppId, AppName, Isolation, Match, Offer, SpaceScope, matches};
use porter_dbus::{AppArg, CallerRole, Details, NeedArg, VerdictArg, need_from_dbus};
use std::sync::Arc;
use zbus::fdo;
use zbus::message::Header;
use zbus::zvariant::{OwnedFd, OwnedValue, Value};

/// The peer object at the accountd path.
#[derive(Debug)]
pub(crate) struct Peer<H, C>(Arc<Core<H, C>>);

impl<H, C> Peer<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// The app a daemon names: its reverse-DNS name and isolation slug.
fn app_of(arg: AppArg) -> Result<AppId, RefusedError> {
    let (name, isolation) = arg;
    Ok(AppId {
        name: AppName::parse(&name).map_err(RefusedError::invalid)?,
        isolation: slug::<Isolation>(&isolation)?,
    })
}

fn text(value: &str) -> Option<OwnedValue> {
    OwnedValue::try_from(Value::from(value.to_owned())).ok()
}

#[zbus::interface(name = "org.quire.Accounts1.Peer")]
impl<H: Host, C: Callers> Peer<H, C> {
    async fn verdicts(
        &self,
        #[zbus(header)] header: Header<'_>,
        app: AppArg,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> Result<Vec<VerdictArg>, RefusedError> {
        let caller = self.0.identify(&header, crate::core::Standing::Any).await?;
        if caller.role != CallerRole::PorterDaemon {
            return Err(RefusedError::access_denied(
                "only a porter daemon may ask the peer interface",
            ));
        }
        let app = app_of(app)?;
        let need = need_from_dbus(need).map_err(RefusedError::invalid)?;
        let (class, usage) = (slug(&class)?, slug(&usage)?);
        let registry = self.0.host.registry();
        Ok(registry
            .accounts
            .iter()
            .filter_map(|account| {
                let claim = account.capabilities.iter().find(|claim| {
                    matches!(claim.offer, Offer::Present(_))
                        && matches(&need, &claim.offer) == Match::Fits
                })?;
                let key = GrantKey {
                    app: app.clone(),
                    account: account.id.clone(),
                    kind: claim.offer.kind(),
                    class,
                    usage,
                    space: SpaceScope::Any,
                };
                let mut details = Details::new();
                let word = match decide(&registry.grants, &key) {
                    Verdict::Granted { grant, scope } => {
                        details.extend(text(grant.as_str()).map(|v| ("grant".to_owned(), v)));
                        let scope = serde_json::to_value(scope).ok()?;
                        details.extend(text(scope.as_str()?).map(|v| ("scope".to_owned(), v)));
                        "granted"
                    }
                    Verdict::Denied => "denied",
                    Verdict::Ask => "ask",
                };
                Some((account.id.to_string(), word.to_owned(), details))
            })
            .collect())
    }

    async fn resolve_key(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
    ) -> fdo::Result<OwnedFd> {
        let _ = (header, grant);
        Err(fdo::Error::NotSupported(
            "ResolveKey waits for the cloud AI lane".into(),
        ))
    }

    async fn report_local(
        &self,
        #[zbus(header)] header: Header<'_>,
        provider: String,
        claims: Vec<(String, Details)>,
        state: String,
    ) -> fdo::Result<String> {
        let _ = (header, provider, claims, state);
        Err(fdo::Error::NotSupported(
            "ReportLocal waits for the local runtime lane".into(),
        ))
    }
}
