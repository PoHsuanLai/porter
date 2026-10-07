//! The open sessions: each a loopback listener task owned by the launcher connection that opened
//! it. Closing a session (or the launcher's connection going away) aborts the task, which drops
//! the listener and, with its join set, every request in flight and the provider connections
//! behind them.

use super::route::Route;
use super::token::Token;
use crate::audit::AuditOut;
use crate::clock::Clock;
use crate::engines::Engines;
use porter_core::capability::AgentProgram;
use porter_core::{AppId, DataClass};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

/// What one session's requests are served from.
pub struct Ctx {
    /// The agent program the session is for.
    pub program: AgentProgram,
    /// The app its grants, spend and audit lines are held under.
    pub app: AppId,
    /// Where requests go.
    pub route: Route,
    /// The data class of everything on the session.
    pub class: DataClass,
    /// The session's secret.
    pub token: Token,
    /// The models, the settings and the hosted reach.
    pub engines: Engines,
    /// Where the one audit line of each request goes.
    pub audit: Arc<dyn AuditOut>,
    /// Now.
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("program", &self.program)
            .field("route", &self.route)
            .field("class", &self.class)
            .finish_non_exhaustive()
    }
}

struct Live {
    owner: String,
    task: JoinHandle<()>,
}

struct Inner {
    engines: Engines,
    audit: Arc<dyn AuditOut>,
    clock: Arc<dyn Clock>,
    live: Mutex<HashMap<String, Live>>,
    /// The bus names being watched for leaving.
    watching: Mutex<HashSet<String>>,
}

/// The agent endpoints of one daemon.
#[derive(Clone)]
pub struct Agents(Arc<Inner>);

impl std::fmt::Debug for Agents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agents")
            .field("open", &self.open_count())
            .finish_non_exhaustive()
    }
}

async fn listen(listener: TcpListener, ctx: Arc<Ctx>) {
    let mut requests = JoinSet::new();
    while let Ok((stream, _)) = listener.accept().await {
        requests.spawn(super::handle::serve(Arc::clone(&ctx), stream));
        while requests.try_join_next().is_some() {}
    }
}

impl Agents {
    /// The endpoints of a daemon with these engines, audit destination and clock.
    pub fn new(engines: Engines, audit: Arc<dyn AuditOut>, clock: Arc<dyn Clock>) -> Self {
        Self(Arc::new(Inner {
            engines,
            audit,
            clock,
            live: Mutex::default(),
            watching: Mutex::default(),
        }))
    }

    /// The engines.
    pub fn engines(&self) -> &Engines {
        &self.0.engines
    }

    /// The audit destination.
    pub fn audit(&self) -> Arc<dyn AuditOut> {
        Arc::clone(&self.0.audit)
    }

    /// The clock.
    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.0.clock)
    }

    /// Notes that `owner` is being watched; false when it already was.
    pub(crate) fn begin_watching(&self, owner: &str) -> bool {
        self.0
            .watching
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(owner.to_owned())
    }

    /// Notes that `owner` is no longer watched.
    pub(crate) fn end_watching(&self, owner: &str) {
        self.0
            .watching
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(owner);
    }

    /// How many sessions are open.
    pub fn open_count(&self) -> usize {
        self.0
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Serves `ctx` on `listener` as session `id`, owned by the bus connection `owner`.
    pub(crate) fn start(&self, id: String, owner: &str, listener: TcpListener, ctx: Ctx) {
        let task = tokio::spawn(listen(listener, Arc::new(ctx)));
        self.0
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                Live {
                    owner: owner.to_owned(),
                    task,
                },
            );
    }

    /// Ends session `id` if `owner` opened it: the listener is closed and its requests in flight
    /// are cancelled before this returns. False when there is no such session of the owner's.
    pub async fn close(&self, owner: &str, id: &str) -> bool {
        let live = {
            let mut all = self.0.live.lock().unwrap_or_else(PoisonError::into_inner);
            match all.get(id) {
                Some(one) if one.owner == owner => all.remove(id),
                _ => None,
            }
        };
        match live {
            Some(live) => {
                end(live).await;
                true
            }
            None => false,
        }
    }

    /// Ends every session `owner` opened (its bus connection went away); how many.
    pub async fn close_owned_by(&self, owner: &str) -> usize {
        let gone: Vec<Live> = {
            let mut all = self.0.live.lock().unwrap_or_else(PoisonError::into_inner);
            let ids: Vec<String> = all
                .iter()
                .filter(|(_, one)| one.owner == owner)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| all.remove(id)).collect()
        };
        let count = gone.len();
        for live in gone {
            end(live).await;
        }
        count
    }
}

async fn end(live: Live) {
    live.task.abort();
    let _ = live.task.await;
}
