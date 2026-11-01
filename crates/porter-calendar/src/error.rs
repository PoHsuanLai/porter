//! Why a calendar store did not do what it was asked.

use porter_http::HttpError;

/// Why a calendar store did not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalendarError {
    /// No answer: the seam could not send the request.
    #[error("the calendar store could not be reached: {0}")]
    Http(#[from] HttpError),
    /// `401` or `403`: the sign-in, or the access it carries, was refused.
    #[error("the calendar store refused the sign-in")]
    Unauthorized,
    /// `412`: the event changed in the store since it was read, so it was not overwritten.
    #[error("the event changed in the calendar store since it was read")]
    Changed,
    /// Any other status the exchange could not go on from.
    #[error("the calendar store answered {status} to {what}")]
    Status {
        /// The HTTP status.
        status: u16,
        /// What was asked.
        what: &'static str,
    },
    /// The answer was not the document asked for.
    #[error("the calendar store's answer could not be read: {0}")]
    Malformed(String),
    /// The bus refused or failed a call (Evolution Data Server).
    #[error("the calendar service failed: {0}")]
    Bus(String),
}

impl CalendarError {
    /// The error a status that is not a success stands for.
    pub(crate) fn of_status(status: u16, what: &'static str) -> Self {
        match status {
            401 | 403 => CalendarError::Unauthorized,
            412 => CalendarError::Changed,
            status => CalendarError::Status { status, what },
        }
    }
}
