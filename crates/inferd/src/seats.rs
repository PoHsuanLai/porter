//! The seats of the engines the daemon runs, shared by every session: a [`Gate`] holds the pure
//! admission state ([`crate::admit`]) behind a lock and tells each waiting request when it has
//! its turn and how many stand ahead of it. A request holds a [`Pass`]; dropping the pass (the
//! request ended, was cancelled, failed, or its task was aborted) frees the seat.
//!
//! The lock is held for a few map operations and never across an await.

use crate::admit::{Admission, Lane, Place, Priority, Promoted, Seats, Ticket};
use engine_supervisor::EngineId;
use model_catalog::{EngineArg, EngineKind};
use porter_core::Count;
use porter_infer::Asker;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::watch;

/// Where a request stands, as the request sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It has a seat.
    Seated,
    /// It waits; `ahead` requests are served first.
    Waiting {
        /// How many are ahead.
        ahead: Count,
    },
}

impl From<Place> for Standing {
    fn from(place: Place) -> Self {
        match place {
            Place::Seated => Standing::Seated,
            Place::Waiting { ahead } => Standing::Waiting { ahead },
        }
    }
}

/// How many requests the engine launched with `args` takes at once: vLLM's `--max-num-seqs`,
/// llama.cpp's `--parallel` (or `-np`), as written in the launch arguments; one when they say
/// nothing or the engine is not a chat engine.
pub fn seats_in(kind: EngineKind, args: &[EngineArg]) -> Seats {
    let names: &[&str] = match kind {
        EngineKind::Vllm => &["--max-num-seqs"],
        EngineKind::LlamaServer => &["--parallel", "-np"],
        _ => &[],
    };
    let value = args.iter().enumerate().find_map(|(index, arg)| {
        names
            .iter()
            .find_map(|name| match arg.0.strip_prefix(*name)? {
                "" => args.get(index + 1)?.0.parse::<u32>().ok(),
                rest => rest.strip_prefix('=')?.parse::<u32>().ok(),
            })
    });
    Seats::of(value.unwrap_or(1))
}

#[derive(Debug, Default)]
struct Inner {
    admission: Admission,
    /// The lane of each engine instance the gate has seen.
    lanes: HashMap<EngineId, Lane>,
    /// The number of the next lane made.
    next_lane: u32,
    /// Where each request is told its standing.
    tell: HashMap<Ticket, watch::Sender<Standing>>,
}

impl Inner {
    fn lane_of(&mut self, engine: &EngineId, seats: Seats) -> Lane {
        let lane = match self.lanes.get(engine) {
            Some(lane) => *lane,
            None => {
                let lane = Lane(self.next_lane);
                self.next_lane = self.next_lane.saturating_add(1);
                self.lanes.insert(engine.clone(), lane);
                lane
            }
        };
        let promoted = self.admission.set_lane(lane, seats);
        self.seat(promoted);
        lane
    }

    /// Tells the promoted requests they have a seat, and the rest of their lanes where they now
    /// stand.
    fn seat(&mut self, promoted: Vec<Promoted>) {
        for Promoted { ticket, lane } in &promoted {
            self.tell_standing(*ticket, Standing::Seated);
            self.renumber(*lane);
        }
    }

    fn renumber(&self, lane: Lane) {
        for (ticket, ahead) in self.admission.waiting(lane) {
            self.tell_standing(ticket, Standing::Waiting { ahead });
        }
    }

    /// Tells `ticket` its standing, waking it only when that is news.
    fn tell_standing(&self, ticket: Ticket, standing: Standing) {
        if let Some(tell) = self.tell.get(&ticket) {
            tell.send_if_modified(|now| {
                let news = *now != standing;
                *now = standing;
                news
            });
        }
    }
}

/// The admission of every engine instance of the daemon, shared by clones.
#[derive(Debug, Clone, Default)]
pub struct Gate(Arc<Mutex<Inner>>);

impl Gate {
    fn locked(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A request of `asker` for the engine instance `engine`, which takes `seats` at once. It
    /// has a seat at once when one is free; otherwise it waits and the pass says where.
    pub fn pass(&self, engine: &EngineId, seats: Seats, asker: &Asker) -> Pass {
        let mut inner = self.locked();
        let lane = inner.lane_of(engine, seats);
        let placed = inner.admission.arrive(&[lane], Priority::from(asker));
        if let Some(placed) = placed {
            inner.renumber(placed.lane);
        }
        let standing = placed.map_or(Standing::Seated, |placed| Standing::from(placed.place));
        let (tell, listen) = watch::channel(standing);
        if let Some(placed) = placed {
            inner.tell.insert(placed.ticket, tell);
        }
        Pass {
            gate: self.clone(),
            ticket: placed.map(|placed| placed.ticket),
            listen,
        }
    }
}

/// One request's hold on a seat, or its place in the queue for one. Dropping it frees the seat
/// (or the place) and seats the next request in line.
#[derive(Debug)]
pub struct Pass {
    gate: Gate,
    ticket: Option<Ticket>,
    listen: watch::Receiver<Standing>,
}

impl Pass {
    /// Where the request stands now.
    pub fn standing(&self) -> Standing {
        *self.listen.borrow()
    }

    /// Waits until the standing changes; false when it never will.
    pub async fn moved(&mut self) -> bool {
        self.listen.changed().await.is_ok()
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        let Some(ticket) = self.ticket else { return };
        let mut inner = self.gate.locked();
        inner.tell.remove(&ticket);
        let lane = inner.admission.lane_of(ticket);
        let promoted = inner.admission.leave(ticket);
        inner.seat(promoted);
        if let Some(lane) = lane {
            inner.renumber(lane);
        }
    }
}

#[cfg(test)]
mod tests;
