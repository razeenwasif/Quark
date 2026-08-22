//! A worker thread that owns PDFium.
//!
//! # Why this exists
//!
//! PDFium is not safe to call from more than one thread. This is not a
//! theoretical constraint: running the engine tests in parallel segfaults
//! reliably. So exactly one thread ever touches the library, and everything
//! else talks to it by message.
//!
//! That also solves the second problem, which is latency. Rasterising an A4
//! page at 400% takes tens of milliseconds; doing it on the UI thread drops
//! frames on every scroll. Here the UI posts a request, keeps painting, and
//! picks up the finished raster whenever it arrives.
//!
//! Requests carry a `token`. The UI bumps the token whenever the view changes,
//! and discards any response carrying a stale one — which is what stops a
//! fast scroll from being followed by a burst of now-useless page images.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use quark_core::annot::Annotation;
use quark_core::geom::{PageSize, Rot};
use quark_core::prefs::PageTint;
use quark_core::search::{SearchHit, SearchOptions};

use crate::doc::{DocInfo, Document, Metadata, OpenError};
use crate::export::{BatesOptions, ImageExport, TextStamp};
use crate::forms::FormField;
use crate::outline::{Attachment, Bookmark};
use crate::render::{Raster, RenderRequest};
use crate::security::{PermissionSet, RedactionMark, RedactionReport, SanitizeOptions, SanitizeReport};
use crate::text::PageText;

/// Identifies an open document within the service.
pub type DocId = u64;

/// A structural change to a document.
///
/// These all go through the lopdf byte pipeline, so they are grouped into one
/// message rather than one message each: the worker applies the operation,
/// reloads the document, and replies with fresh [`DocInfo`].
#[derive(Debug, Clone)]
pub enum DocOp {
    DeletePages(Vec<usize>),
    MovePage { from: usize, to: usize },
    ReorderPages(Vec<usize>),
    RotatePages { pages: Vec<usize>, by: Rot },
    ExtractPages(Vec<usize>),
    InsertPagesFromFile { path: PathBuf, at: usize },
    InsertBlankPage { at: usize, size: PageSize },
    CropPages { pages: Vec<usize>, box_: [f32; 4] },
    SetMetadata(Box<Metadata>),
    AddAnnotations(Vec<Annotation>),
    RemoveAnnotations(Vec<usize>),
    SetOutline(Vec<Bookmark>),
    StampText { pages: Vec<usize>, stamp: Box<TextStamp> },
    ApplyBates { pages: Vec<usize>, options: Box<BatesOptions> },
    ApplyRedactions(Vec<RedactionMark>),
    Sanitize(SanitizeOptions),
    Encrypt { user: String, owner: String, permissions: PermissionSet },
    Decrypt { password: String },
    Optimize,
    SetFieldText { name: String, value: String },
    SetFieldCheck { name: String, checked: bool },
    ClearForm,
}

impl DocOp {
    /// A short description for the undo menu and the status bar.
    pub fn label(&self) -> String {
        match self {
            DocOp::DeletePages(p) => format!("Delete {} page(s)", p.len()),
            DocOp::MovePage { .. } => "Move page".into(),
            DocOp::ReorderPages(_) => "Reorder pages".into(),
            DocOp::RotatePages { pages, .. } => format!("Rotate {} page(s)", pages.len()),
            DocOp::ExtractPages(_) => "Extract pages".into(),
            DocOp::InsertPagesFromFile { .. } => "Insert pages".into(),
            DocOp::InsertBlankPage { .. } => "Insert blank page".into(),
            DocOp::CropPages { .. } => "Crop pages".into(),
            DocOp::SetMetadata(_) => "Change properties".into(),
            DocOp::AddAnnotations(a) => format!("Add {} annotation(s)", a.len()),
            DocOp::RemoveAnnotations(_) => "Remove annotations".into(),
            DocOp::SetOutline(_) => "Edit bookmarks".into(),
            DocOp::StampText { .. } => "Add watermark".into(),
            DocOp::ApplyBates { .. } => "Add Bates numbering".into(),
            DocOp::ApplyRedactions(m) => format!("Apply {} redaction(s)", m.len()),
            DocOp::Sanitize(_) => "Sanitise document".into(),
            DocOp::Encrypt { .. } => "Encrypt".into(),
            DocOp::Decrypt { .. } => "Remove security".into(),
            DocOp::Optimize => "Reduce file size".into(),
            DocOp::SetFieldText { name, .. } => format!("Fill \"{name}\""),
            DocOp::SetFieldCheck { name, .. } => format!("Toggle \"{name}\""),
            DocOp::ClearForm => "Clear form".into(),
        }
    }

