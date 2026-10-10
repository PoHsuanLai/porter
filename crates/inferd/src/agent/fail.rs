//! How a request the endpoint cannot serve is answered: a status and a coarse body in the error
//! shape of the protocol the agent spoke. The body says what kind of thing went wrong and never
//! quotes a provider's reply, a key, the token or the prompt.

use porter_infer::{ModelError, StopReason};
use serde_json::{Value, json};

/// Which protocol the agent spoke, so the answer is in its own error shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
    /// OpenAI Chat Completions (`/v1/chat/completions`, `/v1/models`).
    OpenAi,
}

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// No token, or not this session's.
    Unauthenticated,
    /// The route is outside what the person granted or the policy allows.
    Forbidden,
    /// A model the route does not serve, or a path the endpoint does not have.
    NotFound,
    /// A request the endpoint cannot express: malformed, or a feature it does not map.
    Invalid,
    /// The body is larger than the endpoint reads.
    TooLarge,
    /// A spend cap is reached: the agent is to stop, not to retry.
    SpendCap,
    /// The provider limits the account; retry after this many seconds, when it said.
    RateLimited(Option<u32>),
    /// The route cannot answer now (its engine is not ready, the provider is overloaded).
    Overloaded,
    /// The provider or engine failed or could not be reached.
    Upstream,
    /// The model reasoned and said nothing (`ModelError::OnlyThought`).
    OnlyThought {
        /// Why it ended.
        stop: StopReason,
        /// Bytes of reasoning, never the reasoning.
        thought_len: u32,
    },
}

/// A failed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What went wrong.
    pub cause: Cause,
    /// A short sentence for the agent's log; never content.
    pub message: String,
}

impl Failure {
    /// A failure of `cause` that says `message`.
    pub fn new(cause: Cause, message: impl Into<String>) -> Self {
        Self {
            cause,
            message: message.into(),
        }
    }

    /// What a failed model call is told to the agent as.
    pub fn of_model(error: ModelError) -> Self {
        match error {
            ModelError::Unreachable => Self::new(Cause::Upstream, "the model could not be reached"),
            ModelError::RateLimited(seconds) => Self::new(
                Cause::RateLimited(Some(seconds)),
                "the model is rate limited",
            ),
            ModelError::Unauthorized => Self::new(
                Cause::Upstream,
                "the model refused the account's credentials",
            ),
            ModelError::PaymentRequired => Self::new(Cause::Upstream, "the account needs payment"),
            ModelError::SignInRefused => {
                Self::new(Cause::Upstream, "the company refused the sign-in")
            }
            ModelError::Refused => Self::new(Cause::Invalid, "the model refused the request"),
            ModelError::Unreadable | ModelError::Unparseable => {
                Self::new(Cause::Upstream, "the model's reply could not be read")
            }
            ModelError::NotReady => Self::new(Cause::Overloaded, "the model is not ready"),
            ModelError::ContextOverflow => Self::new(
                Cause::Invalid,
                "the prompt is longer than the model's context window",
            ),
            ModelError::OnlyThought { stop, thought_len } => Self::new(
                Cause::OnlyThought { stop, thought_len },
                format!("the model produced only reasoning ({thought_len} bytes) and no answer"),
            ),
            // a variant a newer porter adds: told to the agent as an upstream failure
            _ => Self::new(Cause::Upstream, "the model failed"),
        }
    }

    /// The HTTP status.
    pub fn status(&self, shape: Shape) -> u16 {
        match self.cause {
            Cause::Unauthenticated => 401,
            Cause::Forbidden => 403,
            Cause::NotFound => 404,
            Cause::Invalid => 400,
            Cause::TooLarge => 413,
            Cause::SpendCap | Cause::RateLimited(_) => 429,
            Cause::Overloaded => match shape {
                Shape::Anthropic => 529,
                Shape::OpenAi => 503,
            },
            Cause::Upstream | Cause::OnlyThought { .. } => 502,
        }
    }

