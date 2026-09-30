//! inferd: the AI broker (design/31 §4.1, §5.5): routing, policy, spend, audit, wire
//! adapters, the GPU queue. A skeleton: it builds the broker and exits, saying so.

mod adapters;

use adapters::AdapterModel;
use clap::Parser;
use porter_infer::{Broker, Policy};
use std::process::ExitCode;

/// porter's AI broker (`org.quire.Inference1`).
#[derive(Debug, Parser)]
#[command(name = "inferd", version)]
struct Args {}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Args {} = Args::parse();
    let broker: Broker<AdapterModel> = Broker::new(Policy::proposed(), Vec::new(), Vec::new());
    // The daemon's one log path is standard error, prefixed with its name.
    eprintln!(
        "inferd: not implemented: {} is frozen as an interface; local-only is {:?}",
        porter_dbus::INFERENCE_BUS,
        broker.policy().local_only
    );
    ExitCode::from(2)
}
