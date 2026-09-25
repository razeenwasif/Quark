//! Server-sent event framing.
//!
//! Anthropic and the OpenAI-compatible endpoints both stream SSE; Ollama
//! streams bare NDJSON. The framing is separated from the wire formats above it
//! so it can be tested against captured bytes without a network, which is the
//! only practical way to cover the cases that actually break: a JSON object
//! split across two reads, a keep-alive comment, and a terminator that is not
//! JSON at all.

use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};

/// Reads an SSE stream, handing each `data:` payload to `on_data`.
///
/// Stops early and returns `Ok(false)` when `cancel` is set, so a stop button
/// takes effect on the next chunk rather than at the end of the response.
/// Returns `Ok(true)` when the stream ended on its own.
///
/// `on_data` returning `false` also stops the loop — that is how a terminal
/// event ends the read without waiting for the connection to close.
pub fn read_events<R: Read>(
    reader: R,
    cancel: &AtomicBool,
    mut on_data: impl FnMut(&str) -> bool,
) -> std::io::Result<bool> {
    for line in BufReader::new(reader).lines() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let line = line?;
        let Some(payload) = line.strip_prefix("data:") else {
            // `event:` names, `:` keep-alive comments and the blank lines
            // between events all carry nothing we need — the payload is
            // self-describing on every provider here.
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        // OpenAI-compatible endpoints terminate with a literal `[DONE]`, which
        // is not JSON; feeding it to a parser is a spurious error every turn.
        if payload == "[DONE]" {
            return Ok(true);
        }
        if !on_data(payload) {
            return Ok(true);
        }
    }
    Ok(true)
}

/// Reads a newline-delimited JSON stream, as Ollama emits.
///
/// Same contract as [`read_events`].
pub fn read_ndjson<R: Read>(
    reader: R,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str) -> bool,
) -> std::io::Result<bool> {
    for line in BufReader::new(reader).lines() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !on_line(line) {
            return Ok(true);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(bytes: &str) -> Vec<String> {
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        read_events(bytes.as_bytes(), &cancel, |d| {
            out.push(d.to_string());
            true
        })
        .unwrap();
        out
    }

    #[test]
    fn event_names_and_blank_lines_are_skipped() {
        // Anthropic sends an `event:` line before every `data:` line; taking
        // both as payloads doubles every chunk.
        let raw = "event: content_block_delta\n\
                   data: {\"a\":1}\n\
                   \n\
                   event: message_stop\n\
                   data: {\"b\":2}\n\n";
        assert_eq!(collect(raw), vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn keep_alive_comments_are_ignored() {
        // A bare `:` line keeps an idle connection open. Parsing it as JSON
        // fails on every heartbeat of a slow answer.
        let raw = ": keep-alive\ndata: {\"a\":1}\n\n: ping\n";
        assert_eq!(collect(raw), vec!["{\"a\":1}"]);
    }

    #[test]
    fn the_openai_done_sentinel_ends_the_stream_without_parsing() {
        // `[DONE]` is not JSON. Anything after it is not ours to read.
        let raw = "data: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"never\":true}\n";
        assert_eq!(collect(raw), vec!["{\"a\":1}"]);
    }

    #[test]
    fn payloads_survive_a_missing_space_after_the_colon() {
        // The spec allows `data:{...}` with no space; Anthropic sends one and
        // some proxies strip it.
        let raw = "data:{\"a\":1}\ndata: {\"b\":2}\n";
        assert_eq!(collect(raw), vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn cancelling_stops_the_read_partway() {
        // The stop button has to take effect during the answer, not after it.
        let raw = "data: {\"a\":1}\ndata: {\"b\":2}\ndata: {\"c\":3}\n";
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let finished = read_events(raw.as_bytes(), &cancel, |d| {
            seen.push(d.to_string());
            // Cancel after the first payload, as a stop click would.
            cancel.store(true, Ordering::Relaxed);
            true
        })
        .unwrap();
        assert_eq!(seen.len(), 1, "read past the cancel");
        assert!(!finished, "a cancelled stream did not run to completion");
    }

    #[test]
    fn returning_false_ends_the_stream_early() {
        let raw = "data: {\"a\":1}\ndata: {\"b\":2}\n";
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let finished = read_events(raw.as_bytes(), &cancel, |d| {
            seen.push(d.to_string());
            false
        })
        .unwrap();
        assert_eq!(seen.len(), 1);
        assert!(finished, "a terminal event is a normal end, not a cancel");
    }

    #[test]
    fn ndjson_yields_one_payload_per_line() {
        let raw = "{\"a\":1}\n{\"b\":2}\n\n{\"c\":3}\n";
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        read_ndjson(raw.as_bytes(), &cancel, |l| {
            out.push(l.to_string());
            true
        })
        .unwrap();
        assert_eq!(out, vec!["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"]);
    }
}