    /// The error's type and code in the protocol's words.
    fn words(&self, shape: Shape) -> (&'static str, Option<&'static str>) {
        match (shape, self.cause) {
            (Shape::Anthropic, Cause::Unauthenticated) => ("authentication_error", None),
            (Shape::Anthropic, Cause::Forbidden) => ("permission_error", None),
            (Shape::Anthropic, Cause::NotFound) => ("not_found_error", None),
            (Shape::Anthropic, Cause::Invalid) => ("invalid_request_error", None),
            (Shape::Anthropic, Cause::TooLarge) => ("request_too_large", None),
            (Shape::Anthropic, Cause::SpendCap | Cause::RateLimited(_)) => {
                ("rate_limit_error", None)
            }
            (Shape::Anthropic, Cause::Overloaded) => ("overloaded_error", None),
            (Shape::Anthropic, Cause::Upstream | Cause::OnlyThought { .. }) => ("api_error", None),
            (Shape::OpenAi, Cause::Unauthenticated) => {
                ("authentication_error", Some("invalid_api_key"))
            }
            (Shape::OpenAi, Cause::Forbidden) => ("permission_error", Some("permission_denied")),
            (Shape::OpenAi, Cause::NotFound) => ("invalid_request_error", Some("model_not_found")),
            (Shape::OpenAi, Cause::Invalid) => ("invalid_request_error", None),
            (Shape::OpenAi, Cause::TooLarge) => {
                ("invalid_request_error", Some("request_too_large"))
            }
            (Shape::OpenAi, Cause::SpendCap) => ("rate_limit_error", Some("spend_cap_reached")),
            (Shape::OpenAi, Cause::RateLimited(_)) => {
                ("rate_limit_error", Some("rate_limit_exceeded"))
            }
            (Shape::OpenAi, Cause::Overloaded) => ("server_error", Some("service_unavailable")),
            (Shape::OpenAi, Cause::Upstream) => ("server_error", Some("upstream_error")),
            (Shape::OpenAi, Cause::OnlyThought { .. }) => ("server_error", Some("only_reasoning")),
        }
    }

    /// The body, in the protocol's error shape.
    pub fn body(&self, shape: Shape) -> Value {
        let (kind, code) = self.words(shape);
        match shape {
            Shape::Anthropic => json!({
                "type": "error",
                "error": { "type": kind, "message": self.message },
                "request_id": "req_porter",
            }),
            Shape::OpenAi => json!({
                "error": {
                    "message": self.message,
                    "type": kind,
                    "param": null,
                    "code": code,
                },
            }),
        }
    }

    /// Headers beyond the content type. A spend cap says not to retry (Anthropic's SDKs read
    /// `x-should-retry`), so the agent stops instead of backing off and asking again.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        match self.cause {
            Cause::SpendCap => vec![("x-should-retry", "false".to_owned())],
            Cause::RateLimited(Some(seconds)) => vec![("retry-after", seconds.to_string())],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cause_has_a_status_and_a_type_in_both_shapes() {
        let only = Cause::OnlyThought {
            stop: StopReason::EndTurn,
            thought_len: 9,
        };
        let cases = [
            (Cause::Unauthenticated, 401, 401, "authentication_error"),
            (Cause::Forbidden, 403, 403, "permission_error"),
            (Cause::NotFound, 404, 404, "not_found_error"),
            (Cause::Invalid, 400, 400, "invalid_request_error"),
            (Cause::TooLarge, 413, 413, "request_too_large"),
            (Cause::SpendCap, 429, 429, "rate_limit_error"),
            (Cause::RateLimited(Some(3)), 429, 429, "rate_limit_error"),
            (Cause::Overloaded, 529, 503, "overloaded_error"),
            (Cause::Upstream, 502, 502, "api_error"),
            (only, 502, 502, "api_error"),
        ];
        for (cause, anthropic, openai, kind) in cases {
            let failure = Failure::new(cause, "x");
            assert_eq!(failure.status(Shape::Anthropic), anthropic, "{cause:?}");
            assert_eq!(failure.status(Shape::OpenAi), openai, "{cause:?}");
            let body = failure.body(Shape::Anthropic);
            assert_eq!(body["type"], "error");
            assert_eq!(body["error"]["type"], kind, "{cause:?}");
            let body = failure.body(Shape::OpenAi);
            assert_eq!(body["error"]["message"], "x");
            assert!(body["error"]["type"].is_string());
        }
    }

    #[test]
    fn a_spend_cap_tells_the_agent_not_to_retry_and_a_thought_only_reply_is_an_error() {
        let cap = Failure::new(Cause::SpendCap, "cap");
        assert_eq!(cap.headers(), vec![("x-should-retry", "false".to_owned())]);
        let only = Failure::of_model(ModelError::OnlyThought {
            stop: StopReason::MaxTokens,
            thought_len: 40,
        });
        assert_eq!(only.status(Shape::OpenAi), 502);
        assert!(only.message.contains("40 bytes"));
        assert!(!only.message.contains("thinking about"));
    }
}
