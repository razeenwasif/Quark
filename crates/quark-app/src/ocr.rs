//! Reading pages with OCR, behind the window: each page drawn by PDFium at
//! 300 dpi on a thread of its own and read by tesseract (`quark-lab`), one
//! page at a time, so the document stays readable meanwhile. What is read
//! comes back whole, to be laid over the pages as one change.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use quark_lab::ocr::{Tesseract, Unread};
use quark_pdf::service::DocId;

/// Dots to an inch the pages are drawn at: what tesseract reads best.
const DPI: f32 = 300.0;

/// What a reading has come to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OcrNews {
    /// `done` of the pages read.
    Read(usize),
    /// Every page read: each one's text-only PDF.
    Done(Vec<(usize, Arc<Vec<u8>>)>),
    Stopped,
    Failed(String),
}

/// A reading under way.
pub(crate) struct OcrRun {
    pub(crate) doc: DocId,
    /// Which layout of the document's pages was read.
    pub(crate) layout: u64,
    pub(crate) done: usize,
    pub(crate) of: usize,
    news: Receiver<OcrNews>,
    stop: Arc<AtomicBool>,
}

impl OcrRun {
    /// Read `pages` (from 0) of the document `bytes`, in `langs`.
    pub(crate) fn start(
        doc: DocId,
        layout: u64,
        bytes: Arc<Vec<u8>>,
        pages: Vec<usize>,
        langs: Vec<String>,
        tesseract: Tesseract,
    ) -> OcrRun {
        let (tell, news) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let of = pages.len();
        let stopping = stop.clone();
        let spawned = std::thread::Builder::new().name("quark-ocr".into()).spawn(move || {
            let said = read(&bytes, &pages, &langs, &tesseract, &stopping, &tell);
            let _ = tell.send(said);
        });
        if let Err(e) = spawned {
            let (tell, news2) = mpsc::channel();
            let _ = tell.send(OcrNews::Failed(e.to_string()));
            return OcrRun { doc, layout, done: 0, of, news: news2, stop };
        }
        OcrRun { doc, layout, done: 0, of, news, stop }
    }

    /// Stop it: the page being read is let go of, and nothing is laid over.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// What has come of it since: how far, or how it ended.
    pub(crate) fn poll(&mut self) -> Option<OcrNews> {
        loop {
            match self.news.try_recv() {
                Ok(OcrNews::Read(done)) => self.done = done,
                Ok(end) => return Some(end),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(OcrNews::Failed("the reading stopped".into())),
            }
        }
    }
}

/// A folder of the reading's own, which only its owner may look in: the
/// pages drawn are in it. Gone when it is dropped.
struct Work(PathBuf);

impl Work {
    fn new() -> std::io::Result<Work> {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("quark-ocr-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Work(dir))
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Each page drawn and read in turn; how it ended.
fn read(
    bytes: &[u8],
    pages: &[usize],
    langs: &[String],
    tesseract: &Tesseract,
    stop: &AtomicBool,
    tell: &mpsc::Sender<OcrNews>,
) -> OcrNews {
    let work = match Work::new() {
        Ok(w) => w,
        Err(e) => return OcrNews::Failed(format!("no folder to read in: {e}")),
    };
    let doc = match quark_pdf::doc::Document::from_bytes(bytes.to_vec(), None) {
        Ok(d) => d,
        Err(e) => return OcrNews::Failed(format!("the document could not be drawn: {e}")),
    };
    let kind = quark_lab::sandbox::available();
    let mut read = Vec::new();
    for (i, &page) in pages.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return OcrNews::Stopped;
        }
        let picture = work.0.join(format!("p{}.png", page + 1));
        let drawn = quark_pdf::render::render_page(
            &doc,
            &quark_pdf::render::RenderRequest {
                page,
                scale: DPI / 72.0,
                rotation: quark_core::geom::Rot::D0,
                tint: quark_core::prefs::PageTint::None,
                annotations: false,
                forms: false,
                smooth: true,
            },
        )
        .map_err(|e| e.to_string())
        .and_then(|r| {
            image::RgbaImage::from_raw(r.width, r.height, r.rgba).ok_or_else(|| "a page drawn wrong".to_string())
        })
        .and_then(|img| img.save(&picture).map_err(|e| e.to_string()));
        if let Err(e) = drawn {
            return OcrNews::Failed(format!("page {} could not be drawn: {e}", page + 1));
        }
        match tesseract.read(&picture, langs, kind, stop) {
            Ok(words) => read.push((page, Arc::new(words))),
            Err(Unread::Stopped) => return OcrNews::Stopped,
            Err(why) => return OcrNews::Failed(format!("page {}: {why}", page + 1)),
        }
        let _ = std::fs::remove_file(&picture);
        let _ = tell.send(OcrNews::Read(i + 1));
    }
    OcrNews::Done(read)
}

/// The pages a reading is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    /// The pages with no words to find: a scan's.
    Blank,
    /// Those chosen, or the page being read.
    Chosen,
    Every,
}

