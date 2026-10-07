//! A People API contact as a vCard 3.0: pure, no I/O.
//!
//! | People `Person` | vCard 3.0 |
//! |---|---|
//! | `resourceName` | `UID` |
//! | `names[0]` | `N` (family, given, middle, prefix, suffix), `FN` (`displayName`, else the parts, else the first address, number or organisation, else the resource name) |
//! | `emailAddresses` | `EMAIL;TYPE=INTERNET[,HOME\|WORK\|OTHER]` |
//! | `phoneNumbers` | `TEL;TYPE=CELL\|HOME\|WORK\|...` |
//! | `addresses` | `ADR;TYPE=HOME\|WORK\|OTHER` (the structured fields; `formattedValue` when there are none) |
//! | `organizations` | `ORG` (name, department), `TITLE` |
//! | `birthdays[0]` | `BDAY` (`YYYY-MM-DD`; with no year, `--MM-DD`) |
//! | `biographies[0]` | `NOTE` |
//! | `urls` | `URL` |
//!
//! Not carried: photos, nicknames, relations, events other than the birthday, custom fields,
//! group memberships. Types Google names freely (`"type": "custom"`, a label of the person's
//! own) are left out rather than guessed.

use super::super::graph::convert::{escape, fold};
use serde::Deserialize;

/// A contact the way `people.connections.list` gives it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Person {
    /// `people/c1234`.
    #[serde(rename = "resourceName")]
    pub resource_name: String,
    /// Its version.
    pub etag: Option<String>,
    /// Bookkeeping, which says when a contact was deleted.
    pub metadata: Option<Metadata>,
    /// Names.
    #[serde(default)]
    pub names: Vec<Name>,
    /// Addresses.
    #[serde(rename = "emailAddresses", default)]
    pub emails: Vec<Typed>,
    /// Numbers.
    #[serde(rename = "phoneNumbers", default)]
    pub phones: Vec<Typed>,
    /// Postal addresses.
    #[serde(default)]
    pub addresses: Vec<Postal>,
    /// Employers.
    #[serde(default)]
    pub organizations: Vec<Organization>,
    /// Birthdays.
    #[serde(default)]
    pub birthdays: Vec<Birthday>,
    /// Notes.
    #[serde(default)]
    pub biographies: Vec<Biography>,
    /// Web pages.
    #[serde(default)]
    pub urls: Vec<Typed>,
}

/// The `metadata` of a person.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Metadata {
    /// Set on a contact that was deleted (only in an incremental sync).
    pub deleted: Option<bool>,
}

/// A name.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Name {
    /// How the contact is shown.
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    /// The family name.
    #[serde(rename = "familyName")]
    pub family: Option<String>,
    /// The given name.
    #[serde(rename = "givenName")]
    pub given: Option<String>,
    /// The middle name.
    #[serde(rename = "middleName")]
    pub middle: Option<String>,
    /// Dr, Ms.
    #[serde(rename = "honorificPrefix")]
    pub prefix: Option<String>,
    /// Jr, PhD.
    #[serde(rename = "honorificSuffix")]
    pub suffix: Option<String>,
}

/// An address, a number or a URL: a value and a type.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Typed {
    /// The value.
    pub value: Option<String>,
    /// `home`, `work`, `mobile` and so on, or `custom`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

/// A postal address.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Postal {
    /// `home`, `work`, `other`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// The whole address as Google formats it.
    #[serde(rename = "formattedValue")]
    pub formatted: Option<String>,
    /// A post office box.
    #[serde(rename = "poBox")]
    pub po_box: Option<String>,
    /// Street and number.
    #[serde(rename = "streetAddress")]
    pub street: Option<String>,
    /// Flat, floor.
    #[serde(rename = "extendedAddress")]
    pub extended: Option<String>,
    /// Town.
    pub city: Option<String>,
    /// State or province.
    pub region: Option<String>,
    /// Post code.
    #[serde(rename = "postalCode")]
    pub postal_code: Option<String>,
    /// Country.
    pub country: Option<String>,
}

/// An employer.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Organization {
    /// The name.
    pub name: Option<String>,
    /// The department.
    pub department: Option<String>,
    /// The job title.
    pub title: Option<String>,
}

/// A birthday.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Birthday {
    /// The date; the year may be missing.
    pub date: Option<Date>,
}

/// A date with parts that may be missing.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Date {
    /// Year.
    pub year: Option<u32>,
    /// Month.
    pub month: Option<u32>,
    /// Day.
    pub day: Option<u32>,
}

/// A note.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Biography {
    /// The text.
    pub value: Option<String>,
}

impl Person {
    /// Whether the contact was deleted: the item is gone.
    pub fn is_deleted(&self) -> bool {
        self.metadata.as_ref().and_then(|m| m.deleted) == Some(true)
    }
}

fn text(value: &Option<String>) -> &str {
    value.as_deref().map_or("", str::trim)
}

fn present(value: &Option<String>) -> Option<&str> {
    Some(text(value)).filter(|t| !t.is_empty())
}

fn emails(person: &Person) -> impl Iterator<Item = (&str, &Typed)> {
    person
        .emails
        .iter()
        .filter_map(|e| Some((present(&e.value)?, e)))
}

