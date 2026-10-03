//! The portal shape of a sheet method (`Choose`, `AddAccount`, `Reauthenticate`): the method
//! returns a Request object's path at once, and the object's `Response(response, results)`
//! signal carries the answer when the person has finished with the sheet.
//!
//! This file is the pure half, shared by the caller and the daemon: the request path, the
//! response codes and the results vardict. `await_response` (the caller's half, `pending.rs`)
//! owns the race with the signal.
//!
//! - **Path.** `/org/quire/Accounts1/request/<sender>/<token>`: the caller's unique name with
//!   the colon dropped and each dot an underscore (the portals' form), then the `handle_token`
//!   the caller put in the call's `options`. A daemon uses a valid token as given and mints one
//!   otherwise, so a caller can subscribe before it calls, by path namespace.
//! - **Code.** 0 the sheet finished (`Done`), 1 the person closed it (`Cancelled`, porter's
//!   `Refusal::Dismissed`), 2 anything else (`Other`: the results hold `refusal`, the slug of the
//!   other `Refusal`, among them `denied` for "Don't Allow").
//! - **Results.** `Choose` (code 0): the chosen candidate as `candidate_to_dbus` gives it, its
//!   fields by name plus `path` (`o`) and `label` (`s`). `AddAccount` (code 0): `account` (the
//!   exact id) and `path`. `Reauthenticate` (code 0): empty. Code 1: empty. Code 2: `refusal`.

use crate::args::Details;
use crate::codec::{candidate_from_dbus, candidate_to_dbus};
use crate::json_value::bad;
use crate::names::account_path;
use porter_core::wire::Refusal;
use porter_core::{AccountId, AccountsReply, CoreError};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// The key in a call's `options` that names the Request object's last path segment.
pub const OPTION_HANDLE_TOKEN: &str = "handle_token";

/// The interface of a Request object.
pub const REQUEST_INTERFACE: &str = "org.quire.Accounts1.Request";

/// The parent of every Request object's path segment pair.
pub const REQUEST_PATH_ROOT: &str = "/org/quire/Accounts1/request";

/// What a sheet method asked for, which decides how its `Response` is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SheetKind {
    /// `Manager.Choose`: the chooser and consent sheet.
    Choose,
    /// `Manager.AddAccount`: the add sheet.
    AddAccount,
    /// `Account.Reauthenticate`: signing in again.
    Reauthenticate,
}

/// The portal's response code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponseCode {
    /// 0: the sheet finished with an answer.
    Done,
    /// 1: the person closed it.
    Cancelled,
    /// 2: anything else; the results say what.
    Other,
}

impl ResponseCode {
    /// The `u` on the wire.
    pub fn to_wire(self) -> u32 {
        match self {
            ResponseCode::Done => 0,
            ResponseCode::Cancelled => 1,
            ResponseCode::Other => 2,
        }
    }

    /// The code a `u` stands for, or `None` for a number the portal shape does not define.
    pub fn from_wire(code: u32) -> Option<Self> {
        match code {
            0 => Some(ResponseCode::Done),
            1 => Some(ResponseCode::Cancelled),
            2 => Some(ResponseCode::Other),
            _ => None,
        }
    }
}

/// A `Response` signal's two arguments, typed.
#[derive(Debug)]
pub struct Response {
    /// The code.
    pub code: ResponseCode,
    /// The results.
    pub results: Details,
}

/// The reason key of a code 2 response.
const REFUSAL_KEY: &str = "refusal";
const PATH_KEY: &str = "path";
const LABEL_KEY: &str = "label";
const ACCOUNT_KEY: &str = "account";

fn text_value(text: &str) -> Option<OwnedValue> {
    OwnedValue::try_from(Value::from(text.to_owned())).ok()
}

fn path_value(path: &str) -> Option<OwnedValue> {
    let path = ObjectPath::try_from(path.to_owned()).ok()?;
    OwnedValue::try_from(Value::from(path)).ok()
}

fn refusal_slug(refusal: Refusal) -> String {
    match serde_json::to_value(refusal) {
        Ok(serde_json::Value::String(slug)) => slug,
        _ => String::new(),
    }
}

fn other(refusal: Refusal) -> Response {
    let reason = text_value(&refusal_slug(refusal));
    Response {
        code: ResponseCode::Other,
        results: reason
            .map(|value| Details::from([(REFUSAL_KEY.to_owned(), value)]))
            .unwrap_or_default(),
    }
}

