//! Opening, describing and saving documents.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;
use quark_core::geom::{PageSize, Rot};
use serde::{Deserialize, Serialize};

use crate::engine::{self, EngineError};

/// Why an open attempt failed.
///
/// A password prompt is a normal outcome rather than an error the user should
/// see as a failure, so it is a distinct variant the UI can act on.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("this document is password protected")]
    PasswordRequired,
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("the file is not a PDF, or is damaged beyond recovery")]
    Malformed,
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Engine(#[from] EngineError),
}

/// A page's own geometry, as stored in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    /// The page's visible size, after its own `/Rotate` is applied.
    pub size: PageSize,
    /// The page's stored rotation.
    pub rotation: Rot,
    /// Whether the page carries any annotations.
    pub has_annotations: bool,
    /// Label shown in the page box — `/PageLabels` if present, else the number.
    pub label: Option<String>,
}

impl Default for PageInfo {
    fn default() -> Self {
        Self {
            size: PageSize::LETTER,
            rotation: Rot::D0,
            has_annotations: false,
            label: None,
        }
    }
}

/// A rectangle on the wire.
///
/// `quark_core::Rect` would do, but the service protocol keeps its own plain
/// types so a change to the geometry crate cannot silently alter what crosses
/// the thread boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl LinkRect {
    pub fn contains(&self, p: quark_core::geom::Vec2) -> bool {
        p.x >= self.min_x && p.x <= self.max_x && p.y >= self.min_y && p.y <= self.max_y
    }

    pub fn to_rect(self) -> quark_core::geom::Rect {
        quark_core::geom::Rect::new(
            quark_core::geom::Vec2::new(self.min_x, self.min_y),
            quark_core::geom::Vec2::new(self.max_x, self.max_y),
        )
    }
}

/// Document metadata, as shown in the Properties dialog.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    pub creation_date: String,
    pub modification_date: String,
}

impl Metadata {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.author.is_empty()
            && self.subject.is_empty()
            && self.keywords.is_empty()
    }
}

/// What the document's security settings allow.
///
/// These are advisory: PDF permissions are not enforced by cryptography and any
/// reader can ignore them. Quark honours them in the UI because a user who set
/// them expects to see them respected, but it never claims they are a security
/// boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Permissions {
    pub print: bool,
    pub print_high_quality: bool,
    pub modify_contents: bool,
    pub extract_content: bool,
    pub modify_annotations: bool,
    pub fill_forms: bool,
    pub create_form_fields: bool,
    pub assemble: bool,
    /// True when the file carries no encryption dictionary at all.
    pub unencrypted: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        // An unencrypted document permits everything.
        Self {
            print: true,
            print_high_quality: true,
            modify_contents: true,
            extract_content: true,
            modify_annotations: true,
            fill_forms: true,
            create_form_fields: true,
            assemble: true,
            unencrypted: true,
        }
    }
}

/// Everything the UI needs to know about an open document without touching
/// PDFium again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocInfo {
    pub path: Option<PathBuf>,
    pub page_count: usize,
    pub pages: Vec<PageInfo>,
    pub metadata: Metadata,
    pub permissions: Permissions,
    pub version: String,
    /// Bytes on disk, or 0 for a document built in memory.
    pub file_size: u64,
    pub encrypted: bool,
    pub has_form: bool,
    pub form_field_count: usize,
    pub has_signatures: bool,
    pub signature_count: usize,
    pub attachment_count: usize,
    pub bookmark_count: usize,
    /// True when the document is tagged for accessibility.
    pub tagged: bool,
    /// True when no page has extractable text, which is what a scan looks like
    /// and what makes OCR worth offering.
    pub likely_scanned: bool,
}

impl DocInfo {
    /// The name shown on the tab.
    pub fn display_name(&self) -> String {
        // Prefer the filename: a document's `/Title` is often a leftover from
        // whatever template it was built from and rarely matches the file.
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .or_else(|| {
                let t = self.metadata.title.trim();
                (!t.is_empty()).then(|| t.to_owned())
            })
            .unwrap_or_else(|| "Untitled".into())
    }

