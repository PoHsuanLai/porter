//! OpenAI Chat Completions, as an agent speaks it: the request read into inferd's chat request,
//! and a reply (a JSON completion or a `data:` chunk stream ending in `[DONE]`) written from
//! inferd's. Mapped: text and `image_url` data URLs, tool calls and tool messages, `system` and
//! `developer` messages, tool choice, stop strings, sampling, `reasoning_effort`, a JSON-schema
//! `response_format`. Dropped without a word: `store`, `metadata`, `user`, `logprobs`,
//! `service_tier`. Refused, typed, naming the feature: `n` over one, image URLs that are not
//! data URLs, audio parts, `response_format` of `json_object`, the legacy `functions` fields.
//! Reasoning is carried as `reasoning_content` (DeepSeek's spelling), which clients ignore when
//! they do not know it.

pub mod reply;
pub mod request;

pub use reply::{Stream, completion_json, models_json};
pub use request::parse;
