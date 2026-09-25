//! The assistant dock: conversation state and the panel that draws it.
//!
//! The backend lives in `quark-ai`; this is the part that knows about the open
//! document, the credential store, and the frame loop.

use quark_ai::{AiService, Backend, Delta, Scope, StopReason};
use quark_core::prefs::Prefs;

/// One side of the conversation, as shown.
pub(crate) struct Turn {
    pub(crate) mine: bool,
    pub(crate) text: String,
    /// Summarised reasoning, where the provider returned any. Kept apart from
    /// `text` so it can be collapsed — it is not the answer.
    pub(crate) thinking: String,
    /// Set when the turn ended in something the reader should know about: a
    /// refusal, a truncation, or an error.
    pub(crate) note: Option<String>,
}

impl Turn {
    fn mine(text: String) -> Self {
        Self {
            mine: true,
            text,
            thinking: String::new(),
            note: None,
        }
    }

    fn theirs() -> Self {
        Self {
            mine: false,
            text: String::new(),
            thinking: String::new(),
            note: None,
        }
    }
}

pub(crate) struct Assistant {
    pub(crate) service: AiService,
    pub(crate) turns: Vec<Turn>,
    pub(crate) draft: String,
    /// The turn currently streaming, if any. Also drives the stop button.
    pub(crate) in_flight: Option<u64>,
    /// Set once the user has been told that a cloud backend sends the document
    /// off the machine, so the warning is shown once rather than every turn.
    pub(crate) egress_acknowledged: bool,
    /// Models the current backend reported, for the picker.
    pub(crate) models: Vec<String>,
    /// Whether a listing has been requested for the backend now selected.
    ///
    /// Without this the picker would re-query on every frame, which for Ollama
    /// is an HTTP request sixty times a second.
    pub(crate) models_requested_for: Option<Backend>,
    /// Whether the settings drawer is open.
    pub(crate) settings_open: bool,
    /// What the user is typing into the key field.
    ///
    /// A stored key is never read back into this — the credential store is
    /// write-and-use, and echoing a secret into a text field puts it on screen
    /// and in the frame buffer for no benefit.
    pub(crate) key_input: String,
    /// Cached answer to "is a key stored for this backend", so the panel does
    /// not hit the credential store every frame.
    pub(crate) key_state: Option<(Backend, KeySource)>,
}

/// Where the key for the selected backend is coming from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeySource {
    /// An environment variable, which wins over anything stored.
    Environment,
    /// The OS credential store.
    Stored,
    /// Nothing configured. Cloud backends cannot answer in this state.
    Missing,
}

impl Assistant {
    pub(crate) fn new() -> Self {
        Self {
            service: AiService::start(),
            turns: Vec::new(),
            draft: String::new(),
            in_flight: None,
            egress_acknowledged: false,
            models: Vec::new(),
            models_requested_for: None,
            settings_open: false,
            key_input: String::new(),
            key_state: None,
        }
    }

    /// Whether a question can be sent right now.
    pub(crate) fn can_send(&self) -> bool {
        self.in_flight.is_none() && !self.draft.trim().is_empty()
    }

    /// Records the question and opens an empty reply for the stream to fill.
    pub(crate) fn begin(&mut self, id: u64, question: String) {
        self.turns.push(Turn::mine(question));
        self.turns.push(Turn::theirs());
        self.in_flight = Some(id);
    }

