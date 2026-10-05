//! Probing local ports for a runtime (Ollama, llama.cpp, LM Studio, ComfyUI).

use porter_core::Claim;
use porter_http::Http;
use porter_provider::Port;

/// A runtime found on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeHit {
    /// The port.
    pub port: Port,
    /// What it serves, as `Discovered` claims.
    pub claims: Vec<Claim>,
}

/// Asks each of `ports` on 127.0.0.1 what answers, in order.
pub async fn probe_ports<H: Http>(http: &H, ports: &[Port]) -> Vec<ProbeHit> {
    let _ = (http, ports);
    todo!("GET `/api/tags` and `/api/show`, `/props`, `/v1/models` per port; skip what refuses")
}
