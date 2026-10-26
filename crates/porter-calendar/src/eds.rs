//! Evolution Data Server, the calendar store of GNOME and of most Linux desktops, over the
//! session bus.
//!
//! The calendars are the registry's sources (`Sources5`'s object manager) with a `[Calendar]`
//! group; a calendar is opened through the calendar factory, which answers with the object and
//! the bus name of its backend, and its events are then got, created, modified and removed there
//! as iCalendar text, by UID. Evolution Data Server keeps no etag a caller could send back, so
//! [`Stored::version`] is always `None` here.
//!
//! The bus names carry the interface's version (`Sources5`, `Calendar8`), which a desktop's
//! release moves: [`Names`] holds them, with today's as its default.

use crate::CalendarStore;
use crate::error::CalendarError;
use crate::event::{CalendarRef, Event, Stored};
use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

/// The bus names and paths Evolution Data Server answers at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// The source registry's bus name.
    pub sources: String,
    /// The registry's object manager.
    pub source_manager: String,
    /// The calendar factory's bus name.
    pub calendar: String,
    /// The calendar factory's object.
    pub factory: String,
}

impl Default for Names {
    fn default() -> Self {
        Self {
            sources: "org.gnome.evolution.dataserver.Sources5".to_owned(),
            source_manager: "/org/gnome/evolution/dataserver/SourceManager".to_owned(),
            calendar: "org.gnome.evolution.dataserver.Calendar8".to_owned(),
            factory: "/org/gnome/evolution/dataserver/CalendarFactory".to_owned(),
        }
    }
}

const SOURCE: &str = "org.gnome.evolution.dataserver.Source";
const FACTORY: &str = "org.gnome.evolution.dataserver.CalendarFactory";
const CALENDAR: &str = "org.gnome.evolution.dataserver.Calendar";

/// The calendars of the desktop's Evolution Data Server.
#[derive(Debug, Clone)]
pub struct Eds {
    bus: Connection,
    names: Names,
}

impl Eds {
    /// The store reached over `bus` (the session bus) at `names`.
    pub fn new(bus: Connection, names: Names) -> Self {
        Self { bus, names }
    }

    /// The calendar `source`'s backend, opened.
    async fn open(&self, source: &str) -> Result<Proxy<'static>, CalendarError> {
        let factory = Proxy::new(
            &self.bus,
            self.names.calendar.clone(),
            self.names.factory.clone(),
            FACTORY,
        )
        .await
        .map_err(bus)?;
        let (path, name): (OwnedObjectPath, String) = factory
            .call("OpenCalendar", &(source,))
            .await
            .map_err(bus)?;
        let calendar = Proxy::new(&self.bus, name, path, CALENDAR)
            .await
            .map_err(bus)?;
        calendar.call::<_, _, ()>("Open", &()).await.map_err(bus)?;
        Ok(calendar)
    }
}

/// A bus failure, as the store's.
fn bus(error: zbus::Error) -> CalendarError {
    CalendarError::Bus(error.to_string())
}

/// Whether `error` is the backend's "no such object".
fn not_found(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str().ends_with("ObjectNotFound"))
}

