//! The seats of the engines the daemon runs, shared by every session: a [`Gate`] holds the pure
//! admission state ([`crate::admit`]) behind a lock and tells each waiting request when it has
//! its turn and how many stand ahead of it. A request holds a [`Pass`]; dropping the pass (the
//! request ended, was cancelled, failed, or its task was aborted) frees the seat.
//!
//! The lock is held for a few map operations and never across an await.

use crate::admit::{Admission, Lane, Place, Priority, Promoted, Seats, Ticket};
use engine_spec::engine;
use engine_supervisor::EngineId;
use model_catalog::{EngineArg, EngineKind};
use porter_ids::Count;
use porter_infer::Asker;
use std::collections::{HashMap, HashSet};
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
    let names = engine(kind).seat_flags().unwrap_or_default();
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
    /// Who asked for each request.
    by: HashMap<Ticket, Asker>,
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

    fn engine_of(&self, lane: Lane) -> Option<EngineId> {
        self.lanes
            .iter()
            .find(|(_, one)| **one == lane)
            .map(|(engine, _)| engine.clone())
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
        self.pass_among(&[(engine.clone(), seats)], asker)
    }

    /// A request of `asker` for a model served by several engine instances, each with its seats:
    /// it goes to an instance with a free seat, else to the one where fewest stand ahead of it.
    /// The pass says which instance it is on.
    pub fn pass_among(&self, instances: &[(EngineId, Seats)], asker: &Asker) -> Pass {
        let mut inner = self.locked();
        let lanes: Vec<Lane> = instances
            .iter()
            .map(|(engine, seats)| inner.lane_of(engine, *seats))
            .collect();
        let placed = inner.admission.arrive(&lanes, Priority::from(asker));
        let engine = placed
            .and_then(|placed| inner.engine_of(placed.lane))
            .or_else(|| instances.first().map(|(engine, _)| engine.clone()));
        if let Some(placed) = placed {
            inner.renumber(placed.lane);
        }
        let standing = placed.map_or(Standing::Seated, |placed| Standing::from(placed.place));
        let (tell, listen) = watch::channel(standing);
        if let Some(placed) = placed {
            inner.tell.insert(placed.ticket, tell);
            inner.by.insert(placed.ticket, asker.clone());
        }
        Pass {
            gate: self.clone(),
            engine,
            ticket: placed.map(|placed| placed.ticket),
            listen,
        }
    }
}

impl Gate {
    /// How many helpers have a request seated or waiting on the engine instance `engine`.
    pub fn helpers_using(&self, engine: &EngineId) -> Count {
        let inner = self.locked();
        let Some(lane) = inner.lanes.get(engine) else {
            return Count(0);
        };
        let helpers: HashSet<&Asker> = inner
            .admission
            .tickets(*lane)
            .iter()
            .filter_map(|ticket| inner.by.get(ticket))
            .filter(|asker| matches!(asker, Asker::Helper(_)))
            .collect();
        Count(u32::try_from(helpers.len()).unwrap_or(u32::MAX))
    }

    /// The engine instances a helper other than `asker` has a request on now, seated or waiting.
    pub fn engines_of_other_helpers(&self, asker: &Asker) -> Vec<EngineId> {
        let inner = self.locked();
        let other_helper = |ticket: &Ticket| {
            inner
                .by
                .get(ticket)
                .is_some_and(|by| matches!(by, Asker::Helper(_)) && by != asker)
        };
        inner
            .lanes
            .iter()
            .filter(|(_, lane)| inner.admission.tickets(**lane).iter().any(other_helper))
            .map(|(engine, _)| engine.clone())
            .collect()
    }

    /// How many helpers have requests anywhere, and how many requests wait anywhere.
    pub fn totals(&self) -> (Count, Count) {
        let inner = self.locked();
        let helpers: HashSet<&Asker> = inner
            .by
            .values()
            .filter(|asker| matches!(asker, Asker::Helper(_)))
            .collect();
        let waiting: usize = inner
            .lanes
            .values()
            .map(|lane| inner.admission.waiting(*lane).len())
            .sum();
        let count = |number: usize| Count(u32::try_from(number).unwrap_or(u32::MAX));
        (count(helpers.len()), count(waiting))
    }

    /// How many requests wait for a seat on the engine instance `engine`.
    pub fn waiting_on(&self, engine: &EngineId) -> Count {
        let inner = self.locked();
        let waiting = inner
            .lanes
            .get(engine)
            .map_or(0, |lane| inner.admission.waiting(*lane).len());
        Count(u32::try_from(waiting).unwrap_or(u32::MAX))
    }
}

/// One request's hold on a seat, or its place in the queue for one. Dropping it frees the seat
/// (or the place) and seats the next request in line.
#[derive(Debug)]
pub struct Pass {
    gate: Gate,
    /// The engine instance the request is on (none only for a request given no instance).
    engine: Option<EngineId>,
    ticket: Option<Ticket>,
    listen: watch::Receiver<Standing>,
}

impl Pass {
    /// The engine instance the request is on.
    pub fn engine(&self) -> Option<&EngineId> {
        self.engine.as_ref()
    }

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
        inner.by.remove(&ticket);
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
