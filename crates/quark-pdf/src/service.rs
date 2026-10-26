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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use quark_core::annot::{AnnotId, Annotation, Color};
use quark_core::geom::{PageSize, Rot, Vec2};
use quark_edit::{Doc as Edit, Op, Opened};
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
    RemoveAnnotations(Vec<AnnotId>),
    /// An annotation moved by `by`, in the page's points.
    MoveAnnotation { id: AnnotId, by: Vec2 },
    /// An annotation recoloured, or what it says changed.
    EditAnnotation { id: AnnotId, color: Option<Color>, contents: Option<String> },
    /// An answer to an annotation, by `author`.
    ReplyTo { id: AnnotId, text: String, author: String },
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
    /// A field set to a choice, or a button group to one of its states.
    SetFieldValue { name: String, value: String },
    /// The form drawn into its pages for good.
    FlattenForm,
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
            DocOp::MoveAnnotation { .. } => "Move annotation".into(),
            DocOp::EditAnnotation { .. } => "Edit annotation".into(),
            DocOp::ReplyTo { .. } => "Reply".into(),
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
            DocOp::SetFieldValue { name, .. } => format!("Fill \"{name}\""),
            DocOp::FlattenForm => "Flatten form".into(),
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
    /// A book's cover: its first page, no longer than `max_edge` on its long
    /// side — read from `keep` when it is there, and kept there when drawn.
    /// The file is opened for this alone and closed after.
    Cover {
        path: PathBuf,
        max_edge: u32,
        keep: Option<PathBuf>,
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
    /// Take back the last change, or make again the last taken back.
    Undo {
        id: DocId,
    },
    Redo {
        id: DocId,
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
    /// A cover, or none for a file that cannot be drawn.
    Cover {
        path: PathBuf,
        raster: Option<Box<Raster>>,
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
    let mut edits: Edits = HashMap::new();

    while let Ok(req) = rx.recv() {
        match req {
            Request::Shutdown => break,
            other => handle(other, &mut docs, &mut edits, &tx),
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

/// What has been changed in each document: `quark-edit`'s document, its
/// log of changes and what saving writes — or why the document can only be
/// read. PDFium draws what it says.
type Edits = HashMap<DocId, Result<Box<Edit>, String>>;

/// `info` told what can be undone and redone, and whether there are changes.
fn told(mut info: DocInfo, edit: Option<&Result<Box<Edit>, String>>) -> DocInfo {
    match edit {
        Some(Ok(edit)) => {
            info.undo = edit.undoable();
            info.redo = edit.redoable();
            info.dirty = edit.dirty();
            info.read_only = None;
        }
        Some(Err(why)) => {
            info.undo = None;
            info.redo = None;
            info.dirty = false;
            info.read_only = Some(why.clone());
        }
        None => {}
    }
    info
}

/// The file read to be changed, with the password it was opened with.
fn editable(path: &Path, password: Option<&str>) -> Result<Box<Edit>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("it could not be read again: {e}"))?;
    match Edit::open_with(std::sync::Arc::new(bytes), password) {
        Opened::Editable(edit) => Ok(edit),
        Opened::ReadOnly(why) => Err(why),
        Opened::Locked { .. } => Err("its password does not open it to be changed".into()),
    }
}

fn handle(req: Request, docs: &mut HashMap<DocId, Document>, edits: &mut Edits, tx: &Sender<Response>) {
    match req {
        Request::Shutdown => {}

        Request::Open { id, path, password } => match Document::open(&path, password.as_deref()) {
            Ok(doc) => {
                let edit = editable(&path, password.as_deref());
                let info = Box::new(told(doc.info.clone(), Some(&edit)));
                docs.insert(id, doc);
                edits.insert(id, edit);
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
            edits.remove(&id);
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

        Request::Cover { path, max_edge, keep } => {
            let raster = cover(&path, max_edge, keep.as_deref());
            let _ = tx.send(Response::Cover { path, raster: raster.map(Box::new) });
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
            // Read from the document of record, named by object: what a
            // change names them by, whatever has changed since.
            if let Some(Ok(edit)) = edits.get(&id) {
                let annotations = crate::edits::annotations(edit.document());
                let _ = tx.send(Response::Annotations { id, annotations });
                return;
            }
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
            // A widget each, read from the document of record, with each
            // box's and button's own "on".
            if let Some(Ok(edit)) = edits.get(&id) {
                let fields = crate::edits::fields(edit);
                let _ = tx.send(Response::Fields { id, fields });
                return;
            }
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

        Request::Apply { id, op } => apply_op(id, *op, docs, edits, tx),

        Request::Undo { id } | Request::Redo { id } => {
            let undo = matches!(req, Request::Undo { .. });
            let Some(Ok(edit)) = edits.get_mut(&id) else { return };
            let said = if undo { edit.undo() } else { edit.redo() };
            match said {
                Ok(Some(said)) => {
                    let label = if undo { format!("Undid {said}") } else { format!("Redid {said}") };
                    reload(id, label, docs, edits, tx, None, None);
                }
                Ok(None) => {}
                Err(e) => fail(tx, id, if undo { "undo" } else { "redo" }, e),
            }
        }

        Request::Save { id, path } => {
            let Some(doc) = docs.get_mut(&id) else { return };
            let target = path.clone().or_else(|| doc.path().map(Path::to_path_buf));
            // The changes appended to the file where they can be, written
            // whole where they cannot: `quark-edit`'s save. A file that can
            // only be read is saved as PDFium has it.
            let result = match (edits.get_mut(&id), &target) {
                (Some(Ok(edit)), Some(target)) => edit.save(target).map_err(|e| e.to_string()),
                (Some(Ok(_)), None) => Err("say where to save it".to_string()),
                _ => match &path {
                    Some(p) => doc.save_as(p).map_err(|e| e.to_string()),
                    None => doc.save().map_err(|e| e.to_string()),
                },
            };
            match result {
                Ok(()) => {
                    if let Some(target) = &target {
                        doc.set_path(target.clone());
                    }
                    let saved = doc.path().map(Path::to_path_buf).unwrap_or_default();
                    doc.info = told(doc.info.clone(), edits.get(&id));
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

/// The seconds since 1970.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What Quark's own code makes of the document as it is now, as one change:
/// `whole` when what it took out must not stay in the file's earlier bytes.
fn remade(
    edit: &Edit,
    label: &str,
    whole: bool,
    f: impl FnOnce(&[u8]) -> Result<Vec<u8>, crate::engine::EngineError>,
) -> Result<Op, String> {
    let bytes = edit.view_bytes()?;
    let out = f(&bytes).map_err(|e| e.to_string())?;
    Ok(Op::Remade { said: label.to_lowercase(), bytes: std::sync::Arc::new(out), whole })
}

/// Several changes as one, or the one.
fn one(label: &str, mut ops: Vec<Op>) -> Result<Op, String> {
    match ops.len() {
        0 => Err("there was nothing to change".into()),
        1 => Ok(ops.remove(0)),
        _ => Ok(Op::Many { said: label.to_lowercase(), ops }),
    }
}

/// Makes a change, in `quark-edit`'s log, where it is undone and saved — as
/// its own operation where it has one, otherwise as the whole document
/// Quark's own code remade — and has PDFium draw what it made.
fn apply_op(id: DocId, op: DocOp, docs: &mut HashMap<DocId, Document>, edits: &mut Edits, tx: &Sender<Response>) {
    let label = op.label();
    let Some(doc) = docs.get(&id) else { return };
    let edit = match edits.get(&id) {
        Some(Ok(edit)) => edit,
        Some(Err(why)) => return fail(tx, id, &label, format!("it can only be read: {why}")),
        None => return,
    };
    let filename = doc
        .path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let secs = now();
    let date = quark_edit::annot::date(secs);
    let pages_of = |p: &[usize]| p.iter().map(|i| *i as u32 + 1).collect::<Vec<u32>>();
    let mut redaction = None;
    let mut sanitize = None;

    let made: Result<Op, String> = match op {
        DocOp::DeletePages(p) => Ok(Op::Delete { pages: pages_of(&p) }),
        DocOp::RotatePages { pages, by } => Ok(Op::Rotate { pages: pages_of(&pages), by: by.degrees() }),
        DocOp::InsertPagesFromFile { path, at } => std::fs::read(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))
            .map(|bytes| Op::InsertFrom {
                after: at as u32,
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                bytes: std::sync::Arc::new(bytes),
                pages: None,
            }),
        DocOp::SetMetadata(m) => Ok(Op::SetInfo {
            title: m.title.clone(),
            author: m.author.clone(),
            subject: m.subject.clone(),
            keywords: m.keywords.clone(),
            date: date.clone(),
        }),
        DocOp::AddAnnotations(list) => one(
            &label,
            list.iter()
                .filter_map(|a| {
                    crate::edits::mark_of(a).map(|mark| Op::Comment {
                        page: a.page as u32 + 1,
                        mark,
                        author: a.author.clone(),
                        date: date.clone(),
                        name: format!("quark-{secs}-{}", a.id.0),
                    })
                })
                .collect(),
        ),
        DocOp::RemoveAnnotations(ids) => one(
            &label,
            ids.iter().map(|id| Op::DeleteComment { id: crate::edits::comment_id(*id) }).collect(),
        ),
        DocOp::MoveAnnotation { id, by } => {
            Ok(Op::MoveComment { id: crate::edits::comment_id(id), by: [by.x, by.y] })
        }
        DocOp::EditAnnotation { id, color, contents } => Ok(Op::EditComment {
            id: crate::edits::comment_id(id),
            color: color.map(crate::edits::hex),
            text: contents,
            date: date.clone(),
        }),
        DocOp::ReplyTo { id, text, author } => Ok(Op::Reply {
            to: crate::edits::comment_id(id),
            text,
            author,
            date: date.clone(),
            name: format!("quark-reply-{secs}"),
        }),
        DocOp::SetFieldText { name, value } | DocOp::SetFieldValue { name, value } => {
            Ok(Op::SetField { name, value })
        }
        // A box is ticked by its own "on" state, whatever the form calls it.
        DocOp::SetFieldCheck { name, checked } => {
            let on = edit
                .fields()
                .into_iter()
                .find(|f| f.name == name)
                .and_then(|f| f.widgets.into_iter().find_map(|w| w.state))
                .unwrap_or_else(|| "Yes".into());
            Ok(Op::SetField { name, value: if checked { on } else { "Off".into() } })
        }
        DocOp::FlattenForm => Ok(Op::Flatten),
        DocOp::ClearForm => one(
            &label,
            edit.fields()
                .into_iter()
                .filter(|f| !f.read_only && f.kind != "button" && f.kind != "signature" && !f.value.is_empty())
                .map(|f| Op::SetField { name: f.name, value: String::new() })
                .collect(),
        ),
        DocOp::Encrypt { user, owner, permissions } => Ok(Op::Protect {
            protection: quark_edit::security::Protection {
                user,
                owner,
                print: permissions.print,
                copy: permissions.copy,
                edit: permissions.modify,
            },
        }),
        DocOp::Decrypt { .. } => Ok(Op::Unprotect),
        DocOp::Optimize => Ok(Op::Compress),
        DocOp::MovePage { from, to } => {
            remade(edit, &label, false, |b| crate::organize::move_page(b, from, to))
        }
        DocOp::ReorderPages(o) => remade(edit, &label, false, |b| crate::organize::reorder_pages(b, &o)),
        DocOp::ExtractPages(p) => remade(edit, &label, false, |b| crate::organize::extract_pages(b, &p)),
        // Built as a one-page document and spliced in, at the size asked.
        DocOp::InsertBlankPage { at, size } => remade(edit, &label, false, |b| {
            let blank = Document::create_blank(size)
                .and_then(|d| d.to_bytes().map_err(OpenError::Engine))
                .map_err(|e| crate::engine::EngineError::Pdfium(e.to_string()))?;
            crate::organize::insert_pages(b, &blank, at)
        }),
        DocOp::CropPages { pages, box_ } => {
            remade(edit, &label, false, |b| crate::organize::crop_pages(b, &pages, box_))
        }
        DocOp::SetOutline(b) => remade(edit, &label, false, |bytes| crate::outline::write_outline(bytes, &b)),
        DocOp::StampText { pages, stamp } => {
            remade(edit, &label, false, |b| crate::export::stamp_text(b, &pages, &stamp, &filename))
        }
        DocOp::ApplyBates { pages, options } => {
            remade(edit, &label, false, |b| crate::export::apply_bates(b, &pages, &options))
        }
        // What is redacted must not stay in the file's earlier bytes: saved whole.
        DocOp::ApplyRedactions(marks) => match crate::security::apply_redactions(doc, &marks) {
            Ok((out, report)) => {
                redaction = Some(report);
                Ok(Op::Remade { said: label.to_lowercase(), bytes: std::sync::Arc::new(out), whole: true })
            }
            Err(e) => Err(e.to_string()),
        },
        DocOp::Sanitize(opts) => match edit.view_bytes().and_then(|b| {
            crate::security::sanitize(&b, opts).map_err(|e| e.to_string())
        }) {
            Ok((out, report)) => {
                sanitize = Some(report);
                Ok(Op::Remade { said: label.to_lowercase(), bytes: std::sync::Arc::new(out), whole: true })
            }
            Err(e) => Err(e),
        },
    };

    let op = match made {
        Ok(op) => op,
        Err(e) => return fail(tx, id, &label, e),
    };
    let Some(Ok(edit)) = edits.get_mut(&id) else { return };
    if let Err(e) = edit.apply(op) {
        return fail(tx, id, &label, e);
    }
    if !reload(id, label.clone(), docs, edits, tx, redaction, sanitize) {
        // PDFium would not draw what the change made: it is taken back.
        if let Some(Ok(edit)) = edits.get_mut(&id) {
            let _ = edit.undo();
        }
    }
}

/// Has PDFium draw the document as its changes have made it, and says so.
/// Whether it could.
fn reload(
    id: DocId,
    label: String,
    docs: &mut HashMap<DocId, Document>,
    edits: &Edits,
    tx: &Sender<Response>,
    redaction: Option<RedactionReport>,
    sanitize: Option<SanitizeReport>,
) -> bool {
    let Some(Ok(edit)) = edits.get(&id) else { return false };
    let bytes = match edit.view_bytes() {
        Ok(b) => b,
        Err(e) => {
            fail(tx, id, &label, e);
            return false;
        }
    };
    let path = docs.get(&id).and_then(|d| d.path().map(Path::to_path_buf));
    match Document::from_bytes(bytes, None) {
        Ok(mut reloaded) => {
            if let Some(p) = path {
                reloaded.set_path(p);
            }
            reloaded.info = told(reloaded.info.clone(), edits.get(&id));
            let info = Box::new(reloaded.info.clone());
            docs.insert(id, reloaded);
            let _ = tx.send(Response::Applied { id, label, info, redaction, sanitize });
            true
        }
        Err(e) => {
            fail(tx, id, &label, e);
            false
        }
    }
}

/// A book's first page, drawn — or read from where it was kept.
fn cover(path: &Path, max_edge: u32, keep: Option<&Path>) -> Option<Raster> {
    if let Some(kept) = keep.and_then(|k| image::open(k).ok()) {
        let rgba = kept.to_rgba8();
        return Some(Raster { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() });
    }
    let doc = Document::open(path, None).ok()?;
    let raster = crate::render::render_thumbnail(&doc, 0, max_edge, PageTint::None).ok()?;
    if let Some(keep) = keep {
        let png = image::RgbaImage::from_raw(raster.width, raster.height, raster.rgba.clone())?;
        let mut bytes = std::io::Cursor::new(Vec::new());
        if png.write_to(&mut bytes, image::ImageFormat::Png).is_ok() {
            if let Some(dir) = keep.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = crate::library::write_atomically(keep, bytes.get_ref());
        }
    }
    Some(raster)
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

    /// The next `Applied`'s document state.
    fn applied(svc: &PdfService) -> DocInfo {
        wait_for(svc, |r| match r {
            Response::Applied { info, .. } => Some((**info).clone()),
            _ => None,
        })
    }

    fn annotations(svc: &PdfService, id: DocId) -> Vec<Annotation> {
        svc.send(Request::ReadAnnotations { id });
        wait_for(svc, |r| match r {
            Response::Annotations { annotations, .. } => Some(annotations.clone()),
            _ => None,
        })
    }

    #[test]
    fn every_change_is_undone_and_redone_and_saving_appends_to_the_file() {
        let _g = testutil::pdfium_guard();
        use quark_core::annot::{AnnotKind, NoteIcon};
        use quark_core::geom::Rect;

        // A copy of the sample, so saving writes a file of the test's own.
        let path = testutil::tmp_path("svc-undo.pdf");
        std::fs::write(&path, std::fs::read(testutil::sample_pdf()).unwrap()).unwrap();
        let original = std::fs::read(&path).unwrap();
        let svc = PdfService::start();
        let id = svc.new_doc_id();
        svc.send(Request::Open { id, path: path.clone(), password: None });
        let info = wait_for(&svc, |r| match r {
            Response::Opened { info, .. } => Some((**info).clone()),
            _ => None,
        });
        assert!(!info.dirty && info.undo.is_none() && info.read_only.is_none(), "{info:?}");

        let mut note = Annotation::new(AnnotId(1), 0, AnnotKind::Note { icon: NoteIcon::Comment }, Rect::from_xywh(100.0, 600.0, 24.0, 24.0), "Ada");
        note.contents = "Look".into();
        svc.send(Request::Apply { id, op: Box::new(DocOp::AddAnnotations(vec![note])) });
        let info = applied(&svc);
        assert!(info.dirty && info.undo.as_deref() == Some("add a note"), "{info:?}");
        svc.send(Request::Apply { id, op: Box::new(DocOp::RotatePages { pages: vec![1], by: Rot::D90 }) });
        let info = applied(&svc);
        assert_eq!(info.pages[1].rotation, Rot::D90);
        assert_eq!(info.undo.as_deref(), Some("turn page 2"));

        // Undone and redone, in order, and drawn so by PDFium each time.
        svc.send(Request::Undo { id });
        let info = applied(&svc);
        assert_eq!(info.pages[1].rotation, Rot::D0);
        assert_eq!(info.redo.as_deref(), Some("turn page 2"));
        svc.send(Request::Undo { id });
        let info = applied(&svc);
        assert!(!info.dirty && info.undo.is_none());
        assert!(annotations(&svc, id).is_empty());
        svc.send(Request::Redo { id });
        applied(&svc);
        let notes = annotations(&svc, id);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].contents, "Look");

        // Saved by appending: the file's first bytes are kept, and it opens
        // with its note.
        svc.send(Request::Save { id, path: None });
        wait_for(&svc, |r| matches!(r, Response::Saved { .. }).then_some(()));
        let saved = std::fs::read(&path).unwrap();
        assert!(saved.len() > original.len() && saved.starts_with(&original), "the original bytes were rewritten");
        let reopened = svc.new_doc_id();
        svc.send(Request::Open { id: reopened, path: path.clone(), password: None });
        wait_for(&svc, |r| matches!(r, Response::Opened { .. }).then_some(()));
        assert_eq!(annotations(&svc, reopened).len(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_comment_deleted_or_moved_by_its_id_stays_so_and_two_never_share_one() {
        let _g = testutil::pdfium_guard();
        use quark_core::annot::AnnotKind;
        use quark_core::geom::Rect;

        let svc = PdfService::start();
        let id = open_sample(&svc);
        let square = |x: f32| Annotation::new(AnnotId(1), 0, AnnotKind::Square, Rect::from_xywh(x, 500.0, 40.0, 40.0), "Ada");
        svc.send(Request::Apply { id, op: Box::new(DocOp::AddAnnotations(vec![square(100.0), square(200.0)])) });
        let info = applied(&svc);
        assert_eq!(info.undo.as_deref(), Some("add 2 annotation(s)"), "two made at once are one step");
        let two = annotations(&svc, id);
        assert_eq!(two.len(), 2);
        assert_ne!(two[0].id, two[1].id, "both were made with id 1; read back, each has its own");

        svc.send(Request::Apply { id, op: Box::new(DocOp::MoveAnnotation { id: two[1].id, by: Vec2::new(10.0, -20.0) }) });
        applied(&svc);
        let moved = annotations(&svc, id);
        assert!((moved[1].rect.min.x - (two[1].rect.min.x + 10.0)).abs() < 0.01);
        assert!((moved[1].rect.min.y - (two[1].rect.min.y - 20.0)).abs() < 0.01);

        svc.send(Request::Apply { id, op: Box::new(DocOp::RemoveAnnotations(vec![two[0].id])) });
        applied(&svc);
        // Read again after another change: still gone.
        svc.send(Request::Apply { id, op: Box::new(DocOp::RotatePages { pages: vec![0], by: Rot::D90 }) });
        applied(&svc);
        let left = annotations(&svc, id);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, two[1].id);
    }

    fn fields_of(svc: &PdfService, id: DocId) -> Vec<crate::forms::FormField> {
        svc.send(Request::ReadFields { id });
        wait_for(svc, |r| match r {
            Response::Fields { fields, .. } => Some(fields.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_comment_is_recoloured_rewritten_and_answered_and_each_is_undone() {
        let _g = testutil::pdfium_guard();
        use quark_core::annot::{AnnotKind, Color, NoteIcon};
        use quark_core::geom::Rect;

        let svc = PdfService::start();
        let id = open_sample(&svc);
        let mut note = Annotation::new(AnnotId(1), 0, AnnotKind::Note { icon: NoteIcon::Comment }, Rect::from_xywh(100.0, 600.0, 24.0, 24.0), "Ada");
        note.contents = "Look".into();
        svc.send(Request::Apply { id, op: Box::new(DocOp::AddAnnotations(vec![note])) });
        applied(&svc);
        let made = annotations(&svc, id).remove(0);

        let blue = Color { r: 0x1e, g: 0x6f, b: 0xd9, a: 255 };
        svc.send(Request::Apply { id, op: Box::new(DocOp::EditAnnotation { id: made.id, color: Some(blue), contents: Some("Look again".into()) }) });
        applied(&svc);
        svc.send(Request::Apply { id, op: Box::new(DocOp::ReplyTo { id: made.id, text: "Done".into(), author: "Grace".into() }) });
        let info = applied(&svc);
        assert_eq!(info.undo.as_deref(), Some("reply"), "{info:?}");
        let read = annotations(&svc, id);
        assert_eq!(read.len(), 1, "a reply is part of its comment's thread, not a comment of its own: {read:#?}");
        let a = &read[0];
        assert_eq!((a.id, a.contents.as_str()), (made.id, "Look again"));
        assert_eq!((a.color.r, a.color.g, a.color.b), (0x1e, 0x6f, 0xd9));
        assert_eq!(a.replies.len(), 1);
        assert_eq!((a.replies[0].author.as_str(), a.replies[0].contents.as_str()), ("Grace", "Done"));

        svc.send(Request::Undo { id });
        applied(&svc);
        assert!(annotations(&svc, id)[0].replies.is_empty());
        svc.send(Request::Undo { id });
        applied(&svc);
        assert_eq!(annotations(&svc, id)[0].contents, "Look");
    }

    #[test]
    fn a_form_is_filled_ticked_and_flattened_and_flattening_is_undone() {
        let _g = testutil::pdfium_guard();
        let mut lo = lopdf::Document::load_mem(&quark_edit::fixture::fixture(&["A form"])).unwrap();
        quark_edit::forms::sample(&mut lo);
        let path = testutil::tmp_path("svc-form.pdf");
        lo.save(&path).unwrap();
        let svc = PdfService::start();
        let id = svc.new_doc_id();
        svc.send(Request::Open { id, path: path.clone(), password: None });
        wait_for(&svc, |r| matches!(r, Response::Opened { .. }).then_some(()));

        svc.send(Request::Apply { id, op: Box::new(DocOp::SetFieldText { name: "name".into(), value: "Ada".into() }) });
        applied(&svc);
        svc.send(Request::Apply { id, op: Box::new(DocOp::SetFieldValue { name: "agree".into(), value: "Yes".into() }) });
        applied(&svc);
        svc.send(Request::Apply { id, op: Box::new(DocOp::SetFieldValue { name: "colour".into(), value: "Blue".into() }) });
        applied(&svc);
        let read = fields_of(&svc, id);
        let value = |name: &str| read.iter().find(|f| f.name == name).map(|f| f.value.clone());
        assert_eq!(value("name").as_deref(), Some("Ada"));
        assert_eq!(value("agree").as_deref(), Some("Yes"));
        assert_eq!(value("colour").as_deref(), Some("Blue"));

        svc.send(Request::Apply { id, op: Box::new(DocOp::FlattenForm) });
        let info = applied(&svc);
        assert!(fields_of(&svc, id).is_empty(), "a flattened form has no fields left");
        assert!(info.undo.is_some());
        svc.send(Request::Undo { id });
        applied(&svc);
        let back = fields_of(&svc, id);
        assert_eq!(back.iter().find(|f| f.name == "name").map(|f| f.value.as_str()), Some("Ada"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_cover_is_drawn_from_the_first_page_kept_and_read_back_from_where_it_was_kept() {
        let _g = testutil::pdfium_guard();
        let svc = PdfService::start();
        let keep = testutil::tmp_path("covers/one.png");
        let _ = std::fs::remove_file(&keep);
        let cover = |svc: &PdfService, path: PathBuf| {
            svc.send(Request::Cover { path, max_edge: 120, keep: Some(keep.clone()) });
            wait_for(svc, |r| match r {
                Response::Cover { raster, .. } => Some(raster.clone()),
                _ => None,
            })
        };
        let drawn = cover(&svc, testutil::sample_pdf()).expect("the sample is drawn");
        assert_eq!(drawn.width.max(drawn.height), 120, "{}×{}", drawn.width, drawn.height);
        assert!(keep.is_file(), "kept for the next time");
        // Kept: the same picture, without the book being opened at all.
        let again = cover(&svc, testutil::tmp_path("there-is-no-such-book.pdf")).expect("read from where it was kept");
        assert_eq!((again.width, again.height, again.rgba == drawn.rgba), (drawn.width, drawn.height, true));
        let _ = std::fs::remove_file(&keep);
        svc.send(Request::Cover { path: testutil::tmp_path("there-is-no-such-book.pdf"), max_edge: 120, keep: None });
        let none = wait_for(&svc, |r| match r {
            Response::Cover { raster, .. } => Some(raster.is_none()),
            _ => None,
        });
        assert!(none, "a file that is not there has no cover");
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
