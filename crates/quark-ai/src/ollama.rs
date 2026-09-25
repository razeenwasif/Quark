//! Ollama, on the local machine.
//!
//! The privacy-preserving option: nothing leaves the machine, so the whole
//! document can go into the prompt without a second thought. No key, and the
//! installed models can actually be enumerated, which the cloud providers
//! cannot do cheaply.
//!
//! Streams newline-delimited JSON rather than SSE.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::provider::{AiError, ChatRequest, Delta, Provider, StopReason};
use crate::sse;

/// Where Ollama listens by default.
pub const DEFAULT_HOST: &str = "http://localhost:11434";

pub struct Ollama {
    host: String,
}

impl Ollama {
    pub fn new(host: impl Into<String>) -> Self {
        Self { host: host.into() }
    }

    pub fn local() -> Self {
        Self::new(DEFAULT_HOST)
    }

    pub fn body(&self, req: &ChatRequest) -> Value {
        let mut messages = vec![json!({ "role": "system", "content": req.system })];
        messages.extend(
            req.messages
                .iter()
                .map(|m| json!({ "role": m.role.wire(), "content": m.text })),
        );
        json!({
            "model": req.model,
            "stream": true,
            "messages": messages,
            "options": { "num_predict": req.max_tokens },
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.host.trim_end_matches('/'), path)
    }
}

impl Provider for Ollama {
    fn name(&self) -> &str {
        "Ollama"
    }

    fn models(&self) -> Result<Vec<String>, AiError> {
        let mut response = ureq::get(&self.url("/api/tags"))
            .call()
            .map_err(|e| AiError::Transport {
                provider: "Ollama".into(),
                source: Box::new(e),
            })?;
        let v: Value = response
            .body_mut()
            .read_json()
            .map_err(|e| AiError::Protocol {
                provider: "Ollama".into(),
                detail: e.to_string(),
            })?;
        Ok(chat_models(&v))
    }

    fn chat(
        &self,
        req: &ChatRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<(), AiError> {
        let response = ureq::post(&self.url("/api/chat"))
            .header("content-type", "application/json")
            .send(serde_json::to_string(&self.body(req)).unwrap_or_default())
            .map_err(|e| match e {
                ureq::Error::StatusCode(status) => AiError::Status {
                    provider: "Ollama".into(),
                    status,
                    body: String::new(),
                },
                other => AiError::Transport {
                    provider: "Ollama".into(),
                    source: Box::new(other),
                },
            })?;

        let finished = sse::read_ndjson(response.into_body().into_reader(), cancel, |line| {
            match parse_line(line) {
                Some(Chunk::Text(t)) => {
                    if !t.is_empty() {
                        on(Delta::Text(t));
                    }
                    true
                }
                // `done: true` is the last line; nothing follows it.
                Some(Chunk::Done) => false,
                None => true,
            }
        })
        .map_err(|e| AiError::Protocol {
            provider: "Ollama".into(),
            detail: e.to_string(),
        })?;

        on(Delta::Done(if finished {
            StopReason::EndTurn
        } else {
            StopReason::Cancelled
        }));
        Ok(())
    }
}

/// Names from `/api/tags`, minus the ones that cannot hold a conversation.
///
/// An Ollama install usually carries embedding models alongside chat ones, and
/// picking `nomic-embed-text` from a list fails with an unhelpful error at the
/// point of asking a question rather than at the point of choosing. Newer
/// Ollama reports `capabilities`; when it is absent the model is kept, because
/// excluding on missing evidence would empty the list on older versions.
fn chat_models(v: &Value) -> Vec<String> {
    let Some(list) = v.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter(|m| match m.get("capabilities").and_then(|c| c.as_array()) {
            Some(caps) => caps.iter().any(|c| c.as_str() == Some("completion")),
            None => true,
        })
        .filter_map(|m| m.get("name")?.as_str().map(str::to_string))
        .collect()
}

enum Chunk {
    Text(String),
    Done,
}

fn parse_line(line: &str) -> Option<Chunk> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
        return Some(Chunk::Done);
    }
    let text = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())?;
    Some(Chunk::Text(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    fn request() -> ChatRequest {
        ChatRequest {
            model: "llama3.2".into(),
            system: "context".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 512,
        }
    }

    #[test]
    fn the_system_prompt_leads_the_message_list() {
        let b = Ollama::local().body(&request());
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["role"], "user");
    }

    #[test]
    fn the_token_ceiling_goes_in_options_not_top_level() {
        // Ollama ignores a top-level max_tokens silently, so a wrong placement
        // looks like the setting simply does nothing.
        let b = Ollama::local().body(&request());
        assert_eq!(b["options"]["num_predict"], 512);
        assert!(b.get("max_tokens").is_none());
    }

    #[test]
    fn a_trailing_slash_on_the_host_does_not_double_up() {
        assert_eq!(
            Ollama::new("http://localhost:11434/").url("/api/chat"),
            "http://localhost:11434/api/chat"
        );
    }

    #[test]
    fn content_is_read_from_each_line() {
        let c = parse_line(r#"{"message":{"role":"assistant","content":"Hi"},"done":false}"#);
        assert!(matches!(c, Some(Chunk::Text(t)) if t == "Hi"));
    }

    #[test]
    fn the_done_line_ends_the_stream() {
        // The final line carries stats and no content; treating it as text
        // appends an empty chunk and keeps the connection open.
        assert!(matches!(
            parse_line(r#"{"done":true,"total_duration":123}"#),
            Some(Chunk::Done)
        ));
    }

    #[test]
    fn embedding_models_are_left_out_of_the_picker() {
        // A real install carries these alongside chat models. Choosing one
        // fails at the point of asking a question, not at the point of
        // choosing, which is a confusing place to find out.
        let tags = serde_json::json!({"models": [
            {"name": "gemma4:26b", "capabilities": ["completion", "tools", "vision"]},
            {"name": "nomic-embed-text-v2-moe:latest", "capabilities": ["embedding"]},
            {"name": "embeddinggemma:latest", "capabilities": ["embedding"]},
            {"name": "qwen3.8:27b", "capabilities": ["completion", "thinking"]}
        ]});
        assert_eq!(chat_models(&tags), vec!["gemma4:26b", "qwen3.8:27b"]);
    }

    #[test]
    fn a_model_that_reports_no_capabilities_is_kept() {
        // Older Ollama omits the field entirely; excluding on missing evidence
        // would show an empty picker to anyone who has not updated.
        let tags = serde_json::json!({"models": [{"name": "llama3.2"}]});
        assert_eq!(chat_models(&tags), vec!["llama3.2"]);
    }

    #[test]
    fn a_malformed_tag_listing_yields_no_models_rather_than_panicking() {
        assert!(chat_models(&serde_json::json!({})).is_empty());
        assert!(chat_models(&serde_json::json!({"models": "nonsense"})).is_empty());
    }

    #[test]
    fn ollama_needs_no_key() {
        // The whole point of the local option: no credential, no egress.
        let p = Ollama::local();
        assert_eq!(p.name(), "Ollama");
    }
}
