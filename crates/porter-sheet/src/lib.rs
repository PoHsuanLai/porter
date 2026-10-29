//! The accounts sheet as values (design/31 §5.6, porter PLAN G5): what the host draws
//! (`SheetView`), what the person does (`SheetInput`), what a sign-in says as it goes
//! (`Progress`) and the pure machine between them (`step`). Nothing here draws, reads a
//! secret store or talks to a provider.
//!
//! The words a person reads for these values are this crate's declared vocabulary, in [`words`]:
//! free functions over the typed values of the lower crates (a service's name, a group's heading,
//! what a whole store's access says), so no value type of porter carries a label. An app that
//! words them itself takes the typed values and ignores `words`.
//!
//! A view and an input cross the bus as JSON (`org.quire.AccountsSheet1`). An input may carry
//! secret text from the sheet into the daemon (a typed password); nothing on that interface
//! flows outward.

mod edit;
mod fields;
mod group;
pub mod host;
mod input;
mod manual;
mod mark;
mod progress;
mod stage;
mod view;
pub mod words;

pub use edit::{EditServers, EditedServers, ServersFixed, servers_editable, servers_fixed};
pub use fields::{Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, first_missing};
pub use group::ProviderGroup;
pub use input::SheetInput;
pub use manual::{
    Hop, JmapServer, MailServers, Manual, Protocol, Security, edit_form, form_problem,
    form_problem_among, manual_form, parse_manual, refit,
};
pub use mark::{MarkColour, MarkFace, MarkFaceError, MarkLetter};
pub use progress::{
    Progress, Review, ServiceRow, ServiceState, SignInFault, SignInInput, UserCode,
};
pub use stage::{Purpose, Sheet, SheetEffect, SheetEnd, SheetEvent, Stage, step};
pub use view::{
    FieldProblem, ProblemKind, ProviderKind, ProviderRow, ReviewView, RowKind, SheetView,
    SignInView,
};