    /// Whether the operation destroys information that cannot be recovered by
    /// undoing it in the UI.
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            DocOp::ApplyRedactions(_) | DocOp::Sanitize(_) | DocOp::Optimize
        )
    }
}

/// A message to the worker.
#[derive(Debug)]
pub enum Request {
    Open {
        id: DocId,
        path: PathBuf,
        password: Option<String>,
    },
    Close {
        id: DocId,
    },
    Render {
        id: DocId,
        request: RenderRequest,
        token: u64,
    },
    Thumbnail {
        id: DocId,
        page: usize,
        max_edge: u32,
        tint: PageTint,
        token: u64,
    },
    PageText {
        id: DocId,
        page: usize,
    },
    Search {
        id: DocId,
        query: String,
        options: SearchOptions,
        token: u64,
    },
    ReadAnnotations {
        id: DocId,
    },
    ReadOutline {
        id: DocId,
    },
    ReadFields {
        id: DocId,
    },
    ReadAttachments {
        id: DocId,
    },
    ReadLinks {
        id: DocId,
    },
    ExtractAttachment {
        id: DocId,
        index: usize,
        to: PathBuf,
    },
    Apply {
        id: DocId,
        op: Box<DocOp>,
    },
    Save {
        id: DocId,
        path: Option<PathBuf>,
    },
    ExportImages {
        id: DocId,
        pages: Vec<usize>,
        dir: PathBuf,
        stem: String,
        options: ImageExport,
    },
    ExportText {
        id: DocId,
        path: PathBuf,
    },
    Shutdown,
}

/// A reply from the worker.
#[derive(Debug)]
pub enum Response {
    Opened {
        id: DocId,
        info: Box<DocInfo>,
    },
    /// The file needs a password. Distinct from a failure so the UI can prompt.
    PasswordRequired {
        id: DocId,
        path: PathBuf,
        wrong: bool,
    },
    Failed {
        id: DocId,
        what: String,
        error: String,
    },
    Rendered {
        id: DocId,
        page: usize,
        token: u64,
        raster: Box<Raster>,
    },
    Thumbnail {
        id: DocId,
        page: usize,
        token: u64,
        raster: Box<Raster>,
    },
    PageText {
        id: DocId,
        page: usize,
        text: Box<PageText>,
    },
    SearchProgress {
        id: DocId,
        token: u64,
        page: usize,
        total: usize,
        hits: Vec<SearchHit>,
    },
    SearchFinished {
        id: DocId,
        token: u64,
    },
    Annotations {
        id: DocId,
        annotations: Vec<Annotation>,
    },
    Outline {
        id: DocId,
        bookmarks: Vec<Bookmark>,
    },
    Fields {
        id: DocId,
        fields: Vec<FormField>,
    },
    Attachments {
        id: DocId,
        attachments: Vec<Attachment>,
    },
    Links {
        id: DocId,
        links: Vec<crate::outline::Link>,
    },
    /// An operation succeeded and the document was reloaded.
    Applied {
        id: DocId,
        label: String,
        info: Box<DocInfo>,
        redaction: Option<RedactionReport>,
        sanitize: Option<SanitizeReport>,
    },
    Saved {
        id: DocId,
        path: PathBuf,
    },
    Exported {
        id: DocId,
        paths: Vec<PathBuf>,
    },
}

