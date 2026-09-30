//! Protocol families: the code behind providers (design/31 §3.1). A closed set; a provider
//! file names one per capability row, and adding a family needs code.

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
}
