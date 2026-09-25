//! The shape every backend is reduced to.
//!
//! Three providers with three different wire formats sit behind one trait:
//! Anthropic's native Messages API, anything speaking OpenAI's chat-completions
//! shape, and Ollama on the local machine. The application above only ever sees
//! [`Delta`] values arriving in order.

use std::sync::atomic::AtomicBool;

/// Who said it. There is no `System` here — the system prompt is a separate
/// field, because two of the three providers treat it as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn wire(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            text: text.into(),
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            text: text.into(),
        }
    }
}

/// One turn's request.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    /// Instructions plus whatever document context was assembled. Kept separate
    /// from `messages` so it can carry a cache breakpoint on the providers that
    /// support one.
    pub system: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    /// Ran into `max_tokens`. Worth surfacing: the answer is cut off, not
    /// finished, and the user should be told rather than left guessing.
    MaxTokens,
    /// A safety classifier declined. Only Anthropic reports this, and it
    /// arrives as a successful HTTP 200, not an error.
    Refusal(String),
    Cancelled,
    Other(String),
}

/// A piece of the streamed reply, in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    /// Visible answer text.
    Text(String),
    /// Summarised reasoning, where the provider returns any. Shown separately
    /// so it can be collapsed — it is not the answer.
    Thinking(String),
    /// The turn finished. Always exactly one, last.
    Done(StopReason),
}

/// Anything that can answer a [`ChatRequest`].
pub trait Provider: Send + Sync {
    /// Human-readable name for the settings UI.
    fn name(&self) -> &str;

    /// Streams a reply, calling `on` for each piece as it arrives.
    ///
    /// `cancel` is polled between chunks: a chat needs a stop button, and the
    /// only responsive place to honour it is inside the read loop.
    fn chat(
        &self,
        req: &ChatRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<(), AiError>;

    /// Models this provider can serve, where it can be asked.
    ///
    /// Ollama knows what is installed locally; the cloud providers return their
    /// documented list, because enumerating models costs a request and the
    /// answer changes rarely.
    fn models(&self) -> Result<Vec<String>, AiError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("no API key configured for {0}")]
    NoKey(String),
    #[error("could not reach {provider}: {source}")]
    Transport {
        provider: String,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("{provider} returned {status}: {body}")]
    Status {
        provider: String,
        status: u16,
        body: String,
    },
    #[error("could not parse the response from {provider}: {detail}")]
    Protocol { provider: String, detail: String },
    #[error("{0}")]
    Other(String),
}

impl AiError {
    /// Whether retrying the same request could plausibly succeed.
    ///
    /// Used to decide between offering a retry and telling the user to fix
    /// something — a 401 will never fix itself, a 429 or a 503 usually does.
    pub fn is_retryable(&self) -> bool {
        match self {
            AiError::Transport { .. } => true,
            AiError::Status { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_use_the_names_every_provider_expects() {
        // All three wire formats happen to agree on these two strings, which is
        // why `Role` can be shared rather than translated per provider.
        assert_eq!(Role::User.wire(), "user");
        assert_eq!(Role::Assistant.wire(), "assistant");
    }

    #[test]
    fn auth_and_client_errors_are_not_offered_as_retryable() {
        // Offering "try again" for a bad key trains the user to click it
        // forever.
        assert!(!AiError::NoKey("anthropic".into()).is_retryable());
        assert!(
            !AiError::Status {
                provider: "anthropic".into(),
                status: 401,
                body: String::new()
            }
            .is_retryable()
        );
        assert!(
            !AiError::Status {
                provider: "anthropic".into(),
                status: 400,
                body: String::new()
            }
            .is_retryable()
        );
    }

    #[test]
    fn rate_limits_and_server_faults_are_retryable() {
        for status in [429, 500, 502, 503, 529] {
            assert!(
                AiError::Status {
                    provider: "anthropic".into(),
                    status,
                    body: String::new()
                }
                .is_retryable(),
                "{status} should be retryable"
            );
        }
    }
}
