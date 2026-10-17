//! Person-first admission: who takes the next seat of a model's engine, and where a request
//! waits when every seat is busy. Pure: state and a decision in, state and a decision out; no
//! clock, no IO. [`crate::seats`] holds the state behind a lock and wakes the waiters.
//!
//! A *lane* is one running instance of an engine; a model with several instances has several
//! lanes. A lane has a number of *seats* (parallel requests the engine takes at once: vLLM's
//! `--max-num-seqs`, llama.cpp's `--parallel`). A request takes a free seat or waits in its
//! lane's queue. The person's own turn goes ahead of every helper's; helpers keep the order they
//! asked in.

use porter_core::Count;
use porter_infer::Asker;
use std::collections::{BTreeMap, BTreeSet};

/// One running instance of an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Lane(pub u32);

/// One request's place in the order of arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ticket(u64);

/// Who goes first. The variants are in the order they are served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    /// The person's own turn.
    Person,
    /// A turn of one of the person's helpers.
    Helper,
}

impl From<&Asker> for Priority {
    fn from(asker: &Asker) -> Self {
        match asker {
            Asker::Person => Priority::Person,
            Asker::Helper(_) => Priority::Helper,
        }
    }
}

/// How many requests an engine instance takes at once; at least one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seats(u32);

impl Seats {
    /// `count` seats, or one when `count` is none.
    pub fn of(count: u32) -> Self {
        Self(count.max(1))
    }

    fn capacity(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// It has a seat and may go to the engine.
    Seated,
    /// It waits; `ahead` requests are served before it.
    Waiting {
        /// How many are ahead of it in its lane.
        ahead: Count,
    },
}

/// A request placed in a lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    /// Its ticket, to leave with.
    pub ticket: Ticket,
    /// The lane it is in.
    pub lane: Lane,
    /// Its seat or its place in the queue.
    pub place: Place,
}

/// A waiting request that got a seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Promoted {
    /// Its ticket.
    pub ticket: Ticket,
    /// The lane.
    pub lane: Lane,
}

/// What a lane held when it went away.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Orphans {
    /// Requests that had a seat (in flight on the instance).
    pub seated: Vec<(Ticket, Priority)>,
    /// Requests that were waiting, in the order they were served.
    pub waiting: Vec<(Ticket, Priority)>,
}

#[derive(Debug)]
struct LaneState {
    seats: Seats,
    seated: BTreeSet<Ticket>,
    /// Ordered by priority, then by arrival: the first is served next.
    waiting: BTreeSet<(Priority, Ticket)>,
}

impl LaneState {
    fn free(&self) -> bool {
        self.seated.len() < self.seats.capacity()
    }

    /// How many waiting requests a new one of `priority` would stand behind.
    fn ahead_of(&self, priority: Priority) -> usize {
        self.waiting
            .iter()
            .filter(|(one, _)| *one <= priority)
            .count()
    }
}

/// The seats and queues of every lane.
#[derive(Debug, Default)]
pub struct Admission {
    lanes: BTreeMap<Lane, LaneState>,
    at: BTreeMap<Ticket, (Lane, Priority)>,
    next: u64,
}

fn count_of(number: usize) -> Count {
    Count(u32::try_from(number).unwrap_or(u32::MAX))
}

impl Admission {
    /// Makes `lane` exist with `seats` seats, or changes its seats; requests that were waiting
    /// and now fit are seated.
    pub fn set_lane(&mut self, lane: Lane, seats: Seats) -> Vec<Promoted> {
        self.lanes
            .entry(lane)
            .or_insert_with(|| LaneState {
                seats,
                seated: BTreeSet::new(),
                waiting: BTreeSet::new(),
            })
            .seats = seats;
        self.fill(lane)
    }

    /// A request of `priority` arrives for a model served by `lanes`. It goes to a lane with a
    /// free seat (the least busy), else to the lane where fewest requests stand ahead of it.
    /// `None` when none of `lanes` exists.
    pub fn arrive(&mut self, lanes: &[Lane], priority: Priority) -> Option<Placed> {
        let ticket = Ticket(self.next);
        self.next = self.next.saturating_add(1);
        self.place(ticket, lanes, priority)
    }

    /// A request that lost its lane (see [`Admission::drop_lane`]) is placed again among `lanes`,
    /// keeping its arrival order.
    pub fn rehome(&mut self, ticket: Ticket, priority: Priority, lanes: &[Lane]) -> Option<Placed> {
        self.place(ticket, lanes, priority)
    }