/// Handle to the PDFium worker thread.
pub struct PdfService {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    next_id: AtomicU64,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PdfService {
    /// Starts the worker.
    pub fn start() -> Self {
        let (req_tx, req_rx) = unbounded::<Request>();
        let (res_tx, res_rx) = unbounded::<Response>();

        let handle = std::thread::Builder::new()
            .name("quark-pdf".into())
            .spawn(move || worker(req_rx, res_tx))
            .expect("could not start the PDF worker thread");

        Self {
            tx: req_tx,
            rx: res_rx,
            next_id: AtomicU64::new(1),
            handle: Some(handle),
        }
    }

    /// A fresh document identifier.
    pub fn new_doc_id(&self) -> DocId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Posts a request. Never blocks.
    pub fn send(&self, req: Request) {
        // A send failure means the worker has gone; there is nothing useful to
        // do about it from a UI frame, and panicking would take the window with
        // it, so it is logged and dropped.
        if self.tx.send(req).is_err() {
            tracing::error!("PDF worker is not running; request dropped");
        }
    }

    /// Collects whatever the worker has finished, without blocking.
    pub fn poll(&self) -> Vec<Response> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(r) => out.push(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Blocks until the next response. Used only by tests.
    pub fn recv_blocking(&self) -> Option<Response> {
        self.rx.recv().ok()
    }
}

impl Drop for PdfService {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Shutdown);
        if let Some(h) = self.handle.take() {
            // Joining matters: PDFium is mid-teardown of its own global state,
            // and letting the process exit underneath it can abort.
            let _ = h.join();
        }
    }
}

/// The worker loop. Everything here runs on one thread.
fn worker(rx: Receiver<Request>, tx: Sender<Response>) {
    if let Err(e) = crate::engine::init() {
        tracing::error!("PDFium failed to initialise: {e}");
        // Every later request will fail; report the reason once rather than
        // per request.
        let _ = tx.send(Response::Failed {
            id: 0,
            what: "start the PDF engine".into(),
            error: e.to_string(),
        });
    }

    let mut docs: HashMap<DocId, Document> = HashMap::new();

    while let Ok(req) = rx.recv() {
        match req {
            Request::Shutdown => break,
            other => handle(other, &mut docs, &tx),
        }
    }
    tracing::debug!("PDF worker stopped");
}

/// Reports a failure in a form the UI can show verbatim.
fn fail(tx: &Sender<Response>, id: DocId, what: &str, error: impl std::fmt::Display) {
    let _ = tx.send(Response::Failed {
        id,
        what: what.to_owned(),
        error: error.to_string(),
    });
}

