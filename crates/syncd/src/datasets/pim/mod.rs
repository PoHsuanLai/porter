//! `PimMirror`: calendars and address books mirrored from a CalDAV/CardDAV server into a local
//! vdir, read-only (design/31 §6.2 "PimMirror", PLAN §2 rows Calendar and Contacts, D9).
//!
//! The vdir is `$XDG_DATA_HOME/porter/vdir/<account>/<collection>/`, `<account>` being
//! `porter_core::object_segment(account)`: the segment `AccountRemoved` wipes. A collection holds
//! one file per item, `<uid>.ics` or `<uid>.vcf`, and the files `displayname` and `color`
//! (vdirsyncer's storage spec, which sill's `calendar.sources = auto` and khal read).
//!
//! - `mirror`: [`PimMirror`], the [`crate::dataset::Dataset`] of one collection, pull-only: the
//!   engine never uploads or removes anything for it, so a file edited or deleted locally never
//!   reaches the server.
//! - `vdir`: the file rules (names, atomic writes, metadata).
//! - `discover`: the collections of an account, from the principal and home sets (porter-dav).
//! - `plan`: collection names as directories and dataset slugs.
//! - `grants`: which granted accounts there are to mirror (a seam over porter-client).
//! - `source`: where the items come from (CalDAV/CardDAV, Graph calendars, Google's Calendar,
//!   People and Tasks APIs), by the capability's
//!   transport; see its module docs for the seam.
//! - `mirrors`: one account's collections kept running (replicas over the relay, one engine
//!   each) and the supervisor over every granted account.

mod discover;
mod grants;
mod mirror;
mod mirrors;
mod plan;
mod relay;
pub mod source;
mod supervisor;
mod vdir;

pub use discover::{Found, discover};
pub use grants::{AccountdUnavailable, ClientGrants, PimGrants, need_of};
pub use mirror::PimMirror;
pub use mirrors::{AccountMirrors, Wiring};
pub use plan::{Planned, plan};
pub use relay::{PimDial, pim_http, pim_replica};
pub use supervisor::{PimConfig, PimSupervisor};
pub use vdir::{COLOR, DISPLAYNAME, Meta, is_complete, items_in};

use porter_core::{DataClass, Family};

/// Which kind of collection a mirror holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PimKind {
    /// Calendars (CalDAV): `.ics` files.
    Calendar,
    /// Address books (CardDAV): `.vcf` files.
    Contacts,
    /// Task lists: `.ics` files holding a `VTODO`, in their own directories.
    Tasks,
}

impl PimKind {
    /// Every kind.
    pub const ALL: [PimKind; 3] = [PimKind::Calendar, PimKind::Contacts, PimKind::Tasks];

    /// The file extension of an item, with its dot.
    pub fn extension(self) -> &'static str {
        match self {
            PimKind::Calendar | PimKind::Tasks => ".ics",
            PimKind::Contacts => ".vcf",
        }
    }

    /// The `BEGIN` and `END` lines a whole item opens and closes with.
    pub fn envelope(self) -> (&'static str, &'static str) {
        match self {
            PimKind::Calendar | PimKind::Tasks => ("BEGIN:VCALENDAR", "END:VCALENDAR"),
            PimKind::Contacts => ("BEGIN:VCARD", "END:VCARD"),
        }
    }

    /// The protocol family that reaches it over DAV (a task list is a CalDAV collection of
    /// `VTODO`s).
    pub fn family(self) -> Family {
        match self {
            PimKind::Calendar | PimKind::Tasks => Family::CalDav,
            PimKind::Contacts => Family::CardDav,
        }
    }

    /// The data class a grant for it covers.
    pub fn class(self) -> DataClass {
        match self {
            PimKind::Calendar => DataClass::Calendar,
            PimKind::Contacts => DataClass::Contacts,
            PimKind::Tasks => DataClass::Tasks,
        }
    }

    /// What its directories end in: nothing for a calendar, `-contacts` for an address book,
    /// `-tasks` for a task list (so the three never share a directory).
    pub fn dir_suffix(self) -> &'static str {
        match self {
            PimKind::Calendar => "",
            PimKind::Contacts => "-contacts",
            PimKind::Tasks => "-tasks",
        }
    }

    /// The prefix of its datasets' names (`pim_cal_personal`, `pim_card_contacts_contacts`).
    pub fn slug(self) -> &'static str {
        match self {
            PimKind::Calendar => "pim_cal",
            PimKind::Contacts => "pim_card",
            PimKind::Tasks => "pim_task",
        }
    }
}
