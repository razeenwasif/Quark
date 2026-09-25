//! The Anthropic Messages API.
//!
//! Native rather than through an OpenAI-compatible shim, because three things
//! this panel wants only exist here: a cache breakpoint on the document text,
//! adaptive thinking, and server-side refusal fallbacks.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::provider::{AiError, ChatRequest, Delta, Provider, StopReason};
use crate::sse;

/// The API version header. Pinned: this is a dated contract, not a "latest".
const API_VERSION: &str = "2023-06-01";

/// Opting into server-side fallbacks. On a policy decline the API re-runs the
/// same request on a fallback model inside the same call, so a refusal on one
/// question does not dead-end the conversation.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Models offered in the settings dropdown.
///
/// Hard-coded rather than fetched: enumerating costs a request on every open,
/// and the list changes far more slowly than the panel is opened. `/v1/models`
/// is the escape hatch if that stops being true.
pub const MODELS: &[&str] = &[
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-haiku-4-5",
];

/// The default. Opus 5 unless the user picks otherwise.
pub const DEFAULT_MODEL: &str = "claude-opus-5";

pub struct Anthropic {
    key: String,
    base: String,
}

impl Anthropic {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            base: "https://api.anthropic.com".into(),
        }
    }

    /// Points the client at a different host, for a proxy or a test server.
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Builds the request body.
    ///
    /// Split out from the send so the shape can be asserted in tests without a
    /// network — the parts that silently cost money or quietly stop working
    /// (the cache breakpoint, the thinking block) are invisible otherwise.
    pub fn body(&self, req: &ChatRequest) -> Value {
        let messages: Vec<Value> = req
            .messages
            .iter()
            .map(|m| json!({ "role": m.role.wire(), "content": m.text }))
            .collect();

        json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "stream": true,
            // The system prompt carries the document text, so it is the stable
            // prefix worth caching: the same document is asked about many
            // times in a row, and re-sending it uncached is the single largest
            // avoidable cost in this panel.
            "system": [{
                "type": "text",
                "text": req.system,
                "cache_control": { "type": "ephemeral" }
            }],
            // Adaptive lets the model decide how much to think; `summarized`
            // means the panel can show reasoning instead of a long silence.
            "thinking": { "type": "adaptive", "display": "summarized" },
            "fallbacks": "default",
            "messages": messages,
        })
    }
}

impl Provider for Anthropic {
    fn name(&self) -> &str {
        "Anthropic"
    }

    fn models(&self) -> Result<Vec<String>, AiError> {
        Ok(MODELS.iter().map(|s| s.to_string()).collect())
    }

