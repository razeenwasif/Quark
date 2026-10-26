//! `file:` URIs, which are the only part of LSP that Windows makes interesting.
//!
//! A server speaks URIs and an editor speaks paths, so every message crosses
//! this boundary twice. `C:\src\main.rs` has to become `file:///C:/src/main.rs`
//! — three slashes, forward slashes, an unencoded drive colon — and a path with
//! a space in it has to survive the trip back unchanged.

use std::path::{Path, PathBuf};

/// Characters that never need escaping, per RFC 3986's `unreserved` set. `/`
/// is added because it is the separator we are building, and `:` because the
/// drive letter's colon is conventionally left bare and servers expect it.
fn is_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Strip Windows' extended-length prefix.
///
/// `Path::canonicalize` hands back `\\?\C:\src\main.rs`. It names the same
/// file, but no language server accepts that spelling, and it is not what
/// comes back in a URI either — so a path canonicalised on the way in would
/// never match the diagnostics that came back for it. Both directions
/// normalise through here, which is what makes those two spellings one key.
pub fn plain(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share` is the share `\\server\share`.
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

pub fn from_path(path: impl AsRef<Path>) -> String {
    let path = plain(path);
    let text = path.to_string_lossy().replace('\\', "/");

    let mut uri = String::from("file://");
    // A Windows path starts at a drive letter rather than at `/`, so the
    // authority-terminating slash has to be supplied.
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        if is_safe(byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///c/x` and `file://localhost/c/x` both mean the local machine;
    // anything else is a host we cannot open.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);

    let decoded = decode(rest)?;
    // `/C:/src` is a Windows path wearing a URI's leading slash.
    let trimmed = match decoded.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &decoded[1..],
        _ => decoded.as_str(),
    };
    Some(PathBuf::from(
        trimmed.replace('/', std::path::MAIN_SEPARATOR_STR),
    ))
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_path_gets_three_slashes_and_a_bare_drive_colon() {
        assert_eq!(
            from_path(r"C:\src\proton\main.rs"),
            "file:///C:/src/proton/main.rs"
        );
    }

    #[test]
    fn a_unix_path_keeps_its_own_leading_slash() {
        assert_eq!(from_path("/home/r/main.rs"), "file:///home/r/main.rs");
    }

    #[test]
    fn spaces_and_other_awkwardness_are_percent_encoded() {
        let uri = from_path(r"C:\My Code\a+b\ünicode.rs");
        assert_eq!(uri, "file:///C:/My%20Code/a%2Bb/%C3%BCnicode.rs");
        // And survive the trip back, which is the only thing that matters.
        assert_eq!(
            to_path(&uri).unwrap(),
            PathBuf::from(format!(
                "C:{sep}My Code{sep}a+b{sep}ünicode.rs",
                sep = std::path::MAIN_SEPARATOR
            ))
        );
    }

    #[test]
    fn a_path_survives_the_round_trip() {
        for path in [r"C:\src\main.rs", r"D:\a b\c.rs"] {
            let back = to_path(&from_path(path)).unwrap();
            assert_eq!(
                back,
                PathBuf::from(path.replace('\\', std::path::MAIN_SEPARATOR_STR))
            );
        }
    }

    #[test]
    fn an_extended_length_path_is_spelled_the_ordinary_way() {
        // What `canonicalize` returns on Windows. Left alone, it produces a
        // URI no server recognises — and, worse, one that never matches the
        // diagnostics that come back.
        assert_eq!(
            from_path(r"\\?\C:\src\main.rs"),
            "file:///C:/src/main.rs",
            "the verbatim prefix must not reach the wire"
        );
        assert_eq!(plain(r"\\?\C:\src"), PathBuf::from(r"C:\src"));
        assert_eq!(
            plain(r"\\?\UNC\host\share\x.rs"),
            PathBuf::from(r"\\host\share\x.rs")
        );
        // An ordinary path is left exactly as it was.
        assert_eq!(plain(r"C:\src\main.rs"), PathBuf::from(r"C:\src\main.rs"));
    }

    #[test]
    fn localhost_is_this_machine_and_a_real_host_is_not_a_path() {
        assert!(to_path("file://localhost/home/r/x.rs").is_some());
        assert!(to_path("https://example.com/x.rs").is_none());
    }
}
