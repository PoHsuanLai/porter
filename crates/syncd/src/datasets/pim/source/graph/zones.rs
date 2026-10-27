//! Graph's time zone names to the IANA names an iCalendar `TZID` carries. Graph writes Windows
//! names (`Pacific Standard Time`) unless asked for another form; it writes IANA names when the
//! client says `Prefer: outlook.timezone`, and `UTC` for UTC. The Windows names are
//! looked up in the `windows-zones` crate.

/// What a Graph zone name is in iCalendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zone {
    /// UTC: times end in `Z`.
    Utc,
    /// An IANA name: `TZID=`.
    Iana(String),
    /// A name this table does not know: the time is written floating and the name kept in a
    /// comment.
    Unknown(String),
}

/// The zone a Graph name stands for; no name is UTC (Graph's default for a date-time with none).
pub fn zone(name: Option<&str>) -> Zone {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Zone::Utc;
    };
    match name {
        "UTC" | "Etc/UTC" | "Coordinated Universal Time" | "GMT" | "Z" => Zone::Utc,
        _ => match windows_zones::iana(name) {
            Some(iana) => Zone::Iana(iana.to_owned()),
            None if name.contains('/') && name.chars().all(|c| c.is_ascii_graphic()) => {
                Zone::Iana(name.to_owned())
            }
            None => Zone::Unknown(name.to_owned()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_utc_windows_iana_or_unknown() {
        let iana = |s: &str| Zone::Iana(s.to_owned());
        let cases: Vec<(Option<&str>, Zone)> = vec![
            (None, Zone::Utc),
            (Some(""), Zone::Utc),
            (Some("UTC"), Zone::Utc),
            (Some("Pacific Standard Time"), iana("America/Los_Angeles")),
            (Some("W. Europe Standard Time"), iana("Europe/Berlin")),
            (Some("Europe/Paris"), iana("Europe/Paris")),
            (
                Some("Mars Standard Time"),
                Zone::Unknown("Mars Standard Time".into()),
            ),
        ];
        for (name, want) in cases {
            assert_eq!(zone(name), want, "{name:?}");
        }
    }
}