    /// Folds one streamed piece into the open reply.
    ///
    /// Deltas from a turn that is no longer in flight are dropped: cancelling
    /// and asking again would otherwise splice the abandoned answer into the
    /// new one.
    pub(crate) fn apply(&mut self, id: u64, delta: Delta) {
        if self.in_flight != Some(id) {
            return;
        }
        let Some(turn) = self.turns.last_mut() else {
            return;
        };
        match delta {
            Delta::Text(t) => turn.text.push_str(&t),
            Delta::Thinking(t) => turn.thinking.push_str(&t),
            Delta::Done(reason) => {
                turn.note = match reason {
                    StopReason::EndTurn => None,
                    StopReason::MaxTokens => {
                        Some("Cut off at the length limit — ask for less at a time.".into())
                    }
                    StopReason::Refusal(why) => Some(format!("The model declined: {why}")),
                    StopReason::Cancelled => Some("Stopped.".into()),
                    StopReason::Other(o) => Some(o),
                };
                if turn.text.is_empty() && turn.note.is_none() {
                    turn.note = Some("The model returned nothing.".into());
                }
                self.in_flight = None;
            }
        }
    }

    /// Records a failure against the open reply.
    pub(crate) fn fail(&mut self, id: u64, message: String) {
        if self.in_flight != Some(id) {
            return;
        }
        if let Some(turn) = self.turns.last_mut() {
            turn.note = Some(message);
        }
        self.in_flight = None;
    }

    /// The conversation so far, as the backend wants it.
    ///
    /// Turns that failed carry no usable text, so they are left out rather than
    /// sent as empty assistant messages — several providers reject those.
    pub(crate) fn history(&self) -> Vec<quark_ai::Message> {
        self.turns
            .iter()
            .filter(|t| !t.text.trim().is_empty())
            .map(|t| {
                if t.mine {
                    quark_ai::Message::user(t.text.clone())
                } else {
                    quark_ai::Message::assistant(t.text.clone())
                }
            })
            .collect()
    }
}

/// The environment variable each cloud backend reads, matching the name its
/// own SDKs use so an existing shell setup just works.
pub(crate) fn env_var(b: Backend) -> Option<&'static str> {
    match b {
        Backend::Ollama => None,
        Backend::Anthropic => Some("ANTHROPIC_API_KEY"),
        Backend::OpenAiCompat => Some("OPENAI_API_KEY"),
    }
}

/// Where the key for `backend` is coming from, if anywhere.
///
/// Environment first, matching how the provider SDKs resolve credentials and
/// making it easy to override a stored key for one session.
pub(crate) fn key_source(backend: Backend) -> KeySource {
    let Some(var) = env_var(backend) else {
        return KeySource::Missing;
    };
    if std::env::var(var).map(|v| !v.trim().is_empty()) == Ok(true) {
        return KeySource::Environment;
    }
    let stored = backend
        .credential_key()
        .and_then(|t| quark_shell::load_secret(t).ok().flatten())
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    if stored {
        KeySource::Stored
    } else {
        KeySource::Missing
    }
}

/// Base URLs for endpoints that speak the OpenAI shape.
///
/// Offered as presets because the path suffix is the part people get wrong —
/// Gemini's compatibility layer in particular is not an obvious URL. Editable,
/// since any of these can change and a proxy will have its own.
pub(crate) const BASE_URL_PRESETS: [(&str, &str); 4] = [
    ("OpenAI", "https://api.openai.com/v1"),
    (
        "Gemini",
        "https://generativelanguage.googleapis.com/v1beta/openai",
    ),
    ("Groq", "https://api.groq.com/openai/v1"),
    ("OpenRouter", "https://openrouter.ai/api/v1"),
];

/// Reads the configured backend out of preferences.
pub(crate) fn backend_of(prefs: &Prefs) -> Backend {
    match prefs.ai_backend.as_str() {
        "anthropic" => Backend::Anthropic,
        "openai" => Backend::OpenAiCompat,
        _ => Backend::Ollama,
    }
}

pub(crate) fn backend_key(b: Backend) -> &'static str {
    match b {
        Backend::Ollama => "ollama",
        Backend::Anthropic => "anthropic",
        Backend::OpenAiCompat => "openai",
    }
}

pub(crate) fn scope_of(prefs: &Prefs) -> Scope {
    match prefs.ai_scope.as_str() {
        "page" => Scope::Page,
        "whole" => Scope::Whole,
        _ => Scope::Nearby,
    }
}