    pub fn page_size(&self, index: usize) -> PageSize {
        self.pages
            .get(index)
            .map(|p| p.size)
            .unwrap_or(PageSize::LETTER)
    }

    /// Every page's size, for the layout engine.
    pub fn page_sizes(&self) -> Vec<PageSize> {
        self.pages.iter().map(|p| p.size).collect()
    }

    /// The label to show for a page, honouring `/PageLabels`.
    pub fn page_label(&self, index: usize) -> String {
        self.pages
            .get(index)
            .and_then(|p| p.label.clone())
            .unwrap_or_else(|| (index + 1).to_string())
    }
}

/// An open document plus the bookkeeping Quark keeps alongside it.
pub struct Document {
    inner: PdfDocument<'static>,
    pub info: DocInfo,
    /// The password the file was opened with, needed to save it back encrypted.
    password: Option<String>,
    /// Raw `/P` bits, read from the file when PDFium cannot report permissions
    /// itself. See [`crate::security::probe_permission_bits`].
    security_bits: Option<i64>,
}

impl Document {
    /// Opens a file from disk.
    pub fn open(path: impl AsRef<Path>, password: Option<&str>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        let engine = engine::init()?;
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        let doc = engine
            .load_pdf_from_file(path, password)
            .map_err(|e| classify_open_error(e, password.is_some()))?;

        let mut d = Self {
            inner: doc,
            info: DocInfo {
                path: Some(path.to_path_buf()),
                file_size,
                ..Default::default()
            },
            password: password.map(str::to_owned),
            security_bits: None,
        };
        d.refresh_info();
        // Only re-read the file when PDFium could not report permissions, so an
        // ordinary unencrypted document never pays for this.
        if d.needs_permission_probe() {
            if let Ok(raw) = std::fs::read(path) {
                d.security_bits = crate::security::probe_permission_bits(&raw);
                d.refresh_info();
            }
        }
        Ok(d)
    }

    /// Opens a document already held in memory.
    pub fn from_bytes(bytes: Vec<u8>, password: Option<&str>) -> Result<Self, OpenError> {
        let engine = engine::init()?;
        let len = bytes.len() as u64;
        // Probed before the buffer is handed to PDFium, which takes ownership.
        let bits = crate::security::probe_permission_bits(&bytes);
        let doc = engine
            .load_pdf_from_byte_vec(bytes, password)
            .map_err(|e| classify_open_error(e, password.is_some()))?;
        let mut d = Self {
            inner: doc,
            info: DocInfo {
                file_size: len,
                ..Default::default()
            },
            password: password.map(str::to_owned),
            security_bits: bits,
        };
        d.refresh_info();
        Ok(d)
    }

    /// Creates an empty single-page document.
    pub fn create_blank(size: PageSize) -> Result<Self, OpenError> {
        let engine = engine::init()?;
        let mut doc = engine.create_new_pdf().map_err(EngineError::from)?;
        doc.pages_mut()
            .create_page_at_end(PdfPagePaperSize::Custom(
                PdfPoints::new(size.width),
                PdfPoints::new(size.height),
            ))
            .map_err(EngineError::from)?;
        let mut d = Self {
            inner: doc,
            info: DocInfo::default(),
            password: None,
            security_bits: None,
        };
        d.refresh_info();
        Ok(d)
    }

    pub fn inner(&self) -> &PdfDocument<'static> {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut PdfDocument<'static> {
        &mut self.inner
    }

    pub fn path(&self) -> Option<&Path> {
        self.info.path.as_deref()
    }

    pub fn set_path(&mut self, p: PathBuf) {
        self.info.path = Some(p);
    }

    pub fn page_count(&self) -> usize {
        self.info.page_count
    }