    fn chat(
        &self,
        req: &ChatRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<(), AiError> {
        if self.key.trim().is_empty() {
            return Err(AiError::NoKey("Anthropic".into()));
        }
        let url = format!("{}/v1/messages", self.base);
        let response = ureq::post(&url)
            .header("content-type", "application/json")
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .send(serde_json::to_string(&self.body(req)).unwrap_or_default())
            .map_err(|e| match e {
                ureq::Error::StatusCode(status) => AiError::Status {
                    provider: "Anthropic".into(),
                    status,
                    body: String::new(),
                },
                other => AiError::Transport {
                    provider: "Anthropic".into(),
                    source: Box::new(other),
                },
            })?;

        let mut stop = StopReason::EndTurn;
        let finished = sse::read_events(response.into_body().into_reader(), cancel, |payload| {
            match parse_event(payload) {
                Some(Event::Text(t)) => on(Delta::Text(t)),
                Some(Event::Thinking(t)) => on(Delta::Thinking(t)),
                Some(Event::Stop(reason)) => stop = reason,
                Some(Event::End) => return false,
                None => {}
            }
            true
        })
        .map_err(|e| AiError::Protocol {
            provider: "Anthropic".into(),
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

/// What a single SSE payload meant.
enum Event {
    Text(String),
    Thinking(String),
    Stop(StopReason),
    End,
}

/// Interprets one Anthropic SSE payload.
///
/// Public to the crate so the wire handling can be tested against captured
/// bytes rather than against a live account.
fn parse_event(payload: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(payload).ok()?;
    match v.get("type")?.as_str()? {
        "content_block_delta" => {
            let delta = v.get("delta")?;
            match delta.get("type")?.as_str()? {
                "text_delta" => Some(Event::Text(delta.get("text")?.as_str()?.to_string())),
                "thinking_delta" => {
                    Some(Event::Thinking(delta.get("thinking")?.as_str()?.to_string()))
                }
                _ => None,
            }
        }
        "message_delta" => {
            let reason = v.get("delta")?.get("stop_reason")?.as_str()?;
            Some(Event::Stop(match reason {
                "end_turn" | "stop_sequence" => StopReason::EndTurn,
                "max_tokens" => StopReason::MaxTokens,
                // A refusal arrives as a successful response, not an error, so
                // it has to be read off the stop reason or it looks like the
                // model simply said nothing.
                "refusal" => StopReason::Refusal(
                    v.get("delta")
                        .and_then(|d| d.get("stop_details"))
                        .and_then(|d| d.get("explanation"))
                        .and_then(|e| e.as_str())
                        .unwrap_or("the request was declined")
                        .to_string(),
                ),
                other => StopReason::Other(other.to_string()),
            }))
        }
        "message_stop" => Some(Event::End),
        // `error` events can arrive mid-stream after a 200.
        "error" => Some(Event::Stop(StopReason::Other(
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("stream error")
                .to_string(),
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    fn request() -> ChatRequest {
        ChatRequest {
            model: DEFAULT_MODEL.into(),
            system: "You are reading a document.".into(),
            messages: vec![Message::user("What is on this page?")],
            max_tokens: 4096,
        }
    }

    #[test]
    fn the_document_text_carries_a_cache_breakpoint() {
        // The same document is asked about repeatedly. Without this the whole
        // document is re-billed at full price on every single question, which
        // is the largest avoidable cost in the panel.
        let b = Anthropic::new("k").body(&request());
        let system = &b["system"][0];
        assert_eq!(system["cache_control"]["type"], "ephemeral");
        assert_eq!(system["text"], "You are reading a document.");
    }

    #[test]
    fn thinking_is_adaptive_and_summarised() {
        // `budget_tokens` is rejected outright on this model family, and the
        // default display is omitted — which reads as a long pause with no
        // output at all.
        let b = Anthropic::new("k").body(&request());
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["thinking"]["display"], "summarized");
        assert!(
            b["thinking"].get("budget_tokens").is_none(),
            "budget_tokens is rejected with a 400 on this model family"
        );
    }

    #[test]
    fn the_request_streams() {
        // A long answer on a non-streaming request hits the HTTP timeout.
        assert_eq!(Anthropic::new("k").body(&request())["stream"], true);
    }

    #[test]
    fn a_missing_key_fails_before_any_request_is_made() {
        let cancel = AtomicBool::new(false);
        let err = Anthropic::new("  ")
            .chat(&request(), &cancel, &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, AiError::NoKey(_)), "{err}");
        assert!(!err.is_retryable());
    }

    #[test]
    fn text_deltas_are_read_from_the_stream() {
        let ev = parse_event(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        );
        assert!(matches!(ev, Some(Event::Text(t)) if t == "Hello"));
    }

    #[test]
    fn thinking_deltas_are_kept_separate_from_the_answer() {
        // Splicing reasoning into the answer text makes the model look like it
        // is rambling before answering.
        let ev = parse_event(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"considering"}}"#,
        );
        assert!(matches!(ev, Some(Event::Thinking(t)) if t == "considering"));
    }

    #[test]
    fn a_refusal_is_read_off_the_stop_reason() {
        // It arrives as HTTP 200. Without this the panel shows an empty reply
        // and no explanation.
        let ev = parse_event(
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","explanation":"declined"}}}"#,
        );
        match ev {
            Some(Event::Stop(StopReason::Refusal(why))) => assert_eq!(why, "declined"),
            other => panic!("expected a refusal, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn hitting_the_token_ceiling_is_distinguishable_from_finishing() {
        let ev = parse_event(r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#);
        assert!(matches!(ev, Some(Event::Stop(StopReason::MaxTokens))));
        let ev = parse_event(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#);
        assert!(matches!(ev, Some(Event::Stop(StopReason::EndTurn))));
    }

    #[test]
    fn unknown_events_are_ignored_rather_than_fatal() {
        // The API adds event types over time; an unrecognised one must not
        // abort a working stream.
        assert!(parse_event(r#"{"type":"content_block_start","index":0}"#).is_none());
        assert!(parse_event(r#"{"type":"ping"}"#).is_none());
        assert!(parse_event("not json at all").is_none());
    }

    #[test]
    fn the_default_model_is_opus_5() {
        assert_eq!(DEFAULT_MODEL, "claude-opus-5");
        assert!(MODELS.contains(&DEFAULT_MODEL));
        // Model ids are complete as published; a date suffix is a 404.
        for m in MODELS {
            assert!(
                !m.ends_with(|c: char| c.is_ascii_digit()) || !m.contains("-2025"),
                "{m} looks date-suffixed"
            );
        }
    }
}
