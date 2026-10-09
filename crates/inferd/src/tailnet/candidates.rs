//! The person's own computers that could be added: online, theirs (not a tagged server, not
//! shared in, not another person's), and answering the hello of a computer that lends its
//! models at the one port.
//!
//! A look at a computer is made only when somebody reads the list (never on a timer), for a
//! computer that accountd lists (so never while Tailscale is not an account the person
//! added), and at most once a minute for each: what the last look found is what is answered in
//! between. A computer that did not answer is not looked at again for longer and longer, up to
//! eight minutes, so one that is off or does not lend costs next to nothing. Every connection
//! first asks Tailscale who is at the address (`porter_tailnet::Dialer`).

use super::machines::Machines;
use porter_core::{MachineOwner, NodeId};
use porter_tailnet::{Dialer, LentModel, greet};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::Instant;

/// The shortest time between two looks at one computer.
const EVERY: Duration = Duration::from_secs(60);

/// How many times longer a computer that does not answer is left alone, at most: eight minutes.
const MOST_BACKOFF: u32 = 3;

/// How long one look may take before the computer is taken not to answer. Longer than the time a
/// computer that is up needs, shorter than the time a person would wait on a list; a look that
/// timed out is made again later.
const LOOK_WAIT: Duration = Duration::from_secs(15);

/// A computer that lends its models and could be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Its stable Tailscale id.
    pub node: NodeId,
    /// The name people call it by.
    pub name: String,
    /// The models it lends.
    pub models: Vec<LentModel>,
    /// Whether the person must still say yes on that computer.
    pub needs_approval: bool,
}

#[derive(Debug)]
struct Look {
    /// Not looked at again before.
    next: Instant,
    /// How many looks in a row found nothing.
    failures: u32,
    /// What the last look that found something found.
    found: Option<(Vec<LentModel>, bool)>,
}

/// The looks at the person's computers and what they found.
#[derive(Debug)]
pub struct Candidates {
    dialer: Dialer,
    machines: Arc<dyn Machines>,
    every: Duration,
    looks: Mutex<BTreeMap<NodeId, Look>>,
}

impl Candidates {
    /// Looks through `dialer` at the computers `machines` lists, each at most once a minute.
    pub fn new(dialer: Dialer, machines: Arc<dyn Machines>) -> Self {
        Self::every(dialer, machines, EVERY)
    }

    /// The same with another shortest time between two looks at a computer (tests shorten it).
    pub fn every(dialer: Dialer, machines: Arc<dyn Machines>, every: Duration) -> Self {
        Self {
            dialer,
            machines,
            every,
            looks: Mutex::default(),
        }
    }

    /// The computers that lend, by name. A computer whose look is due is looked at now, all of
    /// them at once.
    pub async fn list(&self) -> Vec<Candidate> {
        let machines = self.machines.machines().await;
        let mine: Vec<_> = machines
            .into_iter()
            .filter(|machine| machine.owner == MachineOwner::Mine && machine.online)
            .collect();
        let mut looks = self.looks.lock().await;
        looks.retain(|node, _| mine.iter().any(|machine| machine.node == *node));
        let now = Instant::now();
        let mut due = JoinSet::new();
        for machine in &mine {
            if looks.get(&machine.node).is_none_or(|look| look.next <= now) {
                let (dialer, node, host) = (
                    self.dialer.clone(),
                    machine.node.clone(),
                    machine.name.clone(),
                );
                due.spawn(async move {
                    let seen = tokio::time::timeout(LOOK_WAIT, async {
                        let stream = dialer.connect(&node).await.ok()?;
                        greet(stream, &host).await.ok()
                    })
                    .await
                    .ok()
                    .flatten();
                    (node, seen)
                });
            }
        }
        while let Some(done) = due.join_next().await {
            let Ok((node, seen)) = done else { continue };
            let before = looks.remove(&node);
            let look = match seen {
                Some(hello) => Look {
                    next: Instant::now() + self.every,
                    failures: 0,
                    found: Some((hello.models().to_vec(), hello.needs_approval())),
                },
                None => {
                    let failures = before.as_ref().map_or(0, |look| look.failures) + 1;
                    let pause = self.every * 2_u32.pow((failures - 1).min(MOST_BACKOFF));
                    Look {
                        next: Instant::now() + pause,
                        failures,
                        // A computer that stopped answering is not offered.
                        found: None,
                    }
                }
            };
            looks.insert(node, look);
        }
        let mut found: Vec<Candidate> = mine
            .iter()
            .filter_map(|machine| {
                let (models, needs_approval) = looks.get(&machine.node)?.found.clone()?;
                Some(Candidate {
                    node: machine.node.clone(),
                    name: machine.name.clone(),
                    models,
                    needs_approval,
                })
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }
}