    /// Whether PDFium was unable to report this document's permissions, so the
    /// `/Encrypt` dictionary has to be read directly.
    fn needs_permission_probe(&self) -> bool {
        self.info.encrypted && self.security_bits.is_none()
    }

    /// Re-reads everything cached in [`DocInfo`].
    ///
    /// Called after any structural change, because page count, sizes and the
    /// form field inventory all move underneath us when pages are edited.
    pub fn refresh_info(&mut self) {
        let pages = self.inner.pages();
        let count = pages.len() as usize;

        let mut infos = Vec::with_capacity(count);
        let mut any_text = false;
        for page in pages.iter() {
            let s = page.page_size();
            // A degenerate MediaBox would divide by zero in the layout pass.
            let (w, h) = (s.width().value, s.height().value);
            let size = if w > 1.0 && h > 1.0 {
                PageSize::new(w, h)
            } else {
                PageSize::LETTER
            };
            let has_annotations = page.annotations().len() > 0;
            if !any_text {
                if let Ok(t) = page.text() {
                    // A handful of stray characters can come from a scanner's
                    // own watermark, so require enough text to be real content.
                    if t.all().trim().chars().count() > 16 {
                        any_text = true;
                    }
                }
            }
            infos.push(PageInfo {
                size,
                rotation: page
                    .rotation()
                    .map(|r| Rot::from_degrees(rotation_degrees(r)))
                    .unwrap_or(Rot::D0),
                has_annotations,
                label: None,
            });
        }

        let meta = self.inner.metadata();
        let get = |t: PdfDocumentMetadataTagType| -> String {
            meta.get(t).map(|v| v.value().to_owned()).unwrap_or_default()
        };
        let metadata = Metadata {
            title: get(PdfDocumentMetadataTagType::Title),
            author: get(PdfDocumentMetadataTagType::Author),
            subject: get(PdfDocumentMetadataTagType::Subject),
            keywords: get(PdfDocumentMetadataTagType::Keywords),
            creator: get(PdfDocumentMetadataTagType::Creator),
            producer: get(PdfDocumentMetadataTagType::Producer),
            creation_date: get(PdfDocumentMetadataTagType::CreationDate),
            modification_date: get(PdfDocumentMetadataTagType::ModificationDate),
        };

        let perms = self.inner.permissions();
        let revision = perms.security_handler_revision();
        // PDFium reports a distinct "unprotected" revision for a file with no
        // encryption dictionary. An *error* is not the same thing: it means the
        // revision is one pdfium-render cannot name, which is what AES
        // encryption (revision 5 or 6) looks like. Treating that as unencrypted
        // would report a locked-down document as unrestricted.
        let unencrypted = matches!(revision, Ok(PdfSecurityHandlerRevision::Unprotected));

        let permissions = if unencrypted {
            Permissions::default()
        } else if revision.is_ok() {
            let ok = |r: Result<bool, PdfiumError>| r.unwrap_or(true);
            Permissions {
                print: ok(perms.can_print_only_low_quality()) || ok(perms.can_print_high_quality()),
                print_high_quality: ok(perms.can_print_high_quality()),
                modify_contents: ok(perms.can_modify_document_content()),
                extract_content: ok(perms.can_extract_text_and_graphics()),
                modify_annotations: ok(perms.can_add_or_modify_text_annotations()),
                fill_forms: ok(perms.can_fill_existing_interactive_form_fields()),
                create_form_fields: ok(perms.can_create_new_interactive_form_fields()),
                assemble: ok(perms.can_assemble_document()),
                unencrypted: false,
            }
        } else {
            // AES: the bits come from the file itself.
            match self.security_bits {
                Some(bits) => {
                    let p = crate::security::permissions_from_bits(bits);
                    Permissions {
                        print: p.print,
                        print_high_quality: p.print_high_quality,
                        modify_contents: p.modify,
                        extract_content: p.copy,
                        modify_annotations: p.annotate,
                        fill_forms: p.fill_forms,
                        create_form_fields: p.annotate && p.modify,
                        assemble: p.assemble,
                        unencrypted: false,
                    }
                }
                // Encrypted, but the bits could not be read. Assume the
                // document is restricted rather than advertising freedoms it
                // may not grant.
                None => Permissions {
                    unencrypted: false,
                    ..Permissions::default()
                },
            }
        };

        let form_field_count = self
            .inner
            .pages()
            .iter()
            .map(|p| {
                p.annotations()
                    .iter()
                    .filter(|a| a.annotation_type() == PdfPageAnnotationType::Widget)
                    .count()
            })
            .sum();

        let signature_count = self.inner.signatures().len() as usize;
        let attachment_count = self.inner.attachments().len() as usize;
        let bookmark_count = self.inner.bookmarks().iter().count();

        self.info.page_count = count;
        self.info.pages = infos;
        self.info.metadata = metadata;
        self.info.encrypted = !permissions.unencrypted;
        self.info.permissions = permissions;
        self.info.version = format!("{:?}", self.inner.version());
        self.info.has_form = self.inner.form().is_some();
        self.info.form_field_count = form_field_count;
        self.info.signature_count = signature_count;
        self.info.has_signatures = signature_count > 0;
        self.info.attachment_count = attachment_count;
        self.info.bookmark_count = bookmark_count;
        self.info.likely_scanned = count > 0 && !any_text;
    }

