//! Why an attached engine cannot answer now: the one thing that is wrong, typed.

use super::key::KeyFileProblem;
use std::path::PathBuf;

/// Why an attached engine cannot answer now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    /// Nothing is at the socket's path: the tunnel is down.
    SocketMissing {
        /// The path.
        path: PathBuf,
    },
    /// The connection was refused or broke: nothing is listening.
    Refused,
    /// The engine wants a bearer token it was not given, or a different one.
    Unauthorized {
        /// 401 or 403.
        status: u16,
    },
    /// The engine answers and does not serve the model.
    ModelAbsent {
        /// The names it does serve (the first of them).
        served: Vec<String>,
    },
    /// The key file was refused, so no request was sent.
    KeyFile(KeyFileProblem),
    /// It answered, but not with a model list, or not in time.
    Unanswered,
}

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotReady::SocketMissing { path } => {
                write!(f, "nothing is at {}: is the tunnel up?", path.display())
            }
            NotReady::Refused => f.write_str("the connection was refused"),
            NotReady::Unauthorized { status } => {
                write!(
                    f,
                    "the engine answered {status}: it wants a bearer token (key_file)"
                )
            }
            NotReady::ModelAbsent { served } => {
                write!(
                    f,
                    "the engine does not serve the model; it serves: {}",
                    served.join(", ")
                )
            }
            NotReady::KeyFile(problem) => write!(f, "{problem}"),
            NotReady::Unanswered => f.write_str("the engine did not answer with a model list"),
        }
    }
}

impl NotReady {
    /// Whether the person set this up wrong (their file), rather than the engine being away.
    pub fn is_setup(&self) -> bool {
        matches!(
            self,
            NotReady::KeyFile(_) | NotReady::Unauthorized { .. } | NotReady::ModelAbsent { .. }
        )
    }
}
