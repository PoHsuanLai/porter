//! The builders of the families that live in their own crates: each serves a spec when its
//! sign-in (the `AuthKind`, the discovery, the issuer) is the family's own, and hands it back
//! otherwise.

use super::{Built, FamilyIo};
use porter_core::AuthKind;
use porter_family_generic::{DavLookup, FixedLookup, GenericProvider, Lookup, MailLookup};
use porter_family_nextcloud::NextcloudProvider;
use porter_family_oauth::{BrowserIssuer, Google, Microsoft, OAuthEnv, OAuthProvider, Standard};
use porter_http_reqwest::ReqwestHttp;
use porter_provider::{Discovery, Issuer, ProviderSpec};

/// Nextcloud, when `spec` signs in by Login Flow v2.
pub(super) fn nextcloud(spec: ProviderSpec, io: &FamilyIo) -> Built {
    match spec.auth.kind {
        AuthKind::LoginFlowV2 => {
            let sleep = porter_http_reqwest::TokioSleep;
            Ok(Box::new(NextcloudProvider::new(
                spec,
                io.http.clone(),
                sleep,
            )))
        }
        _ => Err(Box::new(spec)),
    }
}

/// The generic family, when `spec` signs in by a password or an app password. How the servers
/// are found is the lookup its discovery names: an address (autoconfig, or a JMAP server the
/// person types), a server's name (`.well-known`), or the file's own servers.
pub(super) fn generic(spec: ProviderSpec, io: &FamilyIo) -> Built {
    fn serving<L: Lookup>(spec: ProviderSpec, io: &FamilyIo) -> Built {
        let provider = GenericProvider::<L>::new(spec, io.http.clone(), io.dns.clone());
        Ok(Box::new(provider.with_providers(io.providers.clone())))
    }
    match (spec.auth.kind, &spec.discovery) {
        (
            AuthKind::Password | AuthKind::AppPassword,
            Discovery::Autoconfig | Discovery::JmapSession,
        ) => serving::<MailLookup>(spec, io),
        (AuthKind::Password | AuthKind::AppPassword, Discovery::WellKnown) => {
            serving::<DavLookup>(spec, io)
        }
        (AuthKind::Password | AuthKind::AppPassword, Discovery::Fixed) => {
            serving::<FixedLookup>(spec, io)
        }
        _ => Err(Box::new(spec)),
    }
}

/// OAuth, when `spec` signs in by PKCE at an issuer it has rules for (Google, Microsoft) or one
/// whose data says who answered ([`Standard`]).
pub(super) fn oauth(spec: ProviderSpec, io: &FamilyIo) -> Built {
    fn serving<R: BrowserIssuer>(rules: R, spec: ProviderSpec, io: &FamilyIo) -> Built {
        let clock = std::sync::Arc::clone(&io.clock);
        let env = OAuthEnv::<ReqwestHttp>::with_client_files(io.clients.clone(), clock);
        Ok(Box::new(OAuthProvider::with_env(rules, spec, env)))
    }
    match (spec.auth.kind, spec.auth.issuer) {
        (AuthKind::OAuthPkce, Some(Issuer::Google)) => serving(Google, spec, io),
        (AuthKind::OAuthPkce, Some(Issuer::Microsoft)) => serving(Microsoft, spec, io),
        (AuthKind::OAuthPkce, Some(issuer)) if Standard::serves(issuer) => {
            serving(Standard::of(issuer), spec, io)
        }
        _ => Err(Box::new(spec)),
    }
}