fn handle(req: Request, docs: &mut HashMap<DocId, Document>, tx: &Sender<Response>) {
    match req {
        Request::Shutdown => {}

        Request::Open { id, path, password } => match Document::open(&path, password.as_deref()) {
            Ok(doc) => {
                let info = Box::new(doc.info.clone());
                docs.insert(id, doc);
                let _ = tx.send(Response::Opened { id, info });
            }
            Err(OpenError::PasswordRequired) => {
                let _ = tx.send(Response::PasswordRequired {
                    id,
                    path,
                    wrong: false,
                });
            }
            Err(OpenError::WrongPassword) => {
                let _ = tx.send(Response::PasswordRequired {
                    id,
                    path,
                    wrong: true,
                });
            }
            Err(e) => fail(tx, id, &format!("open {}", path.display()), e),
        },

        Request::Close { id } => {
            docs.remove(&id);
        }

        Request::Render { id, request, token } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::render::render_page(doc, &request) {
                Ok(raster) => {
                    let _ = tx.send(Response::Rendered {
                        id,
                        page: request.page,
                        token,
                        raster: Box::new(raster),
                    });
                }
                Err(e) => fail(tx, id, &format!("render page {}", request.page + 1), e),
            }
        }

        Request::Thumbnail {
            id,
            page,
            max_edge,
            tint,
            token,
        } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::render::render_thumbnail(doc, page, max_edge, tint) {
                Ok(raster) => {
                    let _ = tx.send(Response::Thumbnail {
                        id,
                        page,
                        token,
                        raster: Box::new(raster),
                    });
                }
                // A thumbnail failure is not worth interrupting the user for;
                // the panel simply shows a placeholder.
                Err(e) => tracing::debug!("thumbnail for page {page} failed: {e}"),
            }
        }

        Request::PageText { id, page } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::text::extract_page(doc, page) {
                Ok(t) => {
                    let _ = tx.send(Response::PageText {
                        id,
                        page,
                        text: Box::new(t),
                    });
                }
                Err(e) => tracing::debug!("text extraction for page {page} failed: {e}"),
            }
        }

        Request::Search {
            id,
            query,
            options,
            token,
        } => {
            let Some(doc) = docs.get(&id) else { return };
            let total = doc.page_count();
            // Results are streamed a page at a time so the first hits appear
            // immediately on a long document rather than after the whole scan.
            for page in 0..total {
                let hits = crate::text::search_page(doc, page, &query, &options, 0)
                    .unwrap_or_default();
                let _ = tx.send(Response::SearchProgress {
                    id,
                    token,
                    page,
                    total,
                    hits,
                });
            }
            let _ = tx.send(Response::SearchFinished { id, token });
        }

        Request::ReadAnnotations { id } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::annots::read_all(doc) {
                Ok(annotations) => {
                    let _ = tx.send(Response::Annotations { id, annotations });
                }
                Err(e) => fail(tx, id, "read annotations", e),
            }
        }

        Request::ReadOutline { id } => {
            let Some(doc) = docs.get(&id) else { return };
            let _ = tx.send(Response::Outline {
                id,
                bookmarks: crate::outline::read_outline(doc),
            });
        }

        Request::ReadFields { id } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::forms::read_fields(doc) {
                Ok(fields) => {
                    let _ = tx.send(Response::Fields { id, fields });
                }
                Err(e) => fail(tx, id, "read form fields", e),
            }
        }

        Request::ReadAttachments { id } => {
            let Some(doc) = docs.get(&id) else { return };
            let _ = tx.send(Response::Attachments {
                id,
                attachments: crate::outline::read_attachments(doc),
            });
        }

        Request::ReadLinks { id } => {
            let Some(doc) = docs.get(&id) else { return };
            let _ = tx.send(Response::Links {
                id,
                links: crate::outline::read_links(doc),
            });
        }

        Request::ExtractAttachment { id, index, to } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::outline::extract_attachment(doc, index)
                .and_then(|b| {
                    std::fs::write(&to, b).map_err(|e| {
                        crate::engine::EngineError::Pdfium(format!("could not write: {e}"))
                    })
                }) {
                Ok(()) => {
                    let _ = tx.send(Response::Exported {
                        id,
                        paths: vec![to],
                    });
                }
                Err(e) => fail(tx, id, "extract attachment", e),
            }
        }

        Request::Apply { id, op } => apply_op(id, *op, docs, tx),

        Request::Save { id, path } => {
            let Some(doc) = docs.get_mut(&id) else { return };
            let result = match &path {
                Some(p) => doc.save_as(p),
                None => doc.save(),
            };
            match result {
                Ok(()) => {
                    let saved = doc.path().map(Path::to_path_buf).unwrap_or_default();
                    let _ = tx.send(Response::Saved { id, path: saved });
                }
                Err(e) => fail(tx, id, "save", e),
            }
        }

        Request::ExportImages {
            id,
            pages,
            dir,
            stem,
            options,
        } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::export::export_pages_as_images(doc, &pages, &dir, &stem, &options) {
                Ok(paths) => {
                    let _ = tx.send(Response::Exported { id, paths });
                }
                Err(e) => fail(tx, id, "export images", e),
            }
        }

        Request::ExportText { id, path } => {
            let Some(doc) = docs.get(&id) else { return };
            match crate::export::export_text(doc, &path) {
                Ok(()) => {
                    let _ = tx.send(Response::Exported {
                        id,
                        paths: vec![path],
                    });
                }
                Err(e) => fail(tx, id, "export text", e),
            }
        }
    }
}

