//! Fixtures for the engine tests.
//!
//! The sample document is written out by hand rather than generated through
//! PDFium, so the tests exercise the real parser against bytes we control and
//! do not depend on the writer to test the reader.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialises access to PDFium across the test harness.
///
/// PDFium is not safe to call concurrently: running these tests in parallel
/// reliably segfaults inside the library. Production code never hits this
/// because [`crate::service`] confines every PDFium call to one worker thread,
/// but `cargo test` spawns a thread per test, so each test that touches the
/// engine takes this lock first.
///
/// Poisoning is deliberately ignored — a panicking test leaves PDFium's own
/// state untouched, and refusing the lock afterwards would turn one failure
/// into a cascade of unrelated ones.
pub fn pdfium_guard() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A scratch path under the system temp directory.
pub fn tmp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("quark-test-{}-{name}", std::process::id()));
    p
}

/// Bytes of a two-page PDF with real text, a filled rectangle and two fonts.
pub fn sample_bytes() -> Vec<u8> {
    let mut objs: Vec<(u32, Vec<u8>)> = Vec::new();
    let content1 = b"BT /F1 36 Tf 72 700 Td (Quark PDF Engine) Tj ET\n\
BT /F1 14 Tf 72 660 Td (Page one body text for extraction tests.) Tj ET\n\
0 0 1 rg 72 400 200 120 re f\n"
        .to_vec();
    let content2 = b"BT /F1 24 Tf 72 700 Td (Second page heading) Tj ET\n\
BT /F1 12 Tf 72 670 Td (findable-token-alpha) Tj ET\n\
BT /F1 12 Tf 72 640 Td (repeated repeated repeated) Tj ET\n"
        .to_vec();

    objs.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    objs.push((2, b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec()));
    objs.push((
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
    ));
    objs.push((4, stream(&content1)));
    objs.push((
        5,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ));
    objs.push((
        6,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
/Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>"
            .to_vec(),
    ));
    objs.push((7, stream(&content2)));

    assemble(&objs, 1)
}

fn stream(body: &[u8]) -> Vec<u8> {
    let mut v = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    v.extend_from_slice(body);
    v.extend_from_slice(b"endstream");
    v
}

/// Writes objects out with a correct cross-reference table.
fn assemble(objs: &[(u32, Vec<u8>)], root: u32) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (n, body) in objs {
        offsets.push((*n, out.len()));
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let max = objs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    out.extend_from_slice(format!("xref\n0 {}\n", max + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for n in 1..=max {
        let off = offsets
            .iter()
            .find(|(m, _)| *m == n)
            .map(|(_, o)| *o)
            .unwrap_or(0);
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n", max + 1)
            .as_bytes(),
    );
    out
}

static SAMPLE: OnceLock<PathBuf> = OnceLock::new();

/// Path to the shared two-page sample, written once per test run.
pub fn sample_pdf() -> PathBuf {
    SAMPLE
        .get_or_init(|| {
            let p = tmp_path("sample.pdf");
            std::fs::write(&p, sample_bytes()).expect("write sample pdf");
            p
        })
        .clone()
}

/// A many-page document, for pagination and layout tests.
pub fn many_page_pdf(pages: usize) -> PathBuf {
    let p = tmp_path(&format!("many-{pages}.pdf"));
    let mut objs: Vec<(u32, Vec<u8>)> = Vec::new();
    // 1 catalog, 2 pages tree, 3 font, then page/content pairs from 4.
    let font_id = 3u32;
    let mut kids = Vec::new();
    let mut next = 4u32;
    let mut page_objs = Vec::new();
    for i in 0..pages {
        let page_id = next;
        let content_id = next + 1;
        next += 2;
        kids.push(format!("{page_id} 0 R"));
        page_objs.push((
            page_id,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
/Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        ));
        let body = format!("BT /F1 24 Tf 72 700 Td (Page {} marker) Tj ET\n", i + 1);
        page_objs.push((content_id, stream(body.as_bytes())));
    }

    objs.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    objs.push((
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        )
        .into_bytes(),
    ));
    objs.push((
        font_id,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ));
    objs.extend(page_objs);

    std::fs::write(&p, assemble(&objs, 1)).expect("write many-page pdf");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_is_a_plausible_pdf() {
        let b = sample_bytes();
        assert!(b.starts_with(b"%PDF-"));
        assert!(b.ends_with(b"%%EOF\n"));
        assert!(b.windows(5).any(|w| w == b"xref\n"));
    }

    #[test]
    fn many_page_fixture_scales() {
        let p = many_page_pdf(12);
        assert!(p.exists());
        let _ = std::fs::remove_file(p);
    }
}