/// The pages `which` names, of a document of `count` pages.
pub(crate) fn pages_for(which: Which, blank: &[usize], chosen: &[usize], count: usize) -> Vec<usize> {
    match which {
        Which::Blank => blank.to_vec(),
        Which::Chosen => chosen.to_vec(),
        Which::Every => (0..count).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pages_read_are_the_blank_the_chosen_or_every_one() {
        assert_eq!(pages_for(Which::Blank, &[1, 4], &[0], 5), [1, 4]);
        assert_eq!(pages_for(Which::Chosen, &[1, 4], &[0, 2], 5), [0, 2]);
        assert_eq!(pages_for(Which::Every, &[1, 4], &[0], 3), [0, 1, 2]);
    }

    #[test]
    fn a_reading_draws_each_page_reads_it_and_is_stopped_when_asked() {
        use std::os::unix::fs::PermissionsExt;
        let _g = quark_pdf::testutil::pdfium_guard();
        let base = std::env::temp_dir().join(format!("quark-app-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // tesseract standing in: says the picture it was given was a PNG, as
        // its words.
        let stand_in = |name: &str, script: &str| {
            let path = base.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Tesseract { path }
        };
        let tesseract = stand_in("reads", "head -c 4 \"$1\" | tail -c 3 > \"$2.pdf\"");
        let bytes = Arc::new(std::fs::read(quark_pdf::testutil::sample_pdf()).unwrap());
        let mut run = OcrRun::start(7, 3, bytes.clone(), vec![1, 0], vec!["eng".into()], tesseract);
        let end = settle(&mut run);
        let OcrNews::Done(read) = end else { panic!("{end:?}") };
        assert_eq!(read.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [1, 0], "in the order asked");
        assert_eq!(read[0].1.as_slice(), b"PNG", "each page drawn as a picture for tesseract");
        assert_eq!((run.done, run.of), (2, 2));

        let slow = stand_in("slow", "sleep 30");
        let mut run = OcrRun::start(7, 3, bytes, vec![0, 1], vec![], slow);
        std::thread::sleep(std::time::Duration::from_millis(300));
        run.stop();
        assert_eq!(settle(&mut run), OcrNews::Stopped);
        let _ = std::fs::remove_dir_all(base);
    }

    /// The real tesseract, on a page drawn by PDFium, its words then laid
    /// over a page and found there: run on purpose, with --ignored.
    #[test]
    #[ignore = "runs this Mac's tesseract"]
    fn live_tesseract_reads_a_drawn_page_and_its_words_are_found_on_the_page() {
        use quark_pdf::service::{DocOp, PdfService, Request, Response};
        let _g = quark_pdf::testutil::pdfium_guard();
        let Some(tesseract) = Tesseract::find() else { return };
        let source = quark_pdf::testutil::sample_pdf();
        let bytes = Arc::new(std::fs::read(&source).unwrap());
        let mut run = OcrRun::start(1, 0, bytes, vec![0], vec!["eng".into()], tesseract);
        let OcrNews::Done(read) = settle(&mut run) else { panic!("not read") };
        assert!(read[0].1.starts_with(b"%PDF"), "a PDF of the words");

        // Laid over a copy of the sample's second page, and found there.
        let copy = quark_pdf::testutil::tmp_path("live-ocr.pdf");
        std::fs::copy(&source, &copy).unwrap();
        let svc = PdfService::start();
        let id = svc.new_doc_id();
        svc.send(Request::Open { id, path: copy.clone(), password: None });
        let text = |svc: &PdfService| {
            svc.send(Request::PageText { id, page: 1 });
            wait(svc, |r| match r {
                Response::PageText { text, .. } => Some(text.text.clone()),
                _ => None,
            })
        };
        wait(&svc, |r| matches!(r, Response::Opened { .. }).then_some(()));
        let before = text(&svc);
        svc.send(Request::OcrPrep { id });
        let layout = wait(&svc, |r| match r {
            Response::OcrPrep { layout, .. } => Some(*layout),
            _ => None,
        });
        svc.send(Request::Apply { id, op: Box::new(DocOp::Ocr { layout, pages: vec![(1, read[0].1.clone())] }) });
        wait(&svc, |r| matches!(r, Response::Applied { .. }).then_some(()));
        let after = text(&svc);
        eprintln!("read: {after:?}");
        assert!(after.len() > before.len(), "the words read were laid over the page");
        let _ = std::fs::remove_file(copy);
    }

    fn wait<T>(svc: &quark_pdf::service::PdfService, mut f: impl FnMut(&quark_pdf::service::Response) -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            assert!(std::time::Instant::now() < deadline, "no answer");
            if let Some(r) = svc.recv_blocking() {
                if let quark_pdf::service::Response::Failed { error, .. } = &r {
                    panic!("{error}");
                }
                if let Some(v) = f(&r) {
                    return v;
                }
            }
        }
    }

    fn settle(run: &mut OcrRun) -> OcrNews {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(end) = run.poll() {
                return end;
            }
            assert!(std::time::Instant::now() < deadline, "it never ended");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
