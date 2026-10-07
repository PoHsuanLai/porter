//! Graph's time zone names to the IANA names an iCalendar `TZID` carries. Graph writes Windows
//! names (`Pacific Standard Time`) unless asked for another form; it writes IANA names when the
//! client says `Prefer: outlook.timezone`, and `UTC` for UTC.

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

const WINDOWS: &[(&str, &str)] = &[
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("Mountain Standard Time", "America/Denver"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Central Standard Time", "America/Chicago"),
    ("Eastern Standard Time", "America/New_York"),
    ("US Eastern Standard Time", "America/Indianapolis"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Canada Central Standard Time", "America/Regina"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Mexico Standard Time", "America/Mexico_City"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("Argentina Standard Time", "America/Buenos_Aires"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("India Standard Time", "Asia/Calcutta"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
];

/// The zone a Graph name stands for; no name is UTC (Graph's default for a date-time with none).
pub fn zone(name: Option<&str>) -> Zone {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Zone::Utc;
    };
    match name {
        "UTC" | "Etc/UTC" | "Coordinated Universal Time" | "GMT" | "Z" => Zone::Utc,
        _ => match WINDOWS.iter().find(|(windows, _)| *windows == name) {
            Some((_, iana)) => Zone::Iana((*iana).to_owned()),
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
