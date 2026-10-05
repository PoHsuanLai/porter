//! `Manager.Adopt`: the `[adopt]` table (which app may adopt from which legacy service) and the
//! legacy store seam reading mailo's `service=mailo` entries (`<uuid>:incoming|outgoing|oauth|
//! carddav`, each a JSON credential in mailo's own form). Tests read fixtures through
//! [`MemoryLegacy`]; the daemon reads the Secret Service through [`Oo7Legacy`], read-only.

use porter_core::{AppName, Credential, SecretText, UnixSeconds};
use porter_service::{LegacyFault, LegacyStore};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// App name to the legacy service it may adopt from: the `[adopt]` table of the daemon's config
/// (`org.quire.Mail = "mailo"`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(transparent)]
pub struct AdoptTable(pub BTreeMap<AppName, String>);

impl AdoptTable {
    /// The table in a config file's text: its `[adopt]` section, none meaning an empty table.
    pub fn from_config(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            adopt: AdoptTable,
        }
        toml::from_str::<File>(text)
            .map(|f| f.adopt)
            .map_err(|e| e.to_string())
    }

    /// The legacy service `app` may read, if the table names it.
    pub fn service_of(&self, app: &AppName) -> Option<&str> {
        self.0.get(app).map(String::as_str)
    }
}

/// What `Adopt` needs: who may ask, and where the old items are. With no store, `Adopt` is
/// refused `Unavailable`.
#[derive(Default)]
pub struct AdoptConfig {
    /// Who may ask, for which service.
    pub table: AdoptTable,
    /// The old items.
    pub store: Option<Arc<dyn LegacyStore>>,
}

impl std::fmt::Debug for AdoptConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdoptConfig")
            .field("table", &self.table)
            .field("store", &self.store.as_ref().map(|_| "<store>"))
            .finish()
    }
}

/// mailo's stored credential, as its keyring held it (JSON, `kind`/`v`).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
enum Mailo {
    Password(String),
    #[serde(rename = "oauth")]
    OAuth {
        access: String,
        refresh: String,
        expires_at: String,
    },
}

/// Seconds since the epoch of an RFC 3339 instant (`2026-10-05T12:30:00Z`, or with a numeric
/// offset; fractions are ignored).
pub fn rfc3339(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let clock_end = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(clock_end);
    let mut c = clock.split(':');
    let h: i64 = c.next()?.parse().ok()?;
    let min: i64 = c.next()?.parse().ok()?;
    let sec: i64 = c.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    let offset = match zone.as_bytes().first() {
        Some(b'+' | b'-') => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (oh, om) = zone[1..].split_once(':')?;
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
        _ => 0,
    };
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + sec - offset)
}

/// mailo's stored text as the porter credential it becomes; `None` for text that is not a
/// password or OAuth credential (mailo also keeps PGP and S/MIME keys, which are not adopted).
pub fn from_mailo(text: &str) -> Option<Credential> {
    match serde_json::from_str::<Mailo>(text).ok()? {
        Mailo::Password(password) => Some(Credential::Password(SecretText::new(password))),
        Mailo::OAuth {
            access,
            refresh,
            expires_at,
        } => Some(Credential::OAuth {
            access: SecretText::new(access),
            refresh: SecretText::new(refresh),
            expires_at: UnixSeconds(rfc3339(&expires_at)?),
        }),
    }
}

/// Entries by (service, name) in memory: the fixture store.
#[derive(Debug, Clone, Default)]
pub struct MemoryLegacy(pub BTreeMap<(String, String), String>);

impl MemoryLegacy {
    /// Files `text` as `service`'s entry `name`.
    pub fn with(mut self, service: &str, name: &str, text: &str) -> Self {
        self.0
            .insert((service.to_owned(), name.to_owned()), text.to_owned());
        self
    }
}

impl LegacyStore for MemoryLegacy {
    fn read<'a>(
        &'a self,
        service: &'a str,
        entry: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Credential>, LegacyFault>> + Send + 'a>> {
        Box::pin(async move {
            match self.0.get(&(service.to_owned(), entry.to_owned())) {
                None => Ok(None),
                Some(text) => from_mailo(text).map(Some).ok_or(LegacyFault::Unavailable),
            }
        })
    }
}

/// The user's Secret Service, read-only: entries of the legacy service by `service` and
/// `username` attributes, which is how the platform keyring crate files an entry.
#[derive(Debug, Default, Clone, Copy)]
pub struct Oo7Legacy;

impl LegacyStore for Oo7Legacy {
    fn read<'a>(
        &'a self,
        service: &'a str,
        entry: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Credential>, LegacyFault>> + Send + 'a>> {
        Box::pin(async move {
            let keyring = oo7::Keyring::new()
                .await
                .map_err(|_| LegacyFault::Unavailable)?;
            let attributes = std::collections::HashMap::from([
                ("service", service.to_owned()),
                ("username", entry.to_owned()),
            ]);
            let items = keyring
                .search_items(&attributes)
                .await
                .map_err(|_| LegacyFault::Unavailable)?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            let secret = item.secret().await.map_err(|_| LegacyFault::Unavailable)?;
            let text =
                std::str::from_utf8(secret.as_bytes()).map_err(|_| LegacyFault::Unavailable)?;
            from_mailo(text).map(Some).ok_or(LegacyFault::Unavailable)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instants_parse_as_unix_seconds() {
        let cases = [
            ("1970-01-01T00:00:00Z", Some(0)),
            ("2026-10-05T12:30:00Z", Some(1_791_203_400)),
            ("2026-10-05T12:30:00.123456Z", Some(1_791_203_400)),
            ("2026-10-05T14:30:00+02:00", Some(1_791_203_400)),
            ("2024-02-29T00:00:00Z", Some(1_709_164_800)),
            ("garbage", None),
        ];
        for (text, want) in cases {
            assert_eq!(rfc3339(text), want, "{text}");
        }
    }

    #[test]
    fn mailo_credentials_read_and_others_do_not() {
        assert_eq!(
            from_mailo(r#"{"kind":"password","v":"pw"}"#),
            Some(Credential::Password(SecretText::new("pw")))
        );
        assert!(matches!(
            from_mailo(
                r#"{"kind":"oauth","v":{"access":"a","refresh":"r","expires_at":"2026-10-05T12:30:00Z"}}"#
            ),
            Some(Credential::OAuth {
                expires_at: UnixSeconds(1_791_203_400),
                ..
            })
        ));
        assert_eq!(from_mailo(r#"{"kind":"openpgp","v":"x"}"#), None);
        assert_eq!(from_mailo("not json"), None);
    }

    #[test]
    fn the_adopt_table_reads_its_section() {
        let table =
            AdoptTable::from_config("[adopt]\n\"org.quire.Mail\" = \"mailo\"\n").expect("table");
        let mail = AppName::parse("org.quire.Mail").expect("name");
        assert_eq!(table.service_of(&mail), Some("mailo"));
        let other = AppName::parse("org.example.Other").expect("name");
        assert_eq!(table.service_of(&other), None);
        assert_eq!(
            AdoptTable::from_config("").expect("empty"),
            AdoptTable::default()
        );
        assert!(AdoptTable::from_config("[adopt]\nnot a name = \"x\"\n").is_err());
    }
}
