//! An open copy of a paper's PDF, kept beside it: downloaded only when it
//! is a PDF — a link that gives a web page instead is turned away — and no
//! larger than the investigation keeps, written whole or not at all.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::model::Record;

use crate::http::{host_of, Failure, Http};

/// The longest a download may take.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// A PDF kept: how big, and its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub bytes: u64,
    pub sha256: String,
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// Where an open copy of a paper may be had, best first: the one its
/// sources named, then arXiv's when it is on arXiv — which a publisher's
/// page that turns out to be a sign-in leaves to fall back on. Over https,
/// as everything is asked.
pub fn links(record: &Record) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let named = record.pdf_url.iter().map(|u| match u.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => u.clone(),
    });
    for url in named.chain(record.arxiv.iter().map(|id| format!("https://arxiv.org/pdf/{id}"))) {
        if url.starts_with("https://") && !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// Download `url` to `dest` if it is a PDF of at most `cap` bytes. The
/// folder must be there already — a download finishing after its
/// investigation was deleted does not make it again — and `dest` must not
/// be: a copy the scientist put there is theirs.
pub fn download(http: &Http, url: &str, dest: &Path, cap: u64, cancel: &AtomicBool) -> Result<Saved, String> {
    let parent = dest.parent().filter(|p| p.is_dir()).ok_or("its folder is gone")?;
    if dest.exists() {
        return Err("a copy is there already".into());
    }
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("paper.pdf");
    let partial = parent.join(format!(".{name}.partial"));
    let result = (|| {
        let host = host_of(url);
        let response = http.get(url, &[("Accept", "application/pdf")], TIMEOUT, cap + 1, false, cancel).map_err(|f| f.to_string())?;
        if response.status != 200 {
            return Err(Failure::Status { host, status: response.status }.to_string());
        }
        if let Some(length) = response.header("content-length").and_then(|v| v.trim().parse::<u64>().ok()) {
            if length > cap {
                return Err(format!("it is {}, more than the {} a PDF is kept at", megabytes(length), megabytes(cap)));
            }
        }
        let mut body = response.body;
        let mut file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut total: u64 = 0;
        let mut head: Vec<u8> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::http::CANCELLED.to_string());
            }
            let n = body.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            if head.len() < 5 {
                head.extend_from_slice(&buffer[..n.min(5 - head.len())]);
                if head.len() == 5 && &head != b"%PDF-" {
                    return Err("the link gives a web page, not a PDF".into());
                }
            }
            total += n as u64;
            if total > cap {
                return Err(format!("it is more than the {} a PDF is kept at", megabytes(cap)));
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        if head.len() < 5 || &head != b"%PDF-" {
            return Err("the link gives something that is not a PDF".into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&partial, dest).map_err(|e| e.to_string())?;
        let sha256 = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Ok(Saved { bytes: total, sha256 })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Answer, FakeClock, Gates, Script};
    use std::sync::Arc;

    fn folder(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("quark-literature-pdf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn http(answers: Vec<Answer>) -> Http {
        let clock = Arc::new(FakeClock::default());
        Http::new(Arc::new(Script::new(clock.clone(), answers)), Arc::new(Gates::default()), clock, None)
    }

    #[test]
    fn a_pdf_is_kept_whole_with_its_digest_and_anything_else_is_not() {
        let dir = folder("kept");
        let cancel = AtomicBool::new(false);
        let pdf = b"%PDF-1.7\n% a paper\n%%EOF\n".to_vec();
        let saved = download(&http(vec![Ok((200, Vec::new(), pdf.clone()))]), "https://arxiv.org/pdf/1706.03762", &dir.join("P001.pdf"), 1024, &cancel).unwrap();
        assert_eq!(saved.bytes, pdf.len() as u64);
        assert_eq!(saved.sha256.len(), 64);
        assert_eq!(std::fs::read(dir.join("P001.pdf")).unwrap(), pdf);

        let page = b"<!doctype html><html>Sign in</html>".to_vec();
        let why = download(&http(vec![Ok((200, Vec::new(), page))]), "https://example.org/paper", &dir.join("P002.pdf"), 1024, &cancel).unwrap_err();
        assert!(why.contains("web page"), "{why}");
        let big = vec![("Content-Length".to_string(), "5000".to_string())];
        let why = download(&http(vec![Ok((200, big, pdf.clone()))]), "https://example.org/big.pdf", &dir.join("P003.pdf"), 1024, &cancel).unwrap_err();
        assert!(why.contains("more than"), "a stated size over the cap is turned away before reading: {why}");
        let mut huge = b"%PDF-1.7\n".to_vec();
        huge.extend(vec![b'x'; 4096]);
        let why = download(&http(vec![Ok((200, Vec::new(), huge))]), "https://example.org/huge.pdf", &dir.join("P004.pdf"), 1024, &cancel).unwrap_err();
        assert!(why.contains("more than"), "{why}");
        let why = download(&http(vec![Ok((403, Vec::new(), Vec::new()))]), "https://example.org/x.pdf", &dir.join("P005.pdf"), 1024, &cancel).unwrap_err();
        assert!(why.contains("403"), "{why}");
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["P001.pdf"], "nothing half-written is left behind");

        let why = download(&http(vec![Ok((200, Vec::new(), pdf.clone()))]), "https://arxiv.org/pdf/1", &dir.join("P001.pdf"), 1024, &cancel).unwrap_err();
        assert!(why.contains("there already"), "{why}");
        assert_eq!(std::fs::read(dir.join("P001.pdf")).unwrap(), pdf, "a copy there is left alone");

        let gone = dir.join("deleted").join("P006.pdf");
        assert!(download(&http(vec![Ok((200, Vec::new(), pdf))]), "https://arxiv.org/pdf/1", &gone, 1024, &cancel).is_err());
        assert!(!dir.join("deleted").exists(), "a folder that is gone is not made again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_open_copy_is_looked_for_where_its_sources_say_and_then_on_arxiv() {
        let record = Record { pdf_url: Some("http://www.example.org/paper.pdf".into()), arxiv: Some("2101.00001".into()), ..Record::default() };
        assert_eq!(links(&record), ["https://www.example.org/paper.pdf", "https://arxiv.org/pdf/2101.00001"]);
        let arxiv = Record { pdf_url: Some("https://arxiv.org/pdf/2101.00001".into()), arxiv: Some("2101.00001".into()), ..Record::default() };
        assert_eq!(links(&arxiv), ["https://arxiv.org/pdf/2101.00001"]);
        assert!(links(&Record::default()).is_empty());
    }
}