/// The name a card is shown by.
fn full_name(person: &Person) -> String {
    let name = person.names.first().cloned().unwrap_or_default();
    let parts: Vec<&str> = [
        &name.prefix,
        &name.given,
        &name.middle,
        &name.family,
        &name.suffix,
    ]
    .into_iter()
    .filter_map(present)
    .collect();
    present(&name.display_name)
        .map(str::to_owned)
        .or_else(|| (!parts.is_empty()).then(|| parts.join(" ")))
        .or_else(|| emails(person).next().map(|(v, _)| v.to_owned()))
        .or_else(|| {
            person
                .phones
                .iter()
                .find_map(|p| present(&p.value))
                .map(str::to_owned)
        })
        .or_else(|| {
            person
                .organizations
                .iter()
                .find_map(|o| present(&o.name))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| person.resource_name.clone())
}

fn email_type(kind: Option<&str>) -> &'static str {
    match kind {
        Some("home") => "INTERNET,HOME",
        Some("work") => "INTERNET,WORK",
        Some("other") => "INTERNET,OTHER",
        _ => "INTERNET",
    }
}

fn phone_type(kind: Option<&str>) -> Option<&'static str> {
    match kind? {
        "mobile" => Some("CELL"),
        "home" => Some("HOME"),
        "work" => Some("WORK"),
        "homeFax" => Some("HOME,FAX"),
        "workFax" => Some("WORK,FAX"),
        "otherFax" => Some("FAX"),
        "pager" => Some("PAGER"),
        "main" => Some("PREF"),
        "other" => Some("VOICE"),
        _ => None,
    }
}

fn address_type(kind: Option<&str>) -> Option<&'static str> {
    match kind? {
        "home" => Some("HOME"),
        "work" => Some("WORK"),
        "other" => Some("OTHER"),
        _ => None,
    }
}

fn with_type(property: &str, kind: Option<&str>, value: &str) -> String {
    match kind {
        Some(kind) => format!("{property};TYPE={kind}:{value}"),
        None => format!("{property}:{value}"),
    }
}

/// The `ADR` value: PO box, extended, street, city, region, code, country.
fn adr_value(address: &Postal) -> Option<String> {
    let fields = [
        &address.po_box,
        &address.extended,
        &address.street,
        &address.city,
        &address.region,
        &address.postal_code,
        &address.country,
    ];
    match fields.iter().any(|f| present(f).is_some()) {
        true => Some(
            fields
                .iter()
                .map(|f| escape(text(f)))
                .collect::<Vec<_>>()
                .join(";"),
        ),
        // Only the formatted text: it all goes in the street field.
        false => {
            present(&address.formatted).map(|t| format!(";;{};;;;", escape(&t.replace('\n', ", "))))
        }
    }
}

fn birthday(date: &Date) -> Option<String> {
    let (month, day) = (
        date.month.filter(|m| (1..=12).contains(m))?,
        date.day.filter(|d| (1..=31).contains(d))?,
    );
    Some(match date.year.filter(|y| *y > 0) {
        Some(year) => format!("{year:04}-{month:02}-{day:02}"),
        None => format!("--{month:02}-{day:02}"),
    })
}

/// The vCard of `person`.
pub fn to_vcf(person: &Person) -> String {
    let name = person.names.first().cloned().unwrap_or_default();
    let component = |value: &Option<String>| escape(text(value));
    let mut lines: Vec<String> = vec![
        "BEGIN:VCARD".into(),
        "VERSION:3.0".into(),
        "PRODID:-//porter//syncd google contacts//EN".into(),
        format!("UID:{}", escape(&person.resource_name)),
        format!("FN:{}", escape(&full_name(person))),
        format!(
            "N:{};{};{};{};{}",
            component(&name.family),
            component(&name.given),
            component(&name.middle),
            component(&name.prefix),
            component(&name.suffix)
        ),
    ];
    lines.extend(
        emails(person).map(|(value, e)| {
            with_type("EMAIL", Some(email_type(e.kind.as_deref())), &escape(value))
        }),
    );
    lines.extend(person.phones.iter().filter_map(|p| {
        let value = present(&p.value)?;
        Some(with_type(
            "TEL",
            phone_type(p.kind.as_deref()),
            &escape(value),
        ))
    }));
    lines.extend(person.addresses.iter().filter_map(|a| {
        Some(with_type(
            "ADR",
            address_type(a.kind.as_deref()),
            &adr_value(a)?,
        ))
    }));
    if let Some(org) = person
        .organizations
        .iter()
        .find(|o| present(&o.name).or(present(&o.department)).is_some())
    {
        let parts = [text(&org.name), text(&org.department)];
        let value = match parts[1].is_empty() {
            true => escape(parts[0]),
            false => format!("{};{}", escape(parts[0]), escape(parts[1])),
        };
        lines.push(format!("ORG:{value}"));
    }
    lines.extend(
        person
            .organizations
            .iter()
            .find_map(|o| present(&o.title))
            .map(|t| format!("TITLE:{}", escape(t))),
    );
    lines.extend(
        person
            .birthdays
            .iter()
            .find_map(|b| birthday(b.date.as_ref()?))
            .map(|b| format!("BDAY:{b}")),
    );
    lines.extend(
        person
            .biographies
            .iter()
            .find_map(|b| present(&b.value))
            .map(|n| format!("NOTE:{}", escape(n))),
    );
    lines.extend(
        person
            .urls
            .iter()
            .filter_map(|u| present(&u.value))
            .map(|u| format!("URL:{}", escape(u))),
    );
    lines.push("END:VCARD".into());
    lines.iter().map(|l| fold(l)).collect()
}

#[cfg(test)]
mod tests;