/// A source's key file, read as `group -> key -> value`.
fn key_file(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut group = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = name.to_owned();
            out.entry(group.clone()).or_default();
        } else if let Some((key, value)) = line.split_once('=') {
            out.entry(group.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    out
}

/// A source as a calendar, when it is one that is switched on.
fn calendar_of(uid: &str, data: &str) -> Option<CalendarRef> {
    let groups = key_file(data);
    let calendar = groups.get("Calendar")?;
    let source = groups.get("Data Source");
    if source.and_then(|s| s.get("Enabled")).map(String::as_str) == Some("false")
        || calendar.get("Enabled").map(String::as_str) == Some("false")
    {
        return None;
    }
    Some(CalendarRef {
        id: uid.to_owned(),
        name: source
            .and_then(|s| s.get("DisplayName"))
            .cloned()
            .unwrap_or_else(|| uid.to_owned()),
        color: calendar.get("Color").filter(|c| !c.is_empty()).cloned(),
        writable: true,
    })
}

fn text(value: Option<&OwnedValue>) -> Option<String> {
    value.and_then(|v| String::try_from(v.clone()).ok())
}

impl CalendarStore for Eds {
    async fn calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let manager = zbus::fdo::ObjectManagerProxy::builder(&self.bus)
            .destination(self.names.sources.clone())
            .map_err(bus)?
            .path(self.names.source_manager.clone())
            .map_err(bus)?
            .build()
            .await
            .map_err(bus)?;
        let objects = manager
            .get_managed_objects()
            .await
            .map_err(|e| CalendarError::Bus(e.to_string()))?;
        let mut out: Vec<CalendarRef> = objects
            .values()
            .filter_map(|interfaces| {
                let source = interfaces
                    .iter()
                    .find(|(name, _)| name.as_str() == SOURCE)
                    .map(|(_, properties)| properties)?;
                calendar_of(&text(source.get("UID"))?, &text(source.get("Data"))?)
            })
            .collect();
        out.sort_by_key(|calendar| calendar.name.to_lowercase());
        Ok(out)
    }

    async fn find(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        let backend = self.open(calendar).await?;
        match backend.call::<_, _, String>("GetObject", &(uid, "")).await {
            Ok(ics) => Ok(Some(Stored {
                calendar: calendar.to_owned(),
                uid: uid.to_owned(),
                id: uid.to_owned(),
                version: None,
                ics: Some(ics),
            })),
            Err(error) if not_found(&error) => Ok(None),
            Err(error) => Err(bus(error)),
        }
    }

    async fn put(&self, calendar: &str, event: &Event) -> Result<Stored, CalendarError> {
        let backend = self.open(calendar).await?;
        let objects = vec![event.ics.clone()];
        let held = match backend
            .call::<_, _, String>("GetObject", &(event.uid.as_str(), ""))
            .await
        {
            Ok(_) => true,
            Err(error) if not_found(&error) => false,
            Err(error) => return Err(bus(error)),
        };
        if held {
            backend
                .call::<_, _, ()>("ModifyObjects", &(objects, "all", 0u32))
                .await
                .map_err(bus)?;
        } else {
            backend
                .call::<_, _, Vec<String>>("CreateObjects", &(objects, 0u32))
                .await
                .map_err(bus)?;
        }
        Ok(Stored {
            calendar: calendar.to_owned(),
            uid: event.uid.clone(),
            id: event.uid.clone(),
            version: None,
            ics: Some(event.ics.clone()),
        })
    }

    async fn remove(&self, calendar: &str, uid: &str) -> Result<bool, CalendarError> {
        let backend = self.open(calendar).await?;
        let which = vec![(uid.to_owned(), String::new())];
        match backend
            .call::<_, _, ()>("RemoveObjects", &(which, "all", 0u32))
            .await
        {
            Ok(()) => Ok(true),
            Err(error) if not_found(&error) => Ok(false),
            Err(error) => Err(bus(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_a_calendar_when_it_has_a_switched_on_calendar_group() {
        let cases = [
            (
                "[Data Source]\nDisplayName=Work\nEnabled=true\n\n[Calendar]\nColor=#62a0ea\n",
                Some(("Work", Some("#62a0ea"))),
            ),
            (
                "[Data Source]\nDisplayName=Contacts\n\n[Address Book]\n",
                None,
            ),
            (
                "[Data Source]\nDisplayName=Old\nEnabled=false\n[Calendar]\n",
                None,
            ),
            ("[Calendar]\nColor=\n", Some(("src-4", None))),
        ];
        for (data, want) in cases {
            let got = calendar_of("src-4", data);
            assert_eq!(
                got.as_ref().map(|c| (c.name.as_str(), c.color.as_deref())),
                want,
                "{data}"
            );
        }
    }
}
