//! The caller's half of a sheet method: the race between the method's reply and the `Response`
//! signal. A daemon may emit the signal before the caller has read the reply (the person
//! answered at once, a scripted prompter), so the caller listens first:
//!
//! 1. [`Sheet::subscribe`]: a match rule on the Request interface under the caller's own path
//!    namespace, and one on the daemon's name owner, then a fresh `handle_token`;
//! 2. the call, with [`Sheet::options`] in its `options` (it returns the Request object's path);
//! 3. [`Sheet::response`] with that path: signals that came in meanwhile are already queued; it
//!    gives the raw `(response, results)`, which `reply_of` reads.
//!
//! A signal counts only when it comes from the connection that owns `org.quire.Accounts1` and
//! from the returned path, so another process on the bus cannot answer for the person. If that
//! owner leaves the bus before it answered, the wait ends in [`SheetError::Gone`] instead of
//! never. [`Closer`] is the caller giving up (`Request.Close`): the task that waits for a sheet is
//! dropped when the app's own call is, and the sheet should not stay on the screen.

use crate::args::Details;
use crate::names::ACCOUNTS_BUS;
use crate::request::RequestProxy;
use crate::sheet::{OPTION_HANDLE_TOKEN, REQUEST_INTERFACE, request_namespace, request_path};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use zbus::export::futures_core::Stream;
use zbus::fdo::{DBusProxy, NameOwnerChangedStream};
use zbus::message::Type;
use zbus::names::UniqueName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{MatchRule, Message, MessageStream};

use crate::BusConnection;
use crate::BusError;

/// Why no `Response` came.
#[derive(Debug)]
pub enum SheetError {
    /// The bus refused a step.
    Bus(BusError),
    /// The daemon left the bus before it answered.
    Gone,
    /// A signal from the daemon at the Request path that is not `Response(u, a{sv})`.
    Unreadable,
}

impl From<BusError> for SheetError {
    fn from(error: BusError) -> Self {
        SheetError::Bus(error)
    }
}

/// A listener for one sheet's `Response`, subscribed before the call is made.
#[derive(Debug)]
pub struct Sheet {
    connection: BusConnection,
    token: String,
    responses: MessageStream,
    owners: NameOwnerChangedStream,
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

/// A token no other call of this process shares.
fn fresh_token() -> String {
    format!(
        "porter_{}_{}",
        std::process::id(),
        NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
    )
}

fn failure(why: &str) -> BusError {
    BusError::Failure(why.to_owned())
}

impl Sheet {
    /// Starts listening, under a token of its own. Call this before the sheet method.
    pub async fn subscribe(connection: &BusConnection) -> Result<Self, BusError> {
        let unique = connection
            .unique_name()
            .ok_or_else(|| failure("the connection has no unique name"))?;
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(REQUEST_INTERFACE)?
            .member("Response")?
            .path_namespace(request_namespace(unique.as_str()))?
            .build();
        let responses = MessageStream::for_match_rule(rule, connection, None).await?;
        let owners = DBusProxy::new(connection)
            .await?
            .receive_name_owner_changed_with_args(&[(0, ACCOUNTS_BUS)])
            .await?;
        Ok(Self {
            connection: connection.clone(),
            token: fresh_token(),
            responses,
            owners,
        })
    }

    /// The `options` dictionary of the sheet method: it names the Request object's path.
    pub fn options(&self) -> Details {
        OwnedValue::try_from(Value::from(self.token.clone()))
            .map(|token| Details::from([(OPTION_HANDLE_TOKEN.to_owned(), token)]))
            .unwrap_or_default()
    }

    /// Where the Request object is when the daemon honours [`Sheet::options`]; `Closer` for a
    /// call that was dropped before it returned its path.
    pub fn expected_path(&self) -> Option<OwnedObjectPath> {
        let unique = self.connection.unique_name()?;
        let path = request_path(unique.as_str(), &self.token)?;
        ObjectPath::try_from(path).ok().map(OwnedObjectPath::from)
    }

    /// A handle that closes the sheet at `path` (`None`: the expected one).
    pub fn closer(&self, path: Option<OwnedObjectPath>) -> Option<Closer> {
        path.or_else(|| self.expected_path()).map(|path| Closer {
            connection: self.connection.clone(),
            path,
        })
    }

    /// Waits for the `Response` of the Request object at `path`, the path the sheet method
    /// returned.
    pub async fn response(&mut self, path: &ObjectPath<'_>) -> Result<(u32, Details), SheetError> {
        let owner = self.owner().await?;
        loop {
            let next = std::future::poll_fn(|cx| self.poll_step(cx)).await;
            match next {
                Step::Message(message) if from_owner(&message, &owner, path) => {
                    return response_in(&message).ok_or(SheetError::Unreadable);
                }
                Step::Message(_) => {}
                Step::OwnerChanged { left } if left == owner.as_str() => {
                    return Err(SheetError::Gone);
                }
                Step::OwnerChanged { .. } => {}
                Step::Ended => return Err(SheetError::Gone),
            }
        }
    }

    /// The connection that owns the daemon's name now; the call that returned a path started it
    /// if it was not running.
    async fn owner(&self) -> Result<String, SheetError> {
        let proxy = DBusProxy::new(&self.connection).await?;
        let name = zbus::names::BusName::try_from(ACCOUNTS_BUS).map_err(BusError::from)?;
        let owner = proxy.get_name_owner(name).await.map_err(BusError::from)?;
        Ok(owner.as_str().to_owned())
    }

    fn poll_step(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Step> {
        if let Poll::Ready(item) = Pin::new(&mut self.responses).poll_next(cx) {
            return Poll::Ready(match item {
                Some(Ok(message)) => Step::Message(message),
                _ => Step::Ended,
            });
        }
        match Pin::new(&mut self.owners).poll_next(cx) {
            Poll::Ready(Some(signal)) => Poll::Ready(
                signal
                    .args()
                    .ok()
                    .and_then(|args| args.old_owner().as_ref().map(|o| o.as_str().to_owned()))
                    .map_or(
                        Step::OwnerChanged {
                            left: String::new(),
                        },
                        |left| Step::OwnerChanged { left },
                    ),
            ),
            Poll::Ready(None) => Poll::Ready(Step::Ended),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// What the listener woke for.
enum Step {
    Message(Message),
    /// A name owner left; the unique name of the one that did (empty if none did).
    OwnerChanged {
        left: String,
    },
    Ended,
}

fn from_owner(message: &Message, owner: &str, path: &ObjectPath<'_>) -> bool {
    let header = message.header();
    header.sender().map(UniqueName::as_str) == Some(owner)
        && header.path().map(ObjectPath::as_str) == Some(path.as_str())
}

fn response_in(message: &Message) -> Option<(u32, Details)> {
    message.body().deserialize().ok()
}

/// Gives up on a sheet: `Request.Close`, after which no `Response` follows.
#[derive(Debug)]
pub struct Closer {
    connection: BusConnection,
    path: OwnedObjectPath,
}

impl Closer {
    /// Closes the sheet. A sheet already gone (answered, or never started) is not an error.
    pub async fn close(self) {
        let Ok(builder) = RequestProxy::builder(&self.connection).path(self.path) else {
            return;
        };
        if let Ok(proxy) = builder.build().await {
            let _ = proxy.close().await;
        }
    }
}
