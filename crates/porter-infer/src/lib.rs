//! The AI broker's pure half (design/31 §5.5): one typed request model whatever the provider,
//! streaming sessions, routing that prefers this computer, per-class floors, a local-only
//! switch, spend caps and an audit record without content. inferd runs it; wire adapters
//! implement [`Model`].

mod audit;
mod broker;
mod choice;
mod control;
mod cua;
mod error;
mod event;
mod ids;
mod model;
mod open;
mod pick;
mod pipeline;
mod policy;
mod readiness;
mod reply;
mod request;
mod route;
mod session;
mod slot;
mod speech;
mod spend;

pub use audit::AuditEntry;
pub use broker::Broker;
pub use choice::{
    AutoRow, Fit, LicenceClass, ModelRef, PickerInput, PickerRow, TierMap, TierRow, picker_rows,
    tier_choice, tier_label,
};
pub use control::{
    ChatControl, Effort, Knob, Reasoning, Sampling, Seed, StopReason, ThoughtSeal, ToolChoice,
    ToolParallelism,
};
pub use cua::{
    CuaBegin, CuaStepFailure, CuaStepReply, CuaStepRequest, DropReason, DroppedAction, FrameImage,
    FrameLayout, MaskedRegions, MediaKind, NoteFrom, PrevResult, SafetyHint, StepIndex, StepNote,
    TreeText, WindowGeometry,
};
pub use error::{InferRefusal, ModelError};
pub use event::{ClientFrame, Flow, InferEvent, StageNote};
pub use ids::{
    AttachIndex, Base64Bytes, JsonSchemaText, JsonText, OpaqueText, SignatureText, TextError,
    ToolCallId, ToolName, Traceparent,
};
pub use model::{ChatSink, Model, ModelCard};
pub use open::{LinkHello, OpenFrame, OpenOptions};
pub use pick::{
    AutoEvict, AutoMode, AutoPolicy, Declined, DeclinedBecause, Door, EngineLoad, Pick,
    PickCandidate, PickPolicy, PickRefusal, Picked, ShowReason, SwapCost, Why, pick,
};
pub use pipeline::{
    Answer, CatalogueModel, ClassSet, DescribeImages, Modality, Pipeline, PlanRules, ProviderId,
    Refusal, RequestShape, SlotPicks, Stage, StageRole, default_choice, picks_for, plan_pipeline,
};
pub use policy::{ClassFloor, Floor, LocalOnly, Policy};
pub use readiness::Readiness;
pub use reply::{
    ChatReply, EmbedReply, EmbedVector, InferReply, ServedBy, SpeakReply, TokenUsage,
    TranscribeReply,
};
pub use request::{
    ChatMessage, ChatRequest, EmbedRequest, EmbedRole, ImagePart, ImageSource, InferRequest,
    MessagePart, ReplyShape, RequestKind, Role, Task, TaskRequest, ThoughtPart, ToolCallPart,
    ToolDecl, ToolResultPart, ToolStatus,
};
pub use route::{Chosen, RouteAsk, RouteCandidate, TierChoice, admit, route};
pub use session::{InferSession, SessionError};
#[allow(deprecated)]
pub use slot::AiKind;
pub use slot::Slot;
pub use speech::{
    AudioFrame, AudioFrameOut, AudioRate, HeardDelta, LangPick, SpeakRequest, TranscribeBegin,
    TranscribeMode, VoiceName,
};
pub use spend::{Period, SpendCap, SpendScope, SpendVerdict, cost, spend_verdict};
