//! Quark's assistant backend.
//!
//! # Shape
//!
//! Three providers behind one [`Provider`] trait — Anthropic's native Messages
//! API, anything speaking OpenAI's chat-completions format, and Ollama on the
//! local machine. Each streams [`Delta`] values in arrival order; the panel
//! above never learns which one answered.
//!
//! # Why blocking HTTP
//!
//! This is a worker thread with channels, the same shape as `PdfService`.
//! An async runtime would buy nothing here and would drag one into a GUI
//! process that currently has none.
//!
//! # What is deliberately not shared
//!
//! Prompt caching and the reasoning channel exist only on the Anthropic path.
//! The OpenAI format has no equivalent, and pretending otherwise by, say,
//! prefixing a fake "thinking" section would be a lie about where the text
//! came from.

pub mod anthropic;
pub mod context;
pub mod ollama;
pub mod openai;
pub mod provider;
pub mod service;
pub mod sse;

pub use anthropic::Anthropic;
pub use ollama::Ollama;
pub use openai::OpenAiCompat;
pub use context::{DocContext, Scope};
pub use service::{AiService, Event as AiEvent};
pub use provider::{AiError, ChatRequest, Delta, Message, Provider, Role, StopReason};

/// Which backend the user has selected.
///
/// Stored in preferences; the key that goes with it lives in the OS credential
/// store, never in the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The local option, and the only one that sends nothing off the machine.
    #[default]
    Ollama,
    Anthropic,
    OpenAiCompat,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Ollama, Backend::Anthropic, Backend::OpenAiCompat];

    pub fn label(self) -> &'static str {
        match self {
            Backend::Ollama => "Ollama (local)",
            Backend::Anthropic => "Anthropic",
            Backend::OpenAiCompat => "OpenAI-compatible",
        }
    }

    /// The credential-store key this backend's secret is filed under.
    ///
    /// `None` for Ollama, which has no credential at all — that absence is the
    /// point of offering it.
    pub fn credential_key(self) -> Option<&'static str> {
        match self {
            Backend::Ollama => None,
            Backend::Anthropic => Some("quark/anthropic"),
            Backend::OpenAiCompat => Some("quark/openai-compatible"),
        }
    }

    /// Whether choosing this backend sends document text off the machine.
    ///
    /// Drives the warning shown before the first cloud question is asked.
    pub fn leaves_the_machine(self) -> bool {
        !matches!(self, Backend::Ollama)
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Backend::Ollama => "llama3.2",
            Backend::Anthropic => anthropic::DEFAULT_MODEL,
            Backend::OpenAiCompat => "gpt-4o-mini",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_backend_is_the_one_that_sends_nothing_anywhere() {
        // Defaulting to a cloud provider would mean the first question a user
        // ever asks silently ships their document to a third party.
        assert_eq!(Backend::default(), Backend::Ollama);
        assert!(!Backend::default().leaves_the_machine());
    }

    #[test]
    fn only_the_cloud_backends_have_a_credential() {
        assert!(Backend::Ollama.credential_key().is_none());
        for b in [Backend::Anthropic, Backend::OpenAiCompat] {
            assert!(b.credential_key().is_some(), "{b:?} needs a key");
            assert!(b.leaves_the_machine());
        }
    }

    #[test]
    fn credential_keys_are_distinct_so_one_does_not_overwrite_the_other() {
        let keys: Vec<_> = Backend::ALL.iter().filter_map(|b| b.credential_key()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(keys.len(), unique.len(), "duplicate credential keys: {keys:?}");
    }

    #[test]
    fn every_backend_names_a_default_model() {
        for b in Backend::ALL {
            assert!(!b.default_model().is_empty(), "{b:?}");
            assert!(!b.label().is_empty(), "{b:?}");
        }
    }
}
