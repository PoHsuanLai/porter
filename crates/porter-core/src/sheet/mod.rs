//! The accounts sheet as values (design/31 §5.6, porter PLAN G5): what the host draws
//! (`SheetView`), what the person does (`SheetInput`), what a sign-in says as it goes
//! (`Progress`) and the pure machine between them (`step`). Nothing here draws, reads a
//! secret store or talks to a provider; labels a person reads belong to the UI that draws a
//! view (quire's `ds-shell::accounts`, mapped from these values in sill and mailo).
//!
//! A view and an input cross the bus as JSON (`org.quire.AccountsSheet1`). An input may carry
//! secret text from the sheet into the daemon (a typed password); nothing on that interface
//! flows outward.

mod fields;
mod input;
mod manual;
mod progress;
mod stage;
mod view;

pub use fields::{Entry, FieldAnswer, FieldKind, FieldSpec, FieldValue, Presence, first_missing};
pub use input::SheetInput;
pub use manual::{
    Hop, JmapServer, MailServers, Manual, Protocol, Security, form_problem, manual_form,
    parse_manual, refit,
};
pub use progress::{
    Progress, Review, ServiceChoice, ServiceRow, ServiceState, SignInFault, SignInInput, UserCode,
};
pub use stage::{Purpose, Sheet, SheetEffect, SheetEnd, SheetEvent, Stage, step};
pub use view::{
    FieldProblem, ProblemKind, ProviderKind, ProviderRow, ReviewView, RowKind, SheetView,
    SignInView,
};
