//! How long a conversation and a reply of a local model may be. The catalogue says what one
//! engine line serves (`context`, `max_output`) and, where it knows them, the model's own limits
//! (`context_max`, `max_output_max`, `context_min`). Unless the person set a length, the window
//! an engine starts with is the largest one the card's free memory holds, and the reply a model
//! may write is bounded by what the model supports and by that window.
//!
//! Everything here is pure: the numbers come in, a number comes out. Who reads the card's free
//! memory, and when, is the engine starter's (inferd's `supervise`).

use super::{EngineConfig, LocalModel, host_env};
use engine_supervisor::{UnitSpec, Window, command_with_window};
use model_catalog::{ContextArg, EngineArg, MiB, TextOut, VramEstimate};
use model_provider::Tokens;

/// A window is chosen in whole blocks of this many tokens, so a figure is the same size the
/// catalogue writes.
const BLOCK: u32 = 1024;

/// The lengths the person set; `None` is Automatic. They apply to every model on this computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lengths {
    /// The longest conversation: the window an engine is started with.
    pub conversation: Option<Tokens>,
    /// The longest reply a model may write.
    pub reply: Option<Tokens>,
}

/// The window to start an engine with.
///
/// * The person's own length wins, but never past the model's largest (`context_max`).
/// * Otherwise it is the largest window whose memory fits in `free` (the card's memory left for
///   this engine): at most `context_max` (the served `context` when the catalogue states none),
///   cut down to whole blocks, and never below the served `context` or the model's smallest
///   useful window (`context_min`).
/// * When `free` is not known, the entry's memory figure for a window is zero, or even the
///   smallest window does not fit, it is the served `context`: the engine starts as it always
///   did.
///
/// The memory of a window is the entry's own `VramEstimate::need` (a thousand tokens to the
/// figure), so a window chosen here is one the supervisor's budget agrees fits.
pub fn choose_window(
    text: &TextOut,
    vram: &VramEstimate,
    free: Option<MiB>,
    own: Option<Tokens>,
) -> Tokens {
    let served = text.context;
    let largest = text.context_max.unwrap_or(served).max(served);
    if let Some(own) = own {
        return own.min(largest);
    }
    let Some(free) = free else {
        return served;
    };
    let per_thousand = u64::from(vram.kv_per_1k_ctx.0);
    if per_thousand == 0 {
        return served;
    }
    let room = free
        .0
        .saturating_sub(vram.weights.0)
        .saturating_sub(vram.overhead.0);
    let fits = u64::from(room) * 1000 / per_thousand;
    let fits = u32::try_from(fits).unwrap_or(u32::MAX) / BLOCK * BLOCK;
    let chosen = largest.0.min(fits);
    let floor = served.0.max(text.context_min.map_or(0, |min| min.0));
    if chosen < floor {
        served
    } else {
        Tokens(chosen)
    }
}

/// The most a reply may write: what the model supports (`max_output_max`, or the served
/// `max_output` when the catalogue states none), or the person's own length when it is less.
/// Never more than half the window, so a prompt still has room. A window of zero (not known)
/// bounds nothing.
pub fn reply_ceiling(text: &TextOut, lengths: Lengths, window: Option<Tokens>) -> Tokens {
    let supported = text
        .max_output_max
        .unwrap_or(text.max_output)
        .max(text.max_output);
    let most = lengths.reply.map_or(supported, |own| own.min(supported));
    match window {
        Some(window) if window.0 > 0 => most.min(Tokens(window.0 / 2)),
        _ => most,
    }
}

/// The memory an engine line gives itself out of a card of `total`, when its `args` say
/// (`--gpu-memory-utilization 0.75`, or `=0.75`): vLLM takes that share of the whole card and
/// fits its weights and window inside it, so a window past it does not start. None when the
/// line says nothing, or says a share that is not one.
pub fn share_of(args: &[EngineArg], total: MiB) -> Option<MiB> {
    let said = args.iter().enumerate().find_map(|(at, arg)| {
        match arg.0.strip_prefix("--gpu-memory-utilization") {
            Some("") => args.get(at + 1).map(|value| value.0.as_str()),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        }
    })?;
    let share: f64 = said
        .parse()
        .ok()
        .filter(|share| *share > 0.0 && *share <= 1.0)?;
    // A share of at most one of a card's MiB fits a u32.
    Some(MiB((f64::from(total.0) * share) as u32))
}

impl LocalModel {
    /// Whether the window of this model's engine is chosen when the engine starts: a launched
    /// chat engine that takes its window as a launch flag. An engine that takes its window on
    /// each request (`ContextArg::PerRequest`, Ollama's `num_ctx`) is one the person runs and
    /// inferd does not launch, so it is not sized here and no request carries a window yet.
    pub fn sizable(&self) -> bool {
        self.attached.is_none()
            && self.loopback.is_none()
            && self.cassette.is_none()
            && matches!(self.profile.context_arg, Some(ContextArg::Flag(_)))
            && self
                .entry
                .capabilities
                .text_out
                .as_ref()
                .is_some_and(|text| text.context.0 > 0)
    }

    /// The window to start this model's engine with when `free` of a card of `total` is left
    /// for it; none for a model whose window is not chosen ([`Self::sizable`]).
    pub fn window(
        &self,
        lengths: Lengths,
        free: Option<MiB>,
        total: Option<MiB>,
    ) -> Option<Tokens> {
        let text = self.entry.capabilities.text_out.as_ref()?;
        let room = match (free, total.and_then(|total| self.share_of(total))) {
            (Some(free), Some(share)) => Some(MiB(free.0.min(share.0))),
            (free, _) => free,
        };
        self.sizable()
            .then(|| choose_window(text, &self.entry.vram, room, lengths.conversation))
    }

    /// The memory this model's engine line gives itself out of a card of `total`: see
    /// [`share_of`].
    pub fn share_of(&self, total: MiB) -> Option<MiB> {
        share_of(&self.profile.args, total)
    }

    /// The memory this model's engine takes with `window` (the served window's, in
    /// `spec.need`, when it has none).
    pub fn need_with(&self, window: Option<Tokens>) -> MiB {
        window.map_or(self.spec.need, |window| self.entry.vram.need(window))
    }

    /// The unit that starts this model's engine with `window`; the same as the one in `spec`
    /// for `Window::Serve`. `None` when `engines` lists no program for its kind.
    pub fn unit_with(&self, engines: &EngineConfig, window: Window) -> Option<UnitSpec> {
        let unit = command_with_window(
            &self.entry,
            &self.profile,
            &engines.paths(),
            &self.socket,
            window,
        )
        .ok()?;
        Some(host_env(unit, self.profile.kind, engines))
    }

    /// The most a reply of this model may write when its engine runs with `window`: see
    /// [`reply_ceiling`]. A model with no text to write has no ceiling.
    pub fn reply_ceiling(&self, lengths: Lengths, window: Option<Tokens>) -> Tokens {
        match &self.entry.capabilities.text_out {
            Some(text) => reply_ceiling(text, lengths, window),
            None => Tokens(u32::MAX),
        }
    }
}

#[cfg(test)]
mod tests;
