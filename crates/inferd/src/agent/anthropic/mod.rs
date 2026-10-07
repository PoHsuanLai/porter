//! Anthropic Messages, as an agent speaks it: the request read into inferd's chat request, and a
//! reply (a JSON message or a server-sent event stream) written from inferd's. Mapped: text,
//! base64 images, tool use and tool results, thinking and redacted thinking, system prompts, tool
//! choice, stop sequences, sampling. Dropped without a word: `cache_control` (a hint only),
//! `metadata`, `service_tier`. Refused, typed, naming the feature: server tools (a tool with a
//! `type` other than `custom`), documents and other block types, image URLs, `mcp_servers`,
//! structured output formats.

pub mod reply;
pub mod request;

pub use reply::{Stream, message_json};
pub use request::{estimate_tokens, parse};
