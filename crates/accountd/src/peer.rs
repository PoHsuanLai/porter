//! `org.quire.Accounts1.Peer`: what inferd and syncd ask on behalf of an app (porter PLAN G3).
//! Only a connection whose caller role is `PorterDaemon` may call; every other sender is
//! `AccessDenied`. The app is named by the daemon from its own connection, never by the app.
//!
//! `Verdicts` and `ResolveKey` (a sealed memfd of an API key) are served. `ReportLocal` (a probed
//! runtime becoming an account) belongs to the local-runtime lane and answers `NotSupported`
//! until it lands (FINDINGS).

use crate::callers::Callers;
use crate::core::{Core, Host, slug};
use crate::errors::RefusedError;
use crate::keys::sealed_key;
use porter_core::consent::{Decision, GrantKey, Verdict, decide};
use porter_core::wire::Refusal;
use porter_core::{AccountState, CapabilityKind, GrantId, Toggle};
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
                // The provider file the account was made from: inferd reaches a hosted model
                // through it, so it never guesses from the account id.
                details.extend(text(account.provider.as_str()).map(|v| ("provider".to_owned(), v)));
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

    /// The API key of a granted Llm account, on a sealed memfd and never a string on the bus.
    /// The grant is named by the daemon from the verdict it asked for: it must be an `Allow`, for
    /// the Llm kind (`AudienceNotGranted` otherwise), of an account that is not turned off for
    /// Llm and is not waiting to be signed in again. The release is audited, without the key.
    async fn resolve_key(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
    ) -> Result<OwnedFd, RefusedError> {
        let caller = self.0.identify(&header, crate::core::Standing::Any).await?;
        if caller.role != CallerRole::PorterDaemon {
            return Err(RefusedError::access_denied(
                "only a porter daemon may ask the peer interface",
            ));
        }
        let grant = GrantId::parse(&grant).map_err(RefusedError::invalid)?;
        let desk = self
            .0
            .keys
            .as_ref()
            .ok_or_else(|| RefusedError::of(Refusal::Unavailable))?;
        let registry = self.0.host.registry();
        let held = registry
            .grants
            .iter()
            .find(|g| g.id == grant && g.decision == Decision::Allow)
            .ok_or_else(|| RefusedError::of(Refusal::UnknownGrant))?;
        if held.key.kind != CapabilityKind::Llm {
            return Err(RefusedError::of(Refusal::AudienceNotGranted));
        }
        let account = registry
            .accounts
            .iter()
            .find(|a| a.id == held.key.account)
            .ok_or_else(|| RefusedError::of(Refusal::UnknownGrant))?;
        let off = registry.toggles.iter().any(|t| {
            t.account == account.id && t.kind == CapabilityKind::Llm && t.toggle == Toggle::Off
        });
        if off {
            return Err(RefusedError::of(Refusal::Denied));
        }
        if account.state == AccountState::NeedsReauth {
            return Err(RefusedError::of(Refusal::NeedsReauth));
        }
        let key = desk.read(&account.id).await.map_err(RefusedError::of)?;
        let fd = sealed_key(key.expose()).map_err(|_| RefusedError::of(Refusal::Unavailable))?;
        desk.note(&caller.app, &account.id, &grant);
        Ok(OwnedFd::from(fd))
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
