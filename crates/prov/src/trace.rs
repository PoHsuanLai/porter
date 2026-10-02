//! The names of our own span attributes, in the `quire.*` namespace.
//!
//! Pure constants and slug functions: no `tracing` dependency here. Spans are created only in the
//! daemons (inferd, companiond, readerd, cuad, memoryd, intentd), each in one `telemetry.rs`
//! whose `macro_rules!` span constructors spell these names as literals; a unit test there asserts
//! the macro's declared field set equals [`attr::ALL`] (and `genai-names` for the `gen_ai.*`
//! half). The OpenTelemetry GenAI names live in stoker's `genai-names`, the lowest repo.
//!
//! Privacy, hard: no prompt, completion, tool argument or result, screen text, title, recipient
//! or `SpaceId` ever goes on a span or metric. Counts, sizes, hashed ids, slugs and enums only.
//! `quire.space.hash` is a blake3 prefix of the id, keyed per install so it cannot be joined
//! across machines, never the id itself.

/// Span attribute keys.
pub mod attr {
    /// Who acted: a value of `ActorKind::slug()`.
    pub const ACTOR_KIND: &str = "quire.actor.kind";
    /// The effect class of an action: a value of `Effect::slug()`.
    pub const EFFECT: &str = "quire.effect";
    /// A keyed hash prefix of the `SpaceId`, never the id.
    pub const SPACE_HASH: &str = "quire.space.hash";
    /// The companion session.
    pub const SESSION: &str = "quire.session.id";
    /// The run.
    pub const RUN: &str = "quire.run.id";
    /// The task.
    pub const TASK: &str = "quire.task.id";
    /// Where a turn ran: `local`, `lan` or `cloud` (from `ServedBy`).
    pub const SERVED_LOCALITY: &str = "quire.served.locality";
    /// The code of a refusal.
    pub const DENY_CODE: &str = "quire.deny.code";
    /// The policy ruling.
    pub const RULING: &str = "quire.policy.ruling";
    /// How a confirmation ended.
    pub const CONFIRM_OUTCOME: &str = "quire.confirm.outcome";
    /// The pipeline stage.
    pub const STAGE: &str = "quire.pipeline.stage";
    /// The computer-use step number.
    pub const CUA_STEP: &str = "quire.cua.step";
    /// The computer-use mode.
    pub const CUA_MODE: &str = "quire.cua.mode";

    /// Every key above, in declaration order: what a span macro's declared fields are tested
    /// against.
    pub const ALL: &[&str] = &[
        ACTOR_KIND,
        EFFECT,
        SPACE_HASH,
        SESSION,
        RUN,
        TASK,
        SERVED_LOCALITY,
        DENY_CODE,
        RULING,
        CONFIRM_OUTCOME,
        STAGE,
        CUA_STEP,
        CUA_MODE,
    ];
}