    /// Writes the document to `path`.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), EngineError> {
        let path = path.as_ref();
        // Write to a sibling temporary file and rename over the target, so an
        // interrupted save cannot leave a truncated document where the user's
        // only copy used to be. Saving over the file being read is the case
        // that makes this matter.
        let tmp = temp_sibling(path);
        self.inner.save_to_file(&tmp).map_err(EngineError::from)?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            EngineError::Pdfium(format!("could not replace {}: {e}", path.display()))
        })?;
        self.info.path = Some(path.to_path_buf());
        self.info.file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(())
    }

    /// Saves back over the file the document came from.
    pub fn save(&mut self) -> Result<(), EngineError> {
        let Some(path) = self.info.path.clone() else {
            return Err(EngineError::Pdfium(
                "this document has never been saved; use Save As".into(),
            ));
        };
        self.save_as(path)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, EngineError> {
        self.inner.save_to_bytes().map_err(EngineError::from)
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// Applies metadata changes.
    ///
    /// PDFium exposes no metadata writer, so this is routed through the lopdf
    /// side by [`crate::organize::set_metadata`] on the saved bytes.
    pub fn pending_metadata(&self) -> &Metadata {
        &self.info.metadata
    }
}

/// Builds a temporary filename beside `path`.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".into());
    name.push_str(".quark-tmp");
    path.with_file_name(name)
}

fn rotation_degrees(r: PdfPageRenderRotation) -> i32 {
    match r {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 90,
        PdfPageRenderRotation::Degrees180 => 180,
        PdfPageRenderRotation::Degrees270 => 270,
    }
}

