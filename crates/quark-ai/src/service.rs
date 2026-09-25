//! The worker thread the panel talks to.
//!
//! Same shape as `PdfService`: send a [`Request`], poll [`Event`]s from the UI
//! thread, never block the frame loop. A chat answer takes tens of seconds, so
//! doing this on the UI thread would freeze the whole application for the
//! duration of every question.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};

use crate::provider::{AiError, ChatRequest, Delta, Provider};

/// Work for the service.
pub enum Request {
    /// Ask a question. The provider is supplied per-request so the credential
    /// never has to live inside the service.
    Ask {
        /// Identifies the turn, so a reply that arrives after the user has
        /// moved on can be discarded rather than appended to a new question.
        id: u64,
        provider: Box<dyn Provider>,
        request: ChatRequest,
    },
    /// Ask a provider what models it can serve.
    ///
    /// Goes through the worker like everything else: Ollama answers over HTTP,
    /// and doing that on the UI thread stalls the frame — for as long as a
    /// connection timeout if Ollama is not running.
    ListModels { provider: Box<dyn Provider> },
    /// Stop the turn in flight, if any.
    Cancel,
}

/// What the service reports back.
#[derive(Debug, Clone)]
pub enum Event {
    /// A piece of the answer.
    Delta { id: u64, delta: Delta },
    /// The models a provider reported, in response to `ListModels`.
    Models { names: Vec<String> },
    /// The turn failed. No further events for this `id`.
    Failed {
        id: u64,
        message: String,
        retryable: bool,
    },
}

pub struct AiService {
    tx: Sender<Request>,
    rx: Receiver<Event>,
    /// Shared with the worker so a cancel takes effect inside the read loop
    /// rather than only between turns.
    cancel: Arc<AtomicBool>,
    next_id: u64,
}

impl AiService {
    pub fn start() -> Self {
        let (tx, work_rx) = unbounded::<Request>();
        let (event_tx, rx) = unbounded::<Event>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);

        std::thread::Builder::new()
            .name("quark-ai".into())
            .spawn(move || worker(work_rx, event_tx, worker_cancel))
            .expect("spawning the AI worker");

