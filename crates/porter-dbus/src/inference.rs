//! `org.quire.Inference1` at `/org/quire/Inference1`: inferd. A session is the fd `Open`
//! returns: the client writes `porter_infer::ClientFrame` frames (requests, `Cancel`, audio),
//! inferd writes `InferEvent` frames; memfds ride on frames as SCM_RIGHTS. Speech and
//! computer-use steps use the same fd, so this interface has no member for them.

use crate::args::{Details, NeedArg};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Inference1",
    default_service = "org.quire.Inference1",
    default_path = "/org/quire/Inference1"
)]
pub trait Inference {
    /// Whether an AI need can be met for this class, revealing no identity. `options` is the
    /// call's vardict; the reserved keys are `traceparent` (`OPTION_TRACEPARENT`), `usage`
    /// (`OPTION_USAGE`) and, for docket's daemons only, `places` and `place_models`
    /// (`OPTION_PLACES`, `OPTION_PLACE_MODELS`). With `places`, when no allowed place can serve,
    /// the call fails with `org.quire.Inference1.Error.NoAllowedPlace.<Reason>` (see
    /// `place_error_name`).
    fn availability(&self, need: &NeedArg, class: &str, options: &Details) -> zbus::Result<String>;
    /// A framed request/stream session for `need`, `class` and `tier`, pinned to one model.
    /// `options` carries `traceparent` when the caller has a trace and `usage` when the session
    /// is not interactive; unknown keys are ignored.
    fn open(
        &self,
        need: &NeedArg,
        class: &str,
        tier: &str,
        options: &Details,
    ) -> zbus::Result<OwnedFd>;
    /// Warms the engine the route would pick (no mic, no request) and answers its readiness
    /// slug (`ready`, `loading`, `loadable`, `downloading`, `downloadable`, `unavailable`); a
    /// refusal answers with its slug.
    fn prepare(
        &self,
        need: &NeedArg,
        class: &str,
        tier: &str,
        options: &Details,
    ) -> zbus::Result<String>;
    /// The caller's usage this period, by name (tokens, spend, cap).
    fn usage(&self) -> zbus::Result<Details>;
    /// Probes local runtimes again.
    fn rescan(&self) -> zbus::Result<()>;
    /// The places the assistant could run, one row per place known now: this computer, each of
    /// the person's own computers, each signed-in cloud AI account. A row is the place's id
    /// (`this-computer`, `computer:<name>`, `account:<account id>`) and a vardict with `kind`,
    /// `name`, `provider` (cloud accounts only), `models` (`a(ss)`: model id and display name,
    /// the models the place can serve now) and `ready` (`b`, data only); the key names are
    /// `PLACE_KEY_*`. Only Settings, the shell and the companion may ask; anyone else is
    /// `AccessDenied`.
    fn places(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Adds a computer of the person's own, with the models it serves, and answers its place id
    /// (`computer:<name>`). `name` is what the person calls it (the id's name is made from it);
    /// each of `models` is a catalogue model id and a vardict: `socket` (`s`, the full path of
    /// the connection on this computer that leads to the engine) or `port` (`q`, the port on this
    /// computer that does), and `key` (`s`, optional; kept in a file only the owner can read, and
    /// never given back). Only Settings may call; a refusal is
    /// `org.quire.Inference1.Error.Computer.<Name>` with a plain sentence (`COMPUTER_ERROR_PREFIX`).
    /// `EnginesChanged` follows a change.
    fn add_computer(&self, name: &str, models: Vec<(String, Details)>) -> zbus::Result<String>;
    /// Removes a computer that `AddComputer` added: `name` is its name or its place id. One
    /// written by hand in the settings file is refused (`AddedByHand`). Only Settings may call.
    fn remove_computer(&self, name: &str) -> zbus::Result<()>;
    /// The person's own computers on their Tailscale network that lend their models and are not
    /// added yet, one row each: the computer's node id first, then `name`, `models` (`a(ss)`,
    /// model id and display name) and `needs_approval` (`b`, whether the person still has to say
    /// yes on that computer); the key names are `CANDIDATE_KEY_*`. Only computers of the same
    /// user that are online, not tagged and not shared in are looked at, each at most once a
    /// minute, and only when Tailscale is an account (an empty list otherwise). Only Settings
    /// may ask.
    fn candidates(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Adds the computer `node` (a Tailscale node id from `Candidates`) as one of the person's
    /// own computers, reached over their Tailscale network, with the models it lends, and answers
    /// its place id (`computer:<name>`, the name made from the one Tailscale knows it by). Only
    /// Settings may call; a refusal is `org.quire.Inference1.Error.Computer.<Name>`.
    fn add_tailnet_computer(&self, node: &str) -> zbus::Result<String>;
    /// The computers that asked to use this computer's models or were answered, one row each:
    /// the node id first, then `name`, `state` (`approved`, `denied` or `asking`) and `since`
    /// (`x`); the key names are `GUEST_KEY_*`. Only Settings and the shell may ask.
    fn guests(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// The person's answer about the computer `node`, a word: `allow` or `deny`
    /// (`GUEST_ANSWER_ALLOW`, `GUEST_ANSWER_DENY`; any other word is `InvalidArgs`). The shell
    /// may answer a computer that is asking; Settings may answer any computer of the person's
    /// network (also a server or another person's, which is never asked about), or take back a
    /// yes (a `deny` keeps it out, and it is not asked about again).
    fn answer_guest(&self, node: &str, answer: &str) -> zbus::Result<()>;
    /// Forgets the answer about `node` (and any question it has waiting): it is asked about
    /// again the next time it wants a model, if it is one of the person's own. Only Settings
    /// may call.
    fn forget_guest(&self, node: &str) -> zbus::Result<()>;
    /// A computer began asking to use this computer's models: the person is to be asked "Let
    /// <name> use this computer's models?". The details are `name` and `since`. Broadcast: a
    /// computer's name is not secret, and only the shell and Settings can answer. A shell that
    /// was not running reads the questions waiting from `Guests` (state `asking`).
    #[zbus(signal)]
    fn guest_asks(&self, node: &str, details: Details) -> zbus::Result<()>;
    /// The answers or the questions waiting changed: a listener reads `Guests` again.
    #[zbus(signal)]
    fn guests_changed(&self) -> zbus::Result<()>;
    /// Engine state changed, or a cloud AI account appeared, went or changed state. Broadcast:
    /// engine state is not personal. Listeners re-read readiness with `Prepare` (callers) or the
    /// settings module (detent), and the places with `Places`.
    #[zbus(signal)]
    fn engines_changed(&self) -> zbus::Result<()>;
    /// The GPU's use: `idle`, `busy` or `loading`.
    #[zbus(property)]
    fn gpu(&self) -> zbus::Result<String>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct InferenceSkeleton;

#[zbus::interface(name = "org.quire.Inference1")]
impl InferenceSkeleton {
    fn availability(&self, need: NeedArg, class: String, options: Details) -> fdo::Result<String> {
        let _ = (need, class, options);
        Err(crate::introspect::frozen())
    }

    fn open(
        &self,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> fdo::Result<OwnedFd> {
        let _ = (need, class, tier, options);
        Err(crate::introspect::frozen())
    }

    fn prepare(
        &self,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> fdo::Result<String> {
        let _ = (need, class, tier, options);
        Err(crate::introspect::frozen())
    }

    fn usage(&self) -> fdo::Result<Details> {
        Err(crate::introspect::frozen())
    }

    fn rescan(&self) -> fdo::Result<()> {
        Err(crate::introspect::frozen())
    }

    fn places(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn add_computer(&self, name: String, models: Vec<(String, Details)>) -> fdo::Result<String> {
        let _ = (name, models);
        Err(crate::introspect::frozen())
    }

    fn remove_computer(&self, name: String) -> fdo::Result<()> {
        let _ = name;
        Err(crate::introspect::frozen())
    }

    fn candidates(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn add_tailnet_computer(&self, node: String) -> fdo::Result<String> {
        let _ = node;
        Err(crate::introspect::frozen())
    }

    fn guests(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn answer_guest(&self, node: String, answer: String) -> fdo::Result<()> {
        let _ = (node, answer);
        Err(crate::introspect::frozen())
    }

    fn forget_guest(&self, node: String) -> fdo::Result<()> {
        let _ = node;
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn guest_asks(
        emitter: &SignalEmitter<'_>,
        node: &str,
        details: Details,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn guests_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn engines_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    fn gpu(&self) -> fdo::Result<String> {
        Err(crate::introspect::frozen())
    }
}
