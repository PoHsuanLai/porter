//! The relay: one machine driven over two streams and a connector.

use crate::connect::{Connect, ConnectFault};
use crate::fault::RelayFault;
use crate::http1::HttpRelay;
use crate::imap::ImapRelay;
use crate::pop3::Pop3Relay;
use crate::sieve::SieveRelay;
use crate::smtp::SmtpRelay;
use crate::step::{Effect, Input, RelayEnd, Relaying, Side};
use porter_core::stream::ByteStream;
use porter_core::{EndpointProtocol, RelayPlan};
use std::collections::VecDeque;
use std::future::poll_fn;
use std::io;
use std::pin::pin;
use std::task::Poll;

/// How much one read takes.
const READ: usize = 16 * 1024;

/// Dials the plan's endpoint through `connect`, runs the machine of its protocol and relays
/// until either side finishes. The relay holds the plan, and so the credential, for as long as
/// it runs and never writes it anywhere but to the endpoint.
///
/// The protocol is the one the endpoint's URL scheme names. `ByteStream::read` must be cancel
/// safe (a read that loses the race for the next byte takes none), as tokio's are.
pub async fn relay<A: ByteStream, C: Connect>(plan: RelayPlan, app: A, connect: &C) -> RelayEnd {
    let origin = plan.endpoint.url.origin();
    let server = match connect.dial(&origin, plan.endpoint.tls).await {
        Ok(server) => server,
        Err(fault) => return RelayEnd::Failed(connect_fault(fault)),
    };
    let ends = Ends {
        app,
        server: Some(server),
        host: origin.host.clone(),
    };
    match origin.scheme.protocol() {
        EndpointProtocol::Imap => run(ImapRelay::new(plan), ends, connect).await,
        EndpointProtocol::Smtp => run(SmtpRelay::new(plan), ends, connect).await,
        EndpointProtocol::Http => run(HttpRelay::new(plan), ends, connect).await,
        EndpointProtocol::Sieve => run(SieveRelay::new(plan), ends, connect).await,
        EndpointProtocol::Pop3 => run(Pop3Relay::new(plan), ends, connect).await,
    }
}

fn connect_fault(fault: ConnectFault) -> RelayFault {
    match fault {
        ConnectFault::Unreachable => RelayFault::Unreachable,
        ConnectFault::Tls => RelayFault::Tls,
    }
}

/// The two ends, and the name the server's certificate is checked for.
struct Ends<A, S> {
    app: A,
    /// `None` only while an upgrade has the stream.
    server: Option<S>,
    host: String,
}

/// What one read of either side brought.
enum Got {
    App(io::Result<usize>),
    Server(io::Result<usize>),
}

/// Which sides still have something to say.
#[derive(Clone, Copy)]
struct Open {
    app: bool,
    server: bool,
}

async fn run<M: Relaying, A: ByteStream, C: Connect>(
    mut machine: M,
    mut ends: Ends<A, C::Stream>,
    connect: &C,
) -> RelayEnd {
    let mut pending: VecDeque<Input> = VecDeque::from([Input::Start]);
    let mut open = Open {
        app: true,
        server: true,
    };
    let (mut app_buf, mut server_buf) = (vec![0u8; READ], vec![0u8; READ]);
    let mut turn = false;
    let end = loop {
        let input = match pending.pop_front() {
            Some(input) => input,
            None if !open.app && !open.server => break RelayEnd::Finished,
            None => {
                turn = !turn;
                let got = next_read(&mut ends, open, (&mut app_buf, &mut server_buf), turn).await;
                match got {
                    Got::App(Ok(n)) if n > 0 => Input::Bytes {
                        from: Side::App,
                        data: app_buf[..n].to_vec(),
                    },
                    Got::Server(Ok(n)) if n > 0 => Input::Bytes {
                        from: Side::Server,
                        data: server_buf[..n].to_vec(),
                    },
                    Got::App(_) => {
                        open.app = false;
                        Input::Closed(Side::App)
                    }
                    Got::Server(_) => {
                        open.server = false;
                        Input::Closed(Side::Server)
                    }
                }
            }
        };
        let (next, effects) = machine.step(input);
        machine = next;
        let mut ended = None;
        for effect in effects {
            match carry_out(effect, &mut ends, connect).await {
                Carried::Done => {}
                Carried::TlsUp => pending.push_back(Input::TlsReady),
                Carried::Ended(end) => {
                    ended = Some(end);
                    break;
                }
            }
        }
        if let Some(end) = ended {
            break end;
        }
    };
    // Whatever the outcome, both ends are finished with; a failed shutdown has no one to tell.
    let _ = ends.app.shutdown().await;
    if let Some(server) = ends.server.as_mut() {
        let _ = server.shutdown().await;
    }
    end
}

enum Carried {
    Done,
    TlsUp,
    Ended(RelayEnd),
}

async fn carry_out<A: ByteStream, C: Connect>(
    effect: Effect,
    ends: &mut Ends<A, C::Stream>,
    connect: &C,
) -> Carried {
    match effect {
        Effect::Send {
            to: Side::App,
            data,
        } => match ends.app.write_all(&data).await {
            Ok(()) => Carried::Done,
            // The app went away: nothing left to relay to.
            Err(_) => Carried::Ended(RelayEnd::Finished),
        },
        Effect::Send {
            to: Side::Server,
            data,
        } => match ends.server.as_mut() {
            Some(server) => match server.write_all(&data).await {
                Ok(()) => Carried::Done,
                Err(_) => Carried::Ended(RelayEnd::Failed(RelayFault::Unreachable)),
            },
            None => Carried::Ended(RelayEnd::Failed(RelayFault::Protocol)),
        },
        Effect::StartTls => {
            let Some(stream) = ends.server.take() else {
                return Carried::Ended(RelayEnd::Failed(RelayFault::Protocol));
            };
            // A failed upgrade leaves no stream: the relay ends there.
            match connect.upgrade(stream, &ends.host).await {
                Ok(upgraded) => {
                    ends.server = Some(upgraded);
                    Carried::TlsUp
                }
                Err(fault) => Carried::Ended(RelayEnd::Failed(connect_fault(fault))),
            }
        }
        Effect::Close(end) => Carried::Ended(end),
    }
}

async fn next_read<A: ByteStream, S: ByteStream>(
    ends: &mut Ends<A, S>,
    open: Open,
    (app_buf, server_buf): (&mut [u8], &mut [u8]),
    app_first: bool,
) -> Got {
    let mut app = pin!(async {
        match open.app {
            true => ends.app.read(app_buf).await,
            false => std::future::pending().await,
        }
    });
    let mut server = pin!(async {
        match (open.server, ends.server.as_mut()) {
            (true, Some(server)) => server.read(server_buf).await,
            _ => std::future::pending().await,
        }
    });
    poll_fn(|cx| {
        let polls: [Side; 2] = match app_first {
            true => [Side::App, Side::Server],
            false => [Side::Server, Side::App],
        };
        for side in polls {
            let ready = match side {
                Side::App => app.as_mut().poll(cx).map(Got::App),
                Side::Server => server.as_mut().poll(cx).map(Got::Server),
            };
            if let Poll::Ready(got) = ready {
                return Poll::Ready(got);
            }
        }
        Poll::Pending
    })
    .await
}