use std::path::Path;

/// Applies a structural operation and reloads the document.
///
/// Every operation goes out to bytes and back, because that is the only way the
/// lopdf and PDFium halves of the engine can both see the change. The reload is
/// also what keeps `DocInfo` honest after the page tree moves.
fn apply_op(id: DocId, op: DocOp, docs: &mut HashMap<DocId, Document>, tx: &Sender<Response>) {
    let Some(doc) = docs.get(&id) else { return };
    let label = op.label();

    let bytes = match doc.to_bytes() {
        Ok(b) => b,
        Err(e) => return fail(tx, id, &label, e),
    };
    let path = doc.path().map(Path::to_path_buf);
    let filename = path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let password = doc.password().map(str::to_owned);

    let mut redaction = None;
    let mut sanitize = None;

    let result: Result<Vec<u8>, crate::engine::EngineError> = match op {
        DocOp::DeletePages(p) => crate::organize::delete_pages(&bytes, &p),
        DocOp::MovePage { from, to } => crate::organize::move_page(&bytes, from, to),
        DocOp::ReorderPages(o) => crate::organize::reorder_pages(&bytes, &o),
        DocOp::RotatePages { pages, by } => crate::organize::rotate_pages(&bytes, &pages, by),
        DocOp::ExtractPages(p) => crate::organize::extract_pages(&bytes, &p),
        DocOp::InsertPagesFromFile { path, at } => match std::fs::read(&path) {
            Ok(src) => crate::organize::insert_pages(&bytes, &src, at),
            Err(e) => Err(crate::engine::EngineError::Pdfium(format!(
                "could not read {}: {e}",
                path.display()
            ))),
        },
        DocOp::InsertBlankPage { at, size } => {
            // Built as a one-page document and spliced in, which reuses the
            // insert path rather than duplicating page-tree surgery.
            match Document::create_blank(size).and_then(|d| {
                d.to_bytes()
                    .map_err(|e| OpenError::Engine(e))
            }) {
                Ok(blank) => crate::organize::insert_pages(&bytes, &blank, at),
                Err(e) => Err(crate::engine::EngineError::Pdfium(e.to_string())),
            }
        }
        DocOp::CropPages { pages, box_ } => crate::organize::crop_pages(&bytes, &pages, box_),
        DocOp::SetMetadata(m) => crate::organize::set_metadata(&bytes, &m),
        DocOp::AddAnnotations(a) => crate::organize::apply_annotations(&bytes, &a),
        DocOp::RemoveAnnotations(pages) => crate::organize::remove_annotations(&bytes, &pages),
        DocOp::SetOutline(b) => crate::outline::write_outline(&bytes, &b),
        DocOp::StampText { pages, stamp } => {
            crate::export::stamp_text(&bytes, &pages, &stamp, &filename)
        }
        DocOp::ApplyBates { pages, options } => {
            crate::export::apply_bates(&bytes, &pages, &options)
        }
        DocOp::ApplyRedactions(marks) => match crate::security::apply_redactions(doc, &marks) {
            Ok((out, report)) => {
                redaction = Some(report);
                Ok(out)
            }
            Err(e) => Err(e),
        },
        DocOp::Sanitize(opts) => match crate::security::sanitize(&bytes, opts) {
            Ok((out, report)) => {
                sanitize = Some(report);
                Ok(out)
            }
            Err(e) => Err(e),
        },
        DocOp::Encrypt {
            user,
            owner,
            permissions,
        } => crate::security::encrypt(&bytes, &user, &owner, permissions),
        DocOp::Decrypt { password } => crate::security::decrypt(&bytes, &password),
        DocOp::Optimize => crate::organize::optimize(&bytes),

        // Form edits act on the live document rather than the byte pipeline,
        // because PDFium owns the field values and regenerates appearances.
        DocOp::SetFieldText { name, value } => {
            let Some(doc) = docs.get_mut(&id) else { return };
            match crate::forms::set_text_value(doc, &name, &value) {
                Ok(()) => {
                    doc.refresh_info();
                    let _ = tx.send(Response::Applied {
                        id,
                        label,
                        info: Box::new(doc.info.clone()),
                        redaction: None,
                        sanitize: None,
                    });
                }
                Err(e) => fail(tx, id, "fill field", e),
            }
            return;
        }
        DocOp::SetFieldCheck { name, checked } => {
            let Some(doc) = docs.get_mut(&id) else { return };
            match crate::forms::set_checkbox(doc, &name, checked) {
                Ok(()) => {
                    doc.refresh_info();
                    let _ = tx.send(Response::Applied {
                        id,
                        label,
                        info: Box::new(doc.info.clone()),
                        redaction: None,
                        sanitize: None,
                    });
                }
                Err(e) => fail(tx, id, "toggle field", e),
            }
            return;
        }
        DocOp::ClearForm => {
            let Some(doc) = docs.get_mut(&id) else { return };
            match crate::forms::clear_form(doc) {
                Ok(_) => {
                    doc.refresh_info();
                    let _ = tx.send(Response::Applied {
                        id,
                        label,
                        info: Box::new(doc.info.clone()),
                        redaction: None,
                        sanitize: None,
                    });
                }
                Err(e) => fail(tx, id, "clear form", e),
            }
            return;
        }
    };

    let new_bytes = match result {
        Ok(b) => b,
        Err(e) => return fail(tx, id, &label, e),
    };

    // Encryption changes which password opens the file, so the newly written
    // bytes must be reopened with the one that was just set, not the old one.
    let reopen_password = match &label {
        l if l == "Encrypt" => None,
        _ => password,
    };

    match Document::from_bytes(new_bytes, reopen_password.as_deref()) {
        Ok(mut reloaded) => {
            if let Some(p) = path {
                reloaded.set_path(p);
            }
            let info = Box::new(reloaded.info.clone());
            docs.insert(id, reloaded);
            let _ = tx.send(Response::Applied {
                id,
                label,
                info,
                redaction,
                sanitize,
            });
        }
        // The operation produced something PDFium will not open. The original
        // document is still in the map untouched, so the user loses the edit
        // rather than the file.
        Err(e) => fail(tx, id, &label, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;
    use std::time::{Duration, Instant};

    /// Waits for a response satisfying `pred`, failing the test on timeout.
    fn wait_for<T>(
        svc: &PdfService,
        mut pred: impl FnMut(&Response) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() > deadline {
                panic!("timed out waiting for a response");
            }
            match svc.recv_blocking() {
                Some(r) => {
                    if let Response::Failed { what, error, .. } = &r {
                        panic!("worker failed during {what}: {error}");
                    }
                    if let Some(v) = pred(&r) {
                        return v;
                    }
                }
                None => panic!("worker disconnected"),
            }
        }
    }

    fn open_sample(svc: &PdfService) -> DocId {
        let id = svc.new_doc_id();
        svc.send(Request::Open {
            id,
            path: testutil::sample_pdf(),
            password: None,
        });
        wait_for(svc, |r| match r {
            Response::Opened { id: i, info } => {
                assert_eq!(info.page_count, 2);
                Some(*i)
            }
            _ => None,
        });
        id
    }

    #[test]
    fn the_worker_opens_a_document_and_reports_it() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = open_sample(&svc);
        assert!(id > 0);
    }

    #[test]
    fn document_ids_are_unique() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let a = svc.new_doc_id();
        let b = svc.new_doc_id();
        assert_ne!(a, b);
    }

    #[test]
    fn a_render_comes_back_with_the_token_it_was_asked_with() {
        let _g = testutil::pdfium_guard();
        // The token is what lets the UI discard results for a view it has
        // already scrolled past.
        let svc = PdfService::start();
        let id = open_sample(&svc);
        svc.send(Request::Render {
            id,
            request: RenderRequest {
                page: 1,
                scale: 0.5,
                ..Default::default()
            },
            token: 4242,
        });
        let (page, token, w) = wait_for(&svc, |r| match r {
            Response::Rendered {
                page, token, raster, ..
            } => Some((*page, *token, raster.width)),
            _ => None,
        });
        assert_eq!(page, 1);
        assert_eq!(token, 4242);
        assert_eq!(w, 306);
    }

    #[test]
    fn opening_a_missing_file_reports_a_failure_rather_than_hanging() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = svc.new_doc_id();
        svc.send(Request::Open {
            id,
            path: PathBuf::from("/definitely/not/here.pdf"),
            password: None,
        });
        let got = match svc.recv_blocking() {
            Some(Response::Failed { id: i, .. }) => i,
            other => panic!("expected a failure, got {other:?}"),
        };
        assert_eq!(got, id);
    }

    #[test]
    fn an_encrypted_document_asks_for_a_password_instead_of_failing() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let enc = crate::security::encrypt(
            &testutil::sample_bytes(),
            "pw",
            "owner",
            PermissionSet::default(),
        )
        .unwrap();
        let path = testutil::tmp_path("svc-encrypted.pdf");
        std::fs::write(&path, enc).unwrap();

        let id = svc.new_doc_id();
        svc.send(Request::Open {
            id,
            path: path.clone(),
            password: None,
        });
        match svc.recv_blocking() {
            Some(Response::PasswordRequired { wrong, .. }) => {
                assert!(!wrong, "should not claim the password was wrong yet");
            }
            other => panic!("expected a password prompt, got {other:?}"),
        }

        // The wrong password is reported as wrong.
        svc.send(Request::Open {
            id,
            path: path.clone(),
            password: Some("nope".into()),
        });
        match svc.recv_blocking() {
            Some(Response::PasswordRequired { wrong, .. }) => assert!(wrong),
            other => panic!("expected WrongPassword, got {other:?}"),
        }

        // And the right one opens it.
        svc.send(Request::Open {
            id,
            path,
            password: Some("pw".into()),
        });
        let n = wait_for(&svc, |r| match r {
            Response::Opened { info, .. } => Some(info.page_count),
            _ => None,
        });
        assert_eq!(n, 2);
    }

    #[test]
    fn search_streams_results_page_by_page_and_then_finishes() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = open_sample(&svc);
        svc.send(Request::Search {
            id,
            query: "repeated".into(),
            options: SearchOptions::default(),
            token: 7,
        });

        let mut hits = 0;
        let mut pages_reported = 0;
        loop {
            match svc.recv_blocking() {
                Some(Response::SearchProgress { token, hits: h, .. }) => {
                    assert_eq!(token, 7);
                    pages_reported += 1;
                    hits += h.len();
                }
                Some(Response::SearchFinished { token, .. }) => {
                    assert_eq!(token, 7);
                    break;
                }
                Some(Response::Failed { what, error, .. }) => {
                    panic!("search failed during {what}: {error}")
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(pages_reported, 2, "should report every page");
        assert_eq!(hits, 3);
    }

    #[test]
    fn a_structural_edit_is_applied_and_the_document_reloaded() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = open_sample(&svc);
        svc.send(Request::Apply {
            id,
            op: Box::new(DocOp::DeletePages(vec![0])),
        });
        let (label, count) = wait_for(&svc, |r| match r {
            Response::Applied { label, info, .. } => Some((label.clone(), info.page_count)),
            _ => None,
        });
        assert_eq!(count, 1, "the page was not removed");
        assert!(label.contains("Delete"));

        // And the reloaded document is the one later requests see.
        svc.send(Request::Render {
            id,
            request: RenderRequest::default(),
            token: 1,
        });
        wait_for(&svc, |r| match r {
            Response::Rendered { .. } => Some(()),
            _ => None,
        });
    }

    #[test]
    fn a_failed_edit_leaves_the_original_document_open() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = open_sample(&svc);
        // Deleting every page is refused by the organiser.
        svc.send(Request::Apply {
            id,
            op: Box::new(DocOp::DeletePages(vec![0, 1])),
        });
        match svc.recv_blocking() {
            Some(Response::Failed { .. }) => {}
            other => panic!("expected a failure, got {other:?}"),
        }
        // The document must still be usable.
        svc.send(Request::Render {
            id,
            request: RenderRequest::default(),
            token: 9,
        });
        let token = wait_for(&svc, |r| match r {
            Response::Rendered { token, .. } => Some(*token),
            _ => None,
        });
        assert_eq!(token, 9);
    }

    #[test]
    fn annotations_added_through_the_service_read_back() {
        let _g = testutil::pdfium_guard();
        use quark_core::annot::{AnnotId, AnnotKind, Annotation};
        use quark_core::geom::Rect;

        let svc = PdfService::start();
        let id = open_sample(&svc);
        let ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Highlight {
                quads: vec![Rect::from_xywh(72.0, 690.0, 200.0, 24.0)],
            },
            Rect::from_xywh(72.0, 690.0, 200.0, 24.0),
            "svc",
        );
        svc.send(Request::Apply {
            id,
            op: Box::new(DocOp::AddAnnotations(vec![ann])),
        });
        wait_for(&svc, |r| match r {
            Response::Applied { .. } => Some(()),
            _ => None,
        });

        svc.send(Request::ReadAnnotations { id });
        let anns = wait_for(&svc, |r| match r {
            Response::Annotations { annotations, .. } => Some(annotations.clone()),
            _ => None,
        });
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].author, "svc");
    }

    #[test]
    fn saving_writes_the_file_the_caller_named() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = open_sample(&svc);
        let out = testutil::tmp_path("svc-saved.pdf");
        svc.send(Request::Save {
            id,
            path: Some(out.clone()),
        });
        let saved = wait_for(&svc, |r| match r {
            Response::Saved { path, .. } => Some(path.clone()),
            _ => None,
        });
        assert_eq!(saved, out);
        assert!(out.exists());
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn closing_a_document_stops_it_answering() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let id = open_sample(&svc);
        svc.send(Request::Close { id });
        svc.send(Request::Render {
            id,
            request: RenderRequest::default(),
            token: 1,
        });
        // Followed by something that does answer, so the test cannot pass just
        // by the render being slow.
        let id2 = svc.new_doc_id();
        svc.send(Request::Open {
            id: id2,
            path: testutil::sample_pdf(),
            password: None,
        });
        match svc.recv_blocking() {
            Some(Response::Opened { id: got, .. }) => assert_eq!(got, id2),
            other => panic!("closed document still answered: {other:?}"),
        }
    }

    #[test]
    fn polling_never_blocks_when_nothing_is_ready() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let start = Instant::now();
        let _ = svc.poll();
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn destructive_operations_are_flagged_as_such() {
        let _g = testutil::pdfium_guard();
        // The UI uses this to decide whether to ask for confirmation.
        assert!(DocOp::ApplyRedactions(vec![]).is_destructive());
        assert!(DocOp::Sanitize(SanitizeOptions::default()).is_destructive());
        assert!(!DocOp::MovePage { from: 0, to: 1 }.is_destructive());
        assert!(!DocOp::RotatePages {
            pages: vec![0],
            by: Rot::D90
        }
        .is_destructive());
    }
}