/// The response that stands for `reply`, the answer to a `kind` sheet. A reply that does not
/// answer that kind of sheet is `Other` with `unavailable`: the daemon could not do what was
/// asked.
pub fn response_of(kind: SheetKind, reply: &AccountsReply) -> Response {
    match (kind, reply) {
        (_, AccountsReply::Refused(Refusal::Dismissed)) => Response {
            code: ResponseCode::Cancelled,
            results: Details::new(),
        },
        (_, AccountsReply::Refused(refusal)) => other(*refusal),
        (SheetKind::Choose, AccountsReply::Chosen(candidate)) => {
            let (path, label, mut results) = candidate_to_dbus(candidate);
            results.extend(path_value(path.as_str()).map(|v| (PATH_KEY.to_owned(), v)));
            results.extend(text_value(&label).map(|v| (LABEL_KEY.to_owned(), v)));
            Response {
                code: ResponseCode::Done,
                results,
            }
        }
        (SheetKind::AddAccount, AccountsReply::Added(account)) => Response {
            code: ResponseCode::Done,
            results: [
                path_value(&account_path(account)).map(|v| (PATH_KEY.to_owned(), v)),
                text_value(account.as_str()).map(|v| (ACCOUNT_KEY.to_owned(), v)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        },
        (SheetKind::Reauthenticate, AccountsReply::Reauthenticated) => Response {
            code: ResponseCode::Done,
            results: Details::new(),
        },
        _ => other(Refusal::Unavailable),
    }
}

fn take_text(results: &mut Details, key: &str) -> Result<String, CoreError> {
    let value = results
        .remove(key)
        .ok_or_else(|| bad(&format!("the results hold no `{key}`")))?;
    String::try_from(value).map_err(|_| bad(&format!("`{key}` is not a string")))
}

fn take_path(results: &mut Details, key: &str) -> Result<OwnedObjectPath, CoreError> {
    let value = results
        .remove(key)
        .ok_or_else(|| bad(&format!("the results hold no `{key}`")))?;
    OwnedObjectPath::try_from(value).map_err(|_| bad(&format!("`{key}` is not an object path")))
}

fn chosen(mut results: Details) -> Result<AccountsReply, CoreError> {
    let path = take_path(&mut results, PATH_KEY)?;
    let label = take_text(&mut results, LABEL_KEY)?;
    candidate_from_dbus((path, label, results)).map(AccountsReply::Chosen)
}

fn added(mut results: Details) -> Result<AccountsReply, CoreError> {
    let path = take_path(&mut results, PATH_KEY)?;
    let id = AccountId::parse(&take_text(&mut results, ACCOUNT_KEY)?)?;
    match account_path(&id) == path.as_str() {
        true => Ok(AccountsReply::Added(id)),
        false => Err(bad("added: the path is not the account's")),
    }
}

fn refusal_in(mut results: Details) -> Result<AccountsReply, CoreError> {
    let slug = take_text(&mut results, REFUSAL_KEY)?;
    serde_json::from_value(serde_json::Value::String(slug))
        .map(AccountsReply::Refused)
        .map_err(|e| bad(&format!("refusal: {e}")))
}

/// The reply a `Response` carries for a `kind` sheet, or why it is not one porter's daemons
/// send: an undefined code, or results that do not hold what the code promises.
pub fn reply_of(kind: SheetKind, code: u32, results: Details) -> Result<AccountsReply, CoreError> {
    match (ResponseCode::from_wire(code), kind) {
        (Some(ResponseCode::Cancelled), _) => Ok(AccountsReply::Refused(Refusal::Dismissed)),
        (Some(ResponseCode::Other), _) => refusal_in(results),
        (Some(ResponseCode::Done), SheetKind::Choose) => chosen(results),
        (Some(ResponseCode::Done), SheetKind::AddAccount) => added(results),
        (Some(ResponseCode::Done), SheetKind::Reauthenticate) => Ok(AccountsReply::Reauthenticated),
        (None, _) => Err(bad(&format!("response code {code} is not defined"))),
    }
}

/// The path segment of a bus connection's unique name: `:1.42` is `1_42`.
pub fn sender_segment(unique_name: &str) -> String {
    unique_name
        .trim_start_matches(':')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Whether `token` may be the last segment of a request path: one or more of `[A-Za-z0-9_]`,
/// at most 64.
pub fn is_handle_token(token: &str) -> bool {
    (1..=64).contains(&token.len()) && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The path under which every Request object of `sender` lives: a caller subscribes to
/// `Response` here before it calls.
pub fn request_namespace(sender: &str) -> String {
    format!("{REQUEST_PATH_ROOT}/{}", sender_segment(sender))
}

/// The Request object of `sender`'s call that carried `token`, or `None` for a token that is
/// not one.
pub fn request_path(sender: &str, token: &str) -> Option<String> {
    is_handle_token(token).then(|| format!("{}/{token}", request_namespace(sender)))
}
