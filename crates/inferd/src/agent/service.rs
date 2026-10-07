//! `org.quire.Inference1.Agents` at `/org/quire/Inference1`: the agent launcher's way to ask for
//! an endpoint. The caller is derived from the bus connection, never sent; any role but the
//! launcher is refused `AccessDenied`. A session lives as long as the connection that opened
//! it: when that name leaves the bus, its sessions end.

use super::open::OpenRequest;
use super::refusal::{Cause, Refusal};
use super::session::Agents;
use crate::peers::{Peers, Role};
use porter_dbus::{Details, EndpointArg, RouteArg};
use std::sync::Arc;
use zbus::message::Header;

/// The bus object.
#[derive(Debug)]
pub struct AgentsService<P> {
    pub(crate) peers: Arc<P>,
    pub(crate) agents: Agents,
}

impl<P> AgentsService<P> {
    /// The sender of a call, when it is the launcher.
    async fn launcher(&self, header: &Header<'_>) -> Result<String, Refusal>
    where
        P: Peers,
    {
        let denied = || {
            Refusal::new(
                Cause::AccessDenied,
                "only the agent launcher may open agent endpoints",
            )
        };
        let sender = header.sender().ok_or_else(denied)?.to_string();
        match self.peers.caller_of(&sender).await {
            Some(caller) if caller.role == Role::AgentLauncher => Ok(sender),
            _ => Err(denied()),
        }
    }
}

/// The next item of a stream.
async fn next_of<S: zbus::export::futures_core::Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

/// Ends `owner`'s sessions when its connection leaves the bus.
async fn watch(connection: &zbus::Connection, agents: Agents, owner: String) {
    if !agents.begin_watching(&owner) {
        return;
    }
    let Ok(bus) = zbus::fdo::DBusProxy::new(connection).await else {
        return;
    };
    let Ok(owner_name) = zbus::names::BusName::try_from(owner.clone()) else {
        return;
    };
    let Ok(mut changes) = bus
        .receive_name_owner_changed_with_args(&[(0, owner.as_str())])
        .await
    else {
        return;
    };
    tokio::spawn(async move {
        // The connection may have gone between the open and the subscription.
        let gone_already = !bus.name_has_owner(owner_name).await.unwrap_or(false);
        if !gone_already {
            while let Some(change) = next_of(&mut changes).await {
                if change.args().is_ok_and(|args| args.new_owner().is_none()) {
                    break;
                }
            }
        }
        agents.close_owned_by(&owner).await;
        agents.end_watching(&owner);
    });
}

#[zbus::interface(name = "org.quire.Inference1.Agents")]
impl<P: Peers> AgentsService<P> {
    // The bus's arguments are the interface's: five of its own, the header and the connection.
    #[allow(clippy::too_many_arguments)]
    async fn open_endpoint(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        program: String,
        route: RouteArg,
        class: String,
        protocols: Vec<String>,
        _options: Details,
    ) -> Result<EndpointArg, Refusal> {
        let owner = self.launcher(&header).await?;
        let endpoint = self
            .agents
            .open(
                &owner,
                OpenRequest {
                    program,
                    route,
                    class,
                    protocols,
                },
            )
            .await?;
        watch(connection, self.agents.clone(), owner).await;
        Ok(endpoint.arg())
    }

    async fn close_endpoint(
        &self,
        #[zbus(header)] header: Header<'_>,
        session: String,
    ) -> Result<(), Refusal> {
        let owner = self.launcher(&header).await?;
        if self.agents.close(&owner, &session).await {
            Ok(())
        } else {
            Err(Refusal::new(
                Cause::NoSuchSession,
                "no such session of yours",
            ))
        }
    }
}