pub(crate) fn scope_key(s: Scope) -> &'static str {
    match s {
        Scope::Page => "page",
        Scope::Nearby => "nearby",
        Scope::Whole => "whole",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cloud_backend_reads_the_variable_its_own_sdks_use() {
        // Someone who already has ANTHROPIC_API_KEY exported expects it to
        // work without being told about a second, Quark-specific name.
        assert_eq!(env_var(Backend::Anthropic), Some("ANTHROPIC_API_KEY"));
        assert_eq!(env_var(Backend::OpenAiCompat), Some("OPENAI_API_KEY"));
        assert_eq!(env_var(Backend::Ollama), None, "the local backend has no key");
    }

    #[test]
    fn the_local_backend_never_reports_a_key() {
        // Showing "no API key set" for Ollama would send people hunting for a
        // credential that does not exist.
        assert_eq!(key_source(Backend::Ollama), KeySource::Missing);
        assert!(!Backend::Ollama.leaves_the_machine());
    }

    #[test]
    fn every_endpoint_preset_is_an_absolute_https_url_with_no_trailing_slash() {
        // The client appends `/chat/completions`; a trailing slash gives a
        // double slash, and a relative URL fails at request time.
        for (name, url) in BASE_URL_PRESETS {
            assert!(url.starts_with("https://"), "{name}: {url}");
            assert!(!url.ends_with('/'), "{name} has a trailing slash: {url}");
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn the_default_endpoint_is_one_of_the_presets() {
        // Otherwise the preset row shows nothing selected on a fresh install,
        // which looks like the setting is unset.
        let d = Prefs::default();
        assert!(
            BASE_URL_PRESETS.iter().any(|(_, u)| *u == d.ai_base_url),
            "default {} is not among the presets",
            d.ai_base_url
        );
    }

    #[test]
    fn gemini_is_reached_through_the_openai_compatible_backend() {
        // It is not an Anthropic model, and it is not Ollama. Anyone looking
        // for it needs to land on the OpenAI-compatible backend.
        let (_, gemini) = BASE_URL_PRESETS
            .iter()
            .find(|(n, _)| *n == "Gemini")
            .expect("Gemini preset missing");
        assert!(gemini.contains("googleapis.com"), "{gemini}");
        assert!(
            gemini.contains("openai"),
            "the compatibility path is what makes it work here: {gemini}"
        );
    }

    fn assistant() -> Assistant {
        Assistant::new()
    }

    #[test]
    fn a_reply_is_assembled_from_its_deltas() {
        let mut a = assistant();
        a.begin(1, "what is this?".into());
        a.apply(1, Delta::Text("It is ".into()));
        a.apply(1, Delta::Text("a book.".into()));
        a.apply(1, Delta::Done(StopReason::EndTurn));
        assert_eq!(a.turns[1].text, "It is a book.");
        assert!(a.turns[1].note.is_none());
        assert!(a.in_flight.is_none(), "the turn never finished");
    }

    #[test]
    fn deltas_from_an_abandoned_turn_are_dropped() {
        // Cancel, ask again, then the old stream's last chunks arrive. Without
        // the id check they append to the new answer.
        let mut a = assistant();
        a.begin(1, "first".into());
        a.apply(1, Delta::Done(StopReason::Cancelled));
        a.begin(2, "second".into());
        a.apply(1, Delta::Text("late text from the old turn".into()));
        assert_eq!(a.turns.last().unwrap().text, "");
    }

    #[test]
    fn reasoning_is_kept_out_of_the_answer() {
        let mut a = assistant();
        a.begin(1, "q".into());
        a.apply(1, Delta::Thinking("weighing it up".into()));
        a.apply(1, Delta::Text("The answer.".into()));
        assert_eq!(a.turns[1].text, "The answer.");
        assert_eq!(a.turns[1].thinking, "weighing it up");
    }

    #[test]
    fn a_truncated_answer_says_so() {
        // Otherwise it just looks like the model stopped mid-sentence.
        let mut a = assistant();
        a.begin(1, "q".into());
        a.apply(1, Delta::Text("Half an ans".into()));
        a.apply(1, Delta::Done(StopReason::MaxTokens));
        assert!(a.turns[1].note.as_deref().unwrap().contains("length limit"));
    }

    #[test]
    fn a_refusal_is_shown_rather_than_leaving_an_empty_bubble() {
        let mut a = assistant();
        a.begin(1, "q".into());
        a.apply(1, Delta::Done(StopReason::Refusal("policy".into())));
        assert!(a.turns[1].note.as_deref().unwrap().contains("policy"));
    }

    #[test]
    fn an_empty_reply_with_no_reason_is_still_explained() {
        // An empty bubble with nothing in it looks like a bug in Quark.
        let mut a = assistant();
        a.begin(1, "q".into());
        a.apply(1, Delta::Done(StopReason::EndTurn));
        assert!(a.turns[1].note.is_some());
    }

    #[test]
    fn a_failure_closes_the_turn_so_the_input_unlocks() {
        // Leaving `in_flight` set would disable the composer permanently.
        let mut a = assistant();
        a.begin(1, "q".into());
        a.fail(1, "No API key set for Anthropic.".into());
        assert!(a.in_flight.is_none());
        assert!(a.turns[1].note.is_some());
    }

    #[test]
    fn failed_turns_are_left_out_of_the_history() {
        // An empty assistant message is rejected outright by some providers.
        let mut a = assistant();
        a.begin(1, "q".into());
        a.fail(1, "boom".into());
        let h = a.history();
        assert_eq!(h.len(), 1, "the empty reply leaked into the history: {h:?}");
        assert_eq!(h[0].role, quark_ai::Role::User);
    }

    #[test]
    fn history_alternates_user_and_assistant() {
        let mut a = assistant();
        a.begin(1, "one".into());
        a.apply(1, Delta::Text("first answer".into()));
        a.apply(1, Delta::Done(StopReason::EndTurn));
        a.begin(2, "two".into());
        a.apply(2, Delta::Text("second answer".into()));
        a.apply(2, Delta::Done(StopReason::EndTurn));
        let roles: Vec<_> = a.history().iter().map(|m| m.role).collect();
        use quark_ai::Role::{Assistant, User};
        assert_eq!(roles, vec![User, Assistant, User, Assistant]);
    }

    #[test]
    fn sending_is_blocked_while_a_turn_is_in_flight_or_the_draft_is_blank() {
        let mut a = assistant();
        assert!(!a.can_send(), "an empty draft should not send");
        a.draft = "   ".into();
        assert!(!a.can_send(), "whitespace should not send");
        a.draft = "a question".into();
        assert!(a.can_send());
        a.begin(1, "a question".into());
        assert!(!a.can_send(), "two turns at once");
    }

    #[test]
    fn backend_and_scope_round_trip_through_their_preference_strings() {
        // Preferences are TOML strings, so a typo here silently resets the
        // user's choice to the default on every launch.
        for b in Backend::ALL {
            let p = Prefs {
                ai_backend: backend_key(b).into(),
                ..Prefs::default()
            };
            assert_eq!(backend_of(&p), b, "{b:?}");
        }
        for s in Scope::ALL {
            let p = Prefs {
                ai_scope: scope_key(s).into(),
                ..Prefs::default()
            };
            assert_eq!(scope_of(&p), s, "{s:?}");
        }
    }

    #[test]
    fn an_unknown_backend_falls_back_to_the_local_one() {
        // A hand-edited or future settings file must not silently start
        // sending documents to a cloud provider.
        let p = Prefs {
            ai_backend: "something-else".into(),
            ..Prefs::default()
        };
        assert_eq!(backend_of(&p), Backend::Ollama);
        assert!(!backend_of(&p).leaves_the_machine());
    }
}
