//! Protocol families: the code behind providers (design/31 §3.1). A closed set; a provider
//! file names one per capability row, and adding a family needs code. The set lives here, below
//! the provider crate, because an account's endpoints name the family that speaks to them.

use crate::capability::CapabilityKind;
use crate::endpoint::EndpointProtocol;
use serde::{Deserialize, Serialize};

/// One protocol engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// IMAP.
    Imap,
    /// SMTP submission.
    Smtp,
    /// POP3.
    Pop3,
    /// JMAP (mail, contacts, calendars).
    Jmap,
    /// ManageSieve (RFC 5804): the server-side mail filters.
    Sieve,
    /// CalDAV.
    #[serde(rename = "caldav")]
    CalDav,
    /// CardDAV.
    #[serde(rename = "carddav")]
    CardDav,
    /// WebDAV files.
    #[serde(rename = "webdav")]
    WebDav,
    /// Microsoft Graph.
    Graph,
    /// The Gmail API.
    GmailApi,
    /// Google Calendar.
    GoogleCalendar,
    /// Google People.
    GooglePeople,
    /// Google Tasks.
    GoogleTasks,
    /// Google Drive.
    GoogleDrive,
    /// Google Photos, upload only.
    GooglePhotosUpload,
    /// Google Photos, the Picker API.
    GooglePhotosPicker,
    /// Dropbox.
    Dropbox,
    /// S3 and compatible stores.
    S3,
    /// The Nextcloud Notes API.
    NextcloudNotes,
    /// OpenAI-compatible Chat Completions.
    ChatCompletions,
    /// OpenAI Responses.
    Responses,
    /// Anthropic Messages.
    Messages,
    /// Gemini generateContent.
    GenerateContent,
    /// Ollama's own API.
    OllamaNative,
    /// A ComfyUI workflow registry.
    ComfyWorkflow,
    /// An external coding agent that speaks ACP and signs in by itself, or runs on an API key
    /// an account holds.
    AcpAgent,
}

impl Family {
    /// The family's stable slug (its serde form): provider files, audiences and the bus use it.
    pub fn slug(self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(slug)) => slug,
            // A fieldless enum always serializes to a string.
            _ => String::new(),
        }
    }

    /// The protocol an authenticated relay speaks for this family, or `None` for a family the
    /// relay does not carry (the AI wires go through inferd, the vendor APIs take bearer tokens).
    pub fn relay_protocol(self) -> Option<EndpointProtocol> {
        match self {
            Family::Imap => Some(EndpointProtocol::Imap),
            Family::Smtp => Some(EndpointProtocol::Smtp),
            Family::Sieve => Some(EndpointProtocol::Sieve),
            Family::Pop3 => Some(EndpointProtocol::Pop3),
            Family::Jmap
            | Family::CalDav
            | Family::CardDav
            | Family::WebDav
            | Family::NextcloudNotes
            | Family::Graph => Some(EndpointProtocol::Http),
            Family::GmailApi
            | Family::GoogleCalendar
            | Family::GooglePeople
            | Family::GoogleTasks
            | Family::GoogleDrive
            | Family::GooglePhotosUpload
            | Family::GooglePhotosPicker
            | Family::Dropbox
            | Family::S3
            | Family::ChatCompletions
            | Family::Responses
            | Family::Messages
            | Family::GenerateContent
            | Family::OllamaNative
            | Family::ComfyWorkflow
            | Family::AcpAgent => None,
        }
    }
}