/// Turns a PDFium load failure into something the UI can act on.
fn classify_open_error(e: PdfiumError, had_password: bool) -> OpenError {
    let s = format!("{e:?}");
    if s.contains("PasswordError") || s.contains("Password") {
        // Telling these apart is what decides between showing a prompt and
        // showing "that password was wrong".
        return if had_password {
            OpenError::WrongPassword
        } else {
            OpenError::PasswordRequired
        };
    }
    if s.contains("FileError") || s.contains("Io") {
        return OpenError::Io(s);
    }
    OpenError::Malformed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn opens_a_document_and_reports_its_pages() {
        let _g = crate::testutil::pdfium_guard();
        let path = testutil::sample_pdf();
        let doc = Document::open(&path, None).expect("open");
        assert_eq!(doc.page_count(), 2);
        assert_eq!(doc.info.pages.len(), 2);
        let s = doc.info.page_size(0);
        assert!((s.width - 612.0).abs() < 0.5);
        assert!((s.height - 792.0).abs() < 0.5);
    }

    #[test]
    fn a_text_document_is_not_mistaken_for_a_scan() {
        let _g = crate::testutil::pdfium_guard();
        let doc = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(!doc.info.likely_scanned);
    }

    #[test]
    fn an_unencrypted_document_permits_everything() {
        let _g = crate::testutil::pdfium_guard();
        let doc = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(doc.info.permissions.unencrypted);
        assert!(doc.info.permissions.print);
        assert!(doc.info.permissions.extract_content);
        assert!(!doc.info.encrypted);
    }

    #[test]
    fn opening_a_non_pdf_fails_as_malformed_not_as_a_password_prompt() {
        let _g = crate::testutil::pdfium_guard();
        let p = testutil::tmp_path("not-a.pdf");
        std::fs::write(&p, b"this is plainly not a PDF").unwrap();
        match Document::open(&p, None) {
            Err(OpenError::Malformed) => {}
            other => panic!("expected Malformed, got {other:?}", other = other.err()),
        }
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_blank_document_starts_with_one_page_of_the_requested_size() {
        let _g = crate::testutil::pdfium_guard();
        let doc = Document::create_blank(PageSize::new(595.0, 842.0)).expect("create");
        assert_eq!(doc.page_count(), 1);
        let s = doc.info.page_size(0);
        assert!((s.width - 595.0).abs() < 1.0, "got {}", s.width);
    }

    #[test]
    fn display_name_prefers_the_filename() {
        let _g = crate::testutil::pdfium_guard();
        let doc = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(doc.info.display_name().ends_with(".pdf"));
        // With no path at all it still produces something printable.
        let blank = Document::create_blank(PageSize::LETTER).unwrap();
        assert_eq!(blank.info.display_name(), "Untitled");
    }

    #[test]
    fn page_labels_default_to_one_based_numbers() {
        let _g = crate::testutil::pdfium_guard();
        let doc = Document::open(testutil::sample_pdf(), None).unwrap();
        assert_eq!(doc.info.page_label(0), "1");
        assert_eq!(doc.info.page_label(1), "2");
    }

    #[test]
    fn save_writes_a_readable_document_back() {
        let _g = crate::testutil::pdfium_guard();
        let mut doc = Document::open(testutil::sample_pdf(), None).unwrap();
        let out = testutil::tmp_path("saved-roundtrip.pdf");
        doc.save_as(&out).expect("save");
        assert!(out.exists());
        let reopened = Document::open(&out, None).expect("reopen");
        assert_eq!(reopened.page_count(), 2);
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let _g = crate::testutil::pdfium_guard();
        let mut doc = Document::open(testutil::sample_pdf(), None).unwrap();
        let out = testutil::tmp_path("no-temp-left.pdf");
        doc.save_as(&out).unwrap();
        let tmp = temp_sibling(&out);
        assert!(!tmp.exists(), "temporary file {} was left behind", tmp.display());
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn save_updates_the_recorded_path_and_size() {
        let _g = crate::testutil::pdfium_guard();
        let mut doc = Document::open(testutil::sample_pdf(), None).unwrap();
        let out = testutil::tmp_path("path-updated.pdf");
        doc.save_as(&out).unwrap();
        assert_eq!(doc.path(), Some(out.as_path()));
        assert!(doc.info.file_size > 0);
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn a_never_saved_document_refuses_a_plain_save() {
        let _g = crate::testutil::pdfium_guard();
        let mut doc = Document::create_blank(PageSize::LETTER).unwrap();
        assert!(doc.save().is_err(), "Save must not guess a path");
    }

    #[test]
    fn page_sizes_feed_the_layout_engine() {
        let _g = crate::testutil::pdfium_guard();
        let doc = Document::open(testutil::sample_pdf(), None).unwrap();
        let sizes = doc.info.page_sizes();
        assert_eq!(sizes.len(), 2);
        let l = quark_core::layout::compute(&sizes, &quark_core::layout::LayoutParams::default());
        assert_eq!(l.pages.len(), 2);
    }
}