        Self {
            tx,
            rx,
            cancel,
            next_id: 1,
        }
    }

    /// Allocates the id for the next turn.
    pub fn new_turn(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn ask(&self, id: u64, provider: Box<dyn Provider>, request: ChatRequest) {
        // Clearing here rather than in the worker: a cancel from the previous
        // turn must not abort the one being sent now.
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Request::Ask {
            id,
            provider,
            request,
        });
    }

    /// Asks a provider for its model list; the answer arrives as
    /// [`Event::Models`].
    pub fn list_models(&self, provider: Box<dyn Provider>) {
        let _ = self.tx.send(Request::ListModels { provider });
    }

    /// Stops the turn in flight. Safe to call when nothing is running.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Request::Cancel);
    }

    /// Non-blocking drain, for the frame loop.
    pub fn try_recv(&self) -> Option<Event> {
        match self.rx.try_recv() {
            Ok(e) => Some(e),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

fn worker(rx: Receiver<Request>, tx: Sender<Event>, cancel: Arc<AtomicBool>) {
    while let Ok(req) = rx.recv() {
        match req {
            Request::ListModels { provider } => {
                // A failure here is not worth an error bubble: the picker just
                // stays empty and the user can type a name.
                let names = provider.models().unwrap_or_default();
                let _ = tx.send(Event::Models { names });
            }
            // A cancel that arrives with nothing running has already done its
            // job by setting the flag.
            Request::Cancel => {}
            Request::Ask {
                id,
                provider,
                request,
            } => {
                let result = provider.chat(&request, &cancel, &mut |delta| {
                    let _ = tx.send(Event::Delta { id, delta });
                });
                if let Err(e) = result {
                    let retryable = e.is_retryable();
                    let _ = tx.send(Event::Failed {
                        id,
                        message: describe(&e),
                        retryable,
                    });
                }
            }
        }
    }
}

/// Turns an error into something worth showing a reader.
///
/// The raw status codes mean nothing to someone who just wanted to ask about a
/// page, so the ones with an obvious remedy get one.
fn describe(e: &AiError) -> String {
    match e {
        AiError::NoKey(p) => {
            format!("No API key set for {p}. Add one in Settings.")
        }
        AiError::Status { status: 401, .. } | AiError::Status { status: 403, .. } => {
            "The API key was rejected. Check it in Settings.".into()
        }
        AiError::Status { status: 429, .. } => {
            "Rate limited by the provider. Wait a moment and try again.".into()
        }
        AiError::Status { status: 404, provider, .. } => {
            format!("{provider} does not recognise that model name.")
        }
        AiError::Status { status, provider, .. } if *status >= 500 => {
            format!("{provider} is having trouble ({status}). Try again shortly.")
        }
        AiError::Transport { provider, .. } => {
            format!("Could not reach {provider}. Check the connection, or the host if it is local.")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Message, StopReason};

    /// A provider that answers from a script, without a network.
    struct Fake {
        deltas: Vec<Delta>,
        fail: Option<AiError>,
    }

    impl Provider for Fake {
        fn name(&self) -> &str {
            "Fake"
        }
        fn models(&self) -> Result<Vec<String>, AiError> {
            Ok(vec![])
        }
        fn chat(
            &self,
            _req: &ChatRequest,
            cancel: &AtomicBool,
            on: &mut dyn FnMut(Delta),
        ) -> Result<(), AiError> {
            if let Some(e) = &self.fail {
                return Err(match e {
                    AiError::NoKey(p) => AiError::NoKey(p.clone()),
                    AiError::Status {
                        provider,
                        status,
                        body,
                    } => AiError::Status {
                        provider: provider.clone(),
                        status: *status,
                        body: body.clone(),
                    },
                    _ => AiError::Other("failed".into()),
                });
            }
            for d in &self.deltas {
                if cancel.load(Ordering::Relaxed) {
                    on(Delta::Done(StopReason::Cancelled));
                    return Ok(());
                }
                on(d.clone());
            }
            Ok(())
        }
    }

    fn request() -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            system: "s".into(),
            messages: vec![Message::user("q")],
            max_tokens: 16,
        }
    }

    fn drain(svc: &AiService, want: usize) -> Vec<Event> {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while out.len() < want && std::time::Instant::now() < deadline {
            if let Some(e) = svc.try_recv() {
                out.push(e);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        out
    }

    #[test]
    fn deltas_arrive_in_order_and_carry_their_turn_id() {
        // Out-of-order text would scramble the answer; a missing id would let a
        // stale reply append itself to the next question.
        let mut svc = AiService::start();
        let id = svc.new_turn();
        svc.ask(
            id,
            Box::new(Fake {
                deltas: vec![
                    Delta::Text("Hello".into()),
                    Delta::Text(" world".into()),
                    Delta::Done(StopReason::EndTurn),
                ],
                fail: None,
            }),
            request(),
        );
        let events = drain(&svc, 3);
        assert_eq!(events.len(), 3, "got {events:?}");
        let texts: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                Event::Delta {
                    delta: Delta::Text(t),
                    id: got,
                } => {
                    assert_eq!(*got, id);
                    Some(t.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts.join(""), "Hello world");
    }

    #[test]
    fn turn_ids_are_unique_and_increasing() {
        // Reusing an id makes a cancelled turn's late deltas indistinguishable
        // from the new turn's.
        let mut svc = AiService::start();
        let ids: Vec<u64> = (0..5).map(|_| svc.new_turn()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    }

    #[test]
    fn a_failure_is_reported_once_with_a_readable_message() {
        let mut svc = AiService::start();
        let id = svc.new_turn();
        svc.ask(
            id,
            Box::new(Fake {
                deltas: vec![],
                fail: Some(AiError::NoKey("Anthropic".into())),
            }),
            request(),
        );
        let events = drain(&svc, 1);
        match events.first() {
            Some(Event::Failed {
                id: got,
                message,
                retryable,
            }) => {
                assert_eq!(*got, id);
                assert!(message.contains("Settings"), "{message}");
                assert!(!retryable, "a missing key never fixes itself");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn asking_again_clears_a_previous_cancel() {
        // Otherwise one stop click silently kills every later question.
        let mut svc = AiService::start();
        svc.cancel();
        let id = svc.new_turn();
        svc.ask(
            id,
            Box::new(Fake {
                deltas: vec![Delta::Text("ok".into()), Delta::Done(StopReason::EndTurn)],
                fail: None,
            }),
            request(),
        );
        let events = drain(&svc, 2);
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::Delta {
                    delta: Delta::Text(t),
                    ..
                } if t == "ok"
            )),
            "the new turn was cancelled by the old flag: {events:?}"
        );
    }

    #[test]
    fn rate_limits_read_as_retryable_and_bad_keys_do_not() {
        assert!(describe(&AiError::Status {
            provider: "Anthropic".into(),
            status: 429,
            body: String::new()
        })
        .contains("try again"));
        assert!(describe(&AiError::Status {
            provider: "Anthropic".into(),
            status: 401,
            body: String::new()
        })
        .contains("Settings"));
    }

    #[test]
    fn an_unreachable_local_host_says_to_check_it() {
        // The commonest Ollama failure by far is that it simply is not running.
        let msg = describe(&AiError::Transport {
            provider: "Ollama".into(),
            source: Box::new(ureq::Error::HostNotFound),
        });
        assert!(msg.contains("Ollama"), "{msg}");
        assert!(msg.contains("local"), "{msg}");
    }
}
