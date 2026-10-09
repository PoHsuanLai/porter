//! Who answers an inference session, and what this computer can run: the part of inferd that an
//! app or another daemon can use without inferd. No bus, no runtime, no process: every function is
//! pure over the lists it is given, or reads a file when it says so.
//!
//! The pieces, from the bottom:
//!
//! - [`catalog`]: stoker's model catalogue (`ModelEntry` files) read from the directories it is
//!   given, and the capabilities each entry claims ([`catalog::claims_of`]).
//! - [`local`]: [`local::LocalModel`], one per catalogue entry a configured engine can serve, with
//!   its card (what routing sees), the supervisor's spec (how to start its engine) and the socket
//!   it listens on; [`local::EngineConfig`] is where the engine programs and the weights are
//!   ([`local::build`] makes the models). Nothing here starts an engine.
//! - [`attached`]: engines the person already runs (a socket or a loopback address), as values:
//!   the table of the file, the token's file, the target of a connect and what is wrong when the
//!   engine cannot answer.
//! - [`swap`]: what loading a model now would take, from stoker's pure memory budget
//!   ([`swap::swap_cost`]), which is the router's `SwapCost` input.
//! - [`router`]: the models that meet a need, minus what the policy forbids, then the person's
//!   picks and `porter_infer::pick` ([`router::choose`], over [`router::Listed`] models), and
//!   [`router::placed`] for a caller that names the places a session may run.
//! - [`pipeline`]: the plan for a request over those models ([`pipeline::plan`]) and the route of
//!   a language session ([`pipeline::decide_text`]).
//! - [`session`]: the session machine, [`session::step`], a pure transition function from a frame
//!   or an answer to the next state and what to do about it, with its tables in `step_tests`.
//! - [`seams`] and [`carried`]: the traits a session server leans on (who answers, the engines
//!   behind it, the model turn, the audit trail), and what a finished turn carried. Running turns
//!   is `porter-turns`; hosting the socket is the daemon's.
//! - [`audio`] and [`startup`]: the rules every audio turn obeys, and why an engine did not start.
//!
//! Nothing here reads the environment or the clock; a daemon (inferd) or an app passes in what it
//! knows. The `testing` feature adds [`testkit`], the helpers the tests of the crates built on
//! this one share.
//!
//! ```
//! use porter_core::DataClass;
//! use porter_router::router::check_class;
//!
//! // A computer-use session carries the screen and nothing else.
//! assert!(check_class(DataClass::Screen).is_ok());
//! assert!(check_class(DataClass::Mail).is_err());
//! ```

pub mod attached;
pub mod audio;
pub mod carried;
pub mod catalog;
pub mod local;
pub mod pipeline;
pub mod router;
pub mod seams;
pub mod session;
pub mod startup;
pub mod swap;
#[cfg(any(test, feature = "testing"))]
pub mod testkit;

pub use carried::Carried;
pub use local::{EngineConfig, LocalModel, Weights, build};
pub use router::{Decided, Listed, choose};
pub use seams::{
    AuditSink, EngineFailed, EngineHost, Router, RunningTurn, Seams, TurnRunner, TurnStep,
};
pub use session::{SessionSpec, step};