impl Family {
    /// Whether an endpoint of this family is one a grant for `kind` may reach: mail reaches
    /// IMAP, SMTP and JMAP; files and photos WebDAV; calendars and tasks CalDAV (or JMAP);
    /// contacts CardDAV (or JMAP); notes the Nextcloud Notes API. Every other family is
    /// reached through its own client, not through an endpoint a candidate lists.
    pub fn serves(self, kind: CapabilityKind) -> bool {
        matches!(
            (self, kind),
            (
                Family::Imap | Family::Smtp | Family::Pop3 | Family::Sieve | Family::Jmap,
                CapabilityKind::Mail
            ) | (
                Family::WebDav | Family::Graph,
                CapabilityKind::Storage | CapabilityKind::Photos
            ) | (
                Family::CalDav,
                CapabilityKind::Calendar | CapabilityKind::Tasks
            ) | (Family::CardDav, CapabilityKind::Contacts)
                | (
                    Family::Graph,
                    CapabilityKind::Calendar | CapabilityKind::Contacts | CapabilityKind::Tasks
                )
                | (Family::NextcloudNotes, CapabilityKind::Notes)
                | (
                    Family::Jmap,
                    CapabilityKind::Calendar | CapabilityKind::Contacts
                )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &[(&str, Family, &str, Option<EndpointProtocol>)] = &[
        ("imap", Family::Imap, "imap", Some(EndpointProtocol::Imap)),
        ("smtp", Family::Smtp, "smtp", Some(EndpointProtocol::Smtp)),
        (
            "sieve",
            Family::Sieve,
            "sieve",
            Some(EndpointProtocol::Sieve),
        ),
        (
            "webdav",
            Family::WebDav,
            "webdav",
            Some(EndpointProtocol::Http),
        ),
        (
            "caldav",
            Family::CalDav,
            "caldav",
            Some(EndpointProtocol::Http),
        ),
        (
            "notes",
            Family::NextcloudNotes,
            "nextcloud_notes",
            Some(EndpointProtocol::Http),
        ),
        (
            "graph is a relayed http api",
            Family::Graph,
            "graph",
            Some(EndpointProtocol::Http),
        ),
        (
            "pop3 is relayed",
            Family::Pop3,
            "pop3",
            Some(EndpointProtocol::Pop3),
        ),
    ];

    #[test]
    fn a_family_serves_the_kinds_its_endpoints_are_for() {
        const CASES: &[(&str, Family, CapabilityKind, bool)] = &[
            ("imap for mail", Family::Imap, CapabilityKind::Mail, true),
            ("smtp for mail", Family::Smtp, CapabilityKind::Mail, true),
            ("sieve for mail", Family::Sieve, CapabilityKind::Mail, true),
            (
                "sieve not for files",
                Family::Sieve,
                CapabilityKind::Storage,
                false,
            ),
            (
                "webdav for files",
                Family::WebDav,
                CapabilityKind::Storage,
                true,
            ),
            (
                "webdav not for mail",
                Family::WebDav,
                CapabilityKind::Mail,
                false,
            ),
            (
                "caldav for tasks",
                Family::CalDav,
                CapabilityKind::Tasks,
                true,
            ),
            (
                "carddav for contacts",
                Family::CardDav,
                CapabilityKind::Contacts,
                true,
            ),
            (
                "jmap for calendars",
                Family::Jmap,
                CapabilityKind::Calendar,
                true,
            ),
            (
                "graph for files",
                Family::Graph,
                CapabilityKind::Storage,
                true,
            ),
            (
                "graph for photos",
                Family::Graph,
                CapabilityKind::Photos,
                true,
            ),
            (
                "graph for calendars",
                Family::Graph,
                CapabilityKind::Calendar,
                true,
            ),
            (
                "graph for contacts",
                Family::Graph,
                CapabilityKind::Contacts,
                true,
            ),
            (
                "graph for tasks",
                Family::Graph,
                CapabilityKind::Tasks,
                true,
            ),
            (
                "graph has no endpoint to list",
                Family::Graph,
                CapabilityKind::Mail,
                false,
            ),
        ];
        for (name, family, kind, serves) in CASES {
            assert_eq!(family.serves(*kind), *serves, "{name}");
        }
    }

    #[test]
    fn a_family_has_its_slug_and_its_relay_protocol() {
        for (name, family, slug, protocol) in CASES {
            assert_eq!(family.slug(), *slug, "{name}");
            assert_eq!(family.relay_protocol(), *protocol, "{name}");
        }
    }
}
