//! The LSP base protocol: `Content-Length` framing over a byte stream.
//!
//! Small enough to read in one sitting, and the place almost every
//! language-server integration bug actually lives — so it is separated from
//! the process supervision above it and tested against plain in-memory
//! buffers rather than against a real server.

use std::io::{self, BufRead, Write};

/// Frame one message and push it out.
pub fn write_message(out: &mut impl Write, body: &[u8]) -> io::Result<()> {
    // No `Content-Type`: the spec's default is exactly what we send, and a
    // header nobody reads is a header that eventually goes stale.
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body)?;
    out.flush()
}

/// Read one framed message.
///
/// `Ok(None)` means the stream ended cleanly *between* messages — a server
/// that exited when we asked it to. Ending part-way through a message is an
/// error, because it means we lost something.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length: Option<usize> = None;
    let mut saw_header = false;
    let mut line = String::new();

    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return if saw_header {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended inside a message header",
                ))
            } else {
                Ok(None)
            };
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        saw_header = true;

        // Header names are case-insensitive, and unknown headers are ignored
        // rather than rejected: the spec allows them, and a client that trips
        // over one is a client that breaks on the next server that adds one.
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad Content-Length: {:?}", value.trim()),
                )
            })?);
        }
    }

    let Some(length) = length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message header had no Content-Length",
        ));
    };

    let mut body = vec![0u8; length];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_message_survives_the_round_trip() {
        let mut wire = Vec::new();
        write_message(&mut wire, br#"{"jsonrpc":"2.0"}"#).unwrap();
        assert_eq!(
            String::from_utf8(wire.clone()).unwrap(),
            "Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}"
        );

        let mut read = Cursor::new(wire);
        let body = read_message(&mut read).unwrap().unwrap();
        assert_eq!(body, br#"{"jsonrpc":"2.0"}"#);
    }

    #[test]
    fn the_length_is_bytes_not_characters() {
        // The one framing bug that only shows up once a user opens a file with
        // an emoji in it.
        let body = r#"{"text":"héllo ☃"}"#;
        let mut wire = Vec::new();
        write_message(&mut wire, body.as_bytes()).unwrap();
        let header = format!("Content-Length: {}", body.len());
        assert!(String::from_utf8_lossy(&wire).starts_with(&header));
        assert!(body.len() > body.chars().count());

        let mut read = Cursor::new(wire);
        let back = read_message(&mut read).unwrap().unwrap();
        assert_eq!(String::from_utf8(back).unwrap(), body);
    }

    #[test]
    fn extra_headers_are_ignored_not_rejected() {
        let wire = "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n\
                    Content-Length: 2\r\n\
                    X-Whatever: 1\r\n\r\n{}";
        let mut read = Cursor::new(wire.as_bytes().to_vec());
        assert_eq!(read_message(&mut read).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn messages_read_back_one_at_a_time() {
        let mut wire = Vec::new();
        write_message(&mut wire, b"{\"a\":1}").unwrap();
        write_message(&mut wire, b"{\"b\":2}").unwrap();
        let mut read = Cursor::new(wire);
        assert_eq!(read_message(&mut read).unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(read_message(&mut read).unwrap().unwrap(), b"{\"b\":2}");
        assert!(read_message(&mut read).unwrap().is_none());
    }

    #[test]
    fn a_truncated_message_is_an_error_not_an_ending() {
        let mut read = Cursor::new(b"Content-Length: 99\r\n\r\n{}".to_vec());
        assert!(read_message(&mut read).is_err());
    }
}
