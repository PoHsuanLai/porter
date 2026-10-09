//! Who may use this computer's models: a decision made from Tailscale's own answer about the
//! computer a request came from, and from what the person said.
//!
//! In this order, the first that holds decides:
//!
//! 1. The request came from one of this computer's own addresses: [`Refusal::ThisComputer`]. A
//!    program of another account on this computer reaches a listener at the network address
//!    through that address, and Tailscale then says it is "this computer", which is of the same
//!    user: without this rule it would pass as the person's other computer.
//! 2. Tailscale could not say who has that address ([`Refusal::Unknown`], or
//!    [`Refusal::CouldNotCheck`] when it could not be asked), or says it is this very computer
//!    ([`Refusal::ThisComputer`]), or does not list the address among the computer's own.
//! 3. The person said no to it: [`Refusal::Denied`]; said yes: let in, whoever it belongs to.
//! 4. It is a tagged server ([`Refusal::Tagged`]) or another person's, or shared in
//!    ([`Refusal::Shared`]): refused, since only the person's own word, given ahead of time in
//!    Settings, lets such a computer in.
//! 5. Otherwise it is the person's own and nobody has answered about it: it is let through as
//!    [`Footing::New`], and the person is asked before it uses a model.

use crate::guests::{Guests, State};
use crate::identity::Identity;
use crate::refusal::Refusal;
use porter_core::{MachineOwner, NodeId};
use porter_tailscale::{TailscaleError, WhoIs};
use std::net::IpAddr;

/// A computer that asked, as Tailscale names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Its stable id.
    pub node: NodeId,
    /// The name people call it by (the first part of its network name).
    pub name: String,
    /// Whose it is.
    pub owner: MachineOwner,
}

/// Where a computer that may ask stands with the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Footing {
    /// The person said yes to it.
    Approved,
    /// The person's own computer, not answered about yet.
    New,
}

/// A computer that passed the judgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    peer: Peer,
    footing: Footing,
}

impl Welcome {
    /// The computer.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// Where it stands.
    pub fn footing(&self) -> Footing {
        self.footing
    }

    /// Whether it may use a model now, or is to be asked about first. A computer the person has
    /// not answered about is put on the list of those waiting (the person is told once) and
    /// refused with [`Refusal::Waiting`] until they answer. The record is read now, so a yes
    /// taken back is taken back at once.
    pub fn admit(&self, guests: &Guests, now: porter_core::UnixSeconds) -> Result<(), Refusal> {
        match guests.state_of(&self.peer.node) {
            Some(State::Approved) => Ok(()),
            Some(State::Denied) => Err(Refusal::Denied),
            None if self.peer.owner == MachineOwner::Mine => {
                guests
                    .ask(&self.peer.node, &self.peer.name, now)
                    .map_err(|e| match e {
                        crate::guests::GuestError::TooManyAsking => Refusal::TooManyAsking,
                        _ => Refusal::NotKept,
                    })?;
                Err(Refusal::Waiting)
            }
            // Someone else's computer is let in by the person's word alone, never asked about.
            None => Err(match self.peer.owner {
                MachineOwner::Tagged => Refusal::Tagged,
                _ => Refusal::Shared,
            }),
        }
    }
}

/// Judges a request that came from `from`, when Tailscale answered `answer` to the question of
/// who has that address. `me` is this computer.
pub fn judge(
    me: &Identity,
    from: IpAddr,
    answer: Result<WhoIs, TailscaleError>,
    guests: &Guests,
) -> Result<Welcome, Refusal> {
    if me.owns(from) {
        return Err(Refusal::ThisComputer);
    }
    let who = match answer {
        Ok(who) => who,
        Err(TailscaleError::NoSuchPeer) => return Err(Refusal::Unknown),
        Err(_) => return Err(Refusal::CouldNotCheck),
    };
    if who.node.id == *me.node() {
        return Err(Refusal::ThisComputer);
    }
    // An address Tailscale does not list among the computer's own is not trusted to be it.
    if !who.node.addresses.contains(&from) {
        return Err(Refusal::Unknown);
    }
    let owner = who.owner(me.user());
    let name = match who.node.dns.split('.').next() {
        Some(first) if !first.is_empty() => first.to_owned(),
        _ => who.node.id.to_string(),
    };
    let peer = Peer {
        node: who.node.id,
        name,
        owner,
    };
    match guests.state_of(&peer.node) {
        Some(State::Denied) => Err(Refusal::Denied),
        Some(State::Approved) => Ok(Welcome {
            peer,
            footing: Footing::Approved,
        }),
        None => match owner {
            MachineOwner::Mine => Ok(Welcome {
                peer,
                footing: Footing::New,
            }),
            MachineOwner::Tagged => Err(Refusal::Tagged),
            MachineOwner::Shared => Err(Refusal::Shared),
        },
    }
}

#[cfg(test)]
mod tests;
