//! inferd's pipelines (capabilities.md sections 3 and 4): the catalogue the planner reads, the
//! plan for a request, and the stage runner.
//!
//! `porter_infer::plan_pipeline` is pure; its edge is `porter_router::pipeline`: [`catalogue_of`]
//! turns what routing lists into catalogue models (what each takes and gives, the slots it is in,
//! the provider that reaches it), [`granted_of`] is the set of providers the person's accounts
//! reach through accountd (the planner discovers none), and [`decide_text`] is the language
//! slot's route: a text-only request is one stage. They are re-exported here at their old path.
//! [`run`] runs a plan.
//!
//! Wired: the text stage on every `Llm` session (so every route goes through the planner), and the
//! text plus `voice_in` plan run by [`run::run_pipeline`] over an engine that hears. Typed as not
//! yet, with a reason ([`run::unsupported`]): a `describe` stage (image_in), a `speak` stage
//! (voice_out). A voice chat on a live session ([`Hearing`]) is planned here with the audio in the
//! request's shape and run by [`run::run_pipeline`]: `speech::Ears` hears it on the speech host,
//! then the answering model gets the transcript.

mod live;
mod run;

pub use live::Hearing;

pub use crate::speech::AudioIn;
pub use porter_router::pipeline::{catalogue_of, decide_text, granted_of, plan};
pub use run::{PipelineInput, Transcriber, VecAudio, run_pipeline, unsupported};

#[cfg(test)]
mod tests;