    /// A request is over (finished, cancelled or failed), or gives up waiting: its seat or its
    /// place in the queue is freed, and the next waiting requests of the lane are seated.
    pub fn leave(&mut self, ticket: Ticket) -> Vec<Promoted> {
        let Some((lane, priority)) = self.at.remove(&ticket) else {
            return Vec::new();
        };
        if let Some(state) = self.lanes.get_mut(&lane) {
            state.seated.remove(&ticket);
            state.waiting.remove(&(priority, ticket));
        }
        self.fill(lane)
    }

    /// An instance went away: its lane is removed and everything it held is handed back.
    pub fn drop_lane(&mut self, lane: Lane) -> Orphans {
        let Some(state) = self.lanes.remove(&lane) else {
            return Orphans::default();
        };
        let priority_of = |ticket: &Ticket| self.at.get(ticket).map(|(_, priority)| *priority);
        let seated: Vec<(Ticket, Priority)> = state
            .seated
            .iter()
            .filter_map(|ticket| Some((*ticket, priority_of(ticket)?)))
            .collect();
        let waiting: Vec<(Ticket, Priority)> = state
            .waiting
            .iter()
            .map(|(priority, ticket)| (*ticket, *priority))
            .collect();
        for (ticket, _) in seated.iter().chain(&waiting) {
            self.at.remove(ticket);
        }
        Orphans { seated, waiting }
    }

    /// The waiting requests of `lane` and how many stand ahead of each, in the order they are
    /// served.
    pub fn waiting(&self, lane: Lane) -> Vec<(Ticket, Count)> {
        self.lanes.get(&lane).map_or_else(Vec::new, |state| {
            state
                .waiting
                .iter()
                .enumerate()
                .map(|(index, (_, ticket))| (*ticket, count_of(index)))
                .collect()
        })
    }

    /// The lane `ticket` is in; `None` when it is not in one.
    pub fn lane_of(&self, ticket: Ticket) -> Option<Lane> {
        self.at.get(&ticket).map(|(lane, _)| *lane)
    }

    /// How many requests stand ahead of `ticket`; `None` when it is seated or unknown.
    pub fn ahead(&self, ticket: Ticket) -> Option<Count> {
        let (lane, _) = self.at.get(&ticket)?;
        self.waiting(*lane)
            .into_iter()
            .find_map(|(one, ahead)| (one == ticket).then_some(ahead))
    }

    /// The requests seated or waiting in `lane`.
    pub fn tickets(&self, lane: Lane) -> Vec<Ticket> {
        self.lanes.get(&lane).map_or_else(Vec::new, |state| {
            state
                .seated
                .iter()
                .copied()
                .chain(state.waiting.iter().map(|(_, ticket)| *ticket))
                .collect()
        })
    }

    /// How many requests are seated or waiting in `lane`.
    pub fn load(&self, lane: Lane) -> Count {
        count_of(
            self.lanes
                .get(&lane)
                .map_or(0, |state| state.seated.len() + state.waiting.len()),
        )
    }

    fn choose(&self, lanes: &[Lane], priority: Priority) -> Option<Lane> {
        lanes
            .iter()
            .filter_map(|lane| Some((*lane, self.lanes.get(lane)?)))
            .min_by_key(|(lane, state)| {
                (
                    !state.free(),
                    state.ahead_of(priority),
                    state.seated.len(),
                    *lane,
                )
            })
            .map(|(lane, _)| lane)
    }

    fn place(&mut self, ticket: Ticket, lanes: &[Lane], priority: Priority) -> Option<Placed> {
        let lane = self.choose(lanes, priority)?;
        let state = self.lanes.get_mut(&lane)?;
        let place = if state.free() {
            state.seated.insert(ticket);
            Place::Seated
        } else {
            let ahead = count_of(state.ahead_of(priority));
            state.waiting.insert((priority, ticket));
            Place::Waiting { ahead }
        };
        self.at.insert(ticket, (lane, priority));
        Some(Placed {
            ticket,
            lane,
            place,
        })
    }

    fn fill(&mut self, lane: Lane) -> Vec<Promoted> {
        let Some(state) = self.lanes.get_mut(&lane) else {
            return Vec::new();
        };
        let mut promoted = Vec::new();
        while state.free() {
            let Some((_, ticket)) = state.waiting.pop_first() else {
                break;
            };
            state.seated.insert(ticket);
            promoted.push(Promoted { ticket, lane });
        }
        promoted
    }
}

#[cfg(test)]
mod tests;
