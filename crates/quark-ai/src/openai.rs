//! Anything speaking OpenAI's chat-completions shape.
//!
//! One client covers OpenAI, Groq, OpenRouter, DeepSeek, Together and most
//! hosted endpoints, because they all copied the same request and SSE format.
//! The only thing that varies is the base URL and the key.
//!
//! Deliberately thinner than [`crate::anthropic`]: this format has no cache
//! breakpoints and no reasoning channel, so those features are simply absent
//! here rather than faked.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::provider::{AiError, ChatRequest, Delta, Provider, StopReason};
use crate::sse;

pub struct OpenAiCompat {
    key: String,
    base: String,
    label: String,
}

impl OpenAiCompat {
    /// `base` is the API root without a trailing slash — for OpenAI itself,
    /// `https://api.openai.com/v1`.
    pub fn new(label: impl Into<String>, base: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            base: base.into(),
            label: label.into(),
        }
    }

    pub fn body(&self, req: &ChatRequest) -> Value {
        // The system prompt is a message here, not a field, and it goes first.
        let mut messages = vec![json!({ "role": "system", "content": req.system })];
        messages.extend(
            req.messages
                .iter()
                .map(|m| json!({ "role": m.role.wire(), "content": m.text })),
        );
        json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "stream": true,
            "messages": messages,
        })
    }
}

impl Provider for OpenAiCompat {
    fn name(&self) -> &str {
        &self.label
    }

    fn models(&self) -> Result<Vec<String>, AiError> {
        // Endpoints differ far too much to hard-code a list, and many proxies
        // serve models under names only the operator knows. The settings UI
        // takes a typed model id for this provider.
        Ok(Vec::new())
    }

    fn chat(
        &self,
        req: &ChatRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<(), AiError> {
        if self.key.trim().is_empty() {
            return Err(AiError::NoKey(self.label.clone()));
        }
        let url = format!("{}/chat/completions", self.base.trim_end_matches('/'));
        let response = ureq::post(&url)
            .header("content-type", "application/json")
            .header("authorization", &format!("Bearer {}", self.key))
            .send(serde_json::to_string(&self.body(req)).unwrap_or_default())
            .map_err(|e| match e {
                ureq::Error::StatusCode(status) => AiError::Status {
                    provider: self.label.clone(),
                    status,
                    body: String::new(),
                },
                other => AiError::Transport {
                    provider: self.label.clone(),
                    source: Box::new(other),
                },
            })?;

        let mut stop = StopReason::EndTurn;
        let finished = sse::read_events(response.into_body().into_reader(), cancel, |payload| {
            if let Some((text, reason)) = parse_chunk(payload) {
                if !text.is_empty() {
                    on(Delta::Text(text));
                }
                if let Some(r) = reason {
                    stop = r;
                }
            }
            true
        })
        .map_err(|e| AiError::Protocol {
            provider: self.label.clone(),
            detail: e.to_string(),
        })?;

        on(Delta::Done(if finished {
            stop
        } else {
            StopReason::Cancelled
        }));
        Ok(())
    }
}

/// Pulls the text and any finish reason out of one chunk.
fn parse_chunk(payload: &str) -> Option<(String, Option<StopReason>)> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let choice = v.get("choices")?.get(0)?;
    let text = choice
        .get("delta")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let reason = choice
        .get("finish_reason")
        .and_then(|r| r.as_str())
        .map(|r| match r {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            other => StopReason::Other(other.to_string()),
        });
    Some((text, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    fn request() -> ChatRequest {
        ChatRequest {
            model: "gpt-4o-mini".into(),
            system: "context".into(),
            messages: vec![Message::user("hi"), Message::assistant("hello")],
            max_tokens: 1024,
        }
    }

    #[test]
    fn the_system_prompt_becomes_the_first_message() {
        // This format has no system field; putting it anywhere but first makes
        // some endpoints ignore it entirely.
        let b = OpenAiCompat::new("OpenAI", "https://api.openai.com/v1", "k").body(&request());
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "context");
        assert_eq!(b["messages"][1]["role"], "user");
        assert_eq!(b["messages"][2]["role"], "assistant");
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        // Users paste base URLs with and without one; `//chat/completions` is
        // a 404 on most endpoints.
        let p = OpenAiCompat::new("Proxy", "https://example.test/v1/", "k");
        assert_eq!(p.base.trim_end_matches('/'), "https://example.test/v1");
    }

    #[test]
    fn content_deltas_are_read() {
        let (text, reason) = parse_chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(text, "Hi");
        assert!(reason.is_none());
    }

    #[test]
    fn the_finish_reason_distinguishes_truncation_from_completion() {
        let (_, reason) =
            parse_chunk(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#).unwrap();
        assert_eq!(reason, Some(StopReason::MaxTokens));
        let (_, reason) =
            parse_chunk(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#).unwrap();
        assert_eq!(reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn a_role_only_opening_chunk_yields_no_text() {
        // The first chunk carries `{"role":"assistant"}` and no content.
        let (text, _) =
            parse_chunk(r#"{"choices":[{"index":0,"delta":{"role":"assistant"}}]}"#).unwrap();
        assert!(text.is_empty());
    }

    #[test]
    fn a_missing_key_fails_before_any_request_is_made() {
        let cancel = AtomicBool::new(false);
        let err = OpenAiCompat::new("OpenAI", "https://api.openai.com/v1", "")
            .chat(&request(), &cancel, &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, AiError::NoKey(_)), "{err}");
    }

}
