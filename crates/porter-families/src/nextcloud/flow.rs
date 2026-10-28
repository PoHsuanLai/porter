//! Login Flow v2's two calls, sent: start a flow, and poll it. The requests and the reading of
//! the answers are nextcloud-proto's.

use super::fault;
use crate::io::Io;
use crate::password::fault_of;
use nextcloud_proto::flow::{self, Polled, Started};
use porter_core::EndpointUrl;
use porter_http::Http;
use porter_sheet::SignInFault;

/// Starts a flow at `server`.
pub(super) async fn start(io: &Io, server: &EndpointUrl) -> Result<Started, SignInFault> {
    let request = flow::start_request(server).map_err(fault)?;
    let response = io.http.send(request).await.map_err(fault_of)?;
    flow::started(&response).map_err(fault)
}

/// Polls once: what came of it, or the fault that ends the flow. A server that could not be
/// reached this time is a hiccup, not an end.
pub(super) async fn poll(io: &Io, started: &Started) -> Result<Polled, SignInFault> {
    let request = flow::poll_request(started).map_err(fault)?;
    match io.http.send(request).await {
        Ok(response) => Ok(flow::polled(&response)),
        Err(error) => match fault_of(error) {
            SignInFault::Unreachable => Ok(Polled::Hiccup),
            fault => Err(fault),
        },
    }
}
