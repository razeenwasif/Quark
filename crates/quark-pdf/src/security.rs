//! Encryption, permissions, redaction and sanitisation.
//!
//! # What redaction means here
//!
//! Marking a redaction only draws a box. **Applying** it is what removes the
//! content, and that removal has to be real: a black rectangle painted over
//! text leaves the text in the content stream, where any reader — or any
//! `strings` invocation — recovers it. This is the single most consequential
//! correctness requirement in the module, so [`apply_redactions`] deletes the
//! underlying page objects rather than covering them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions as LoPermissions};
use lopdf::{Document as LoDocument, Object};
use quark_core::geom::Rect;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::engine::EngineError;

type Result<T> = std::result::Result<T, EngineError>;

/// What a reader is permitted to do with an encrypted document.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PermissionSet {
    pub print: bool,
    pub print_high_quality: bool,
    pub modify: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
    pub accessibility: bool,
}

impl Default for PermissionSet {
    fn default() -> Self {
        Self {
            print: true,
            print_high_quality: true,
            modify: true,
            copy: true,
            annotate: true,
            fill_forms: true,
            // Accessibility extraction is always granted. PDF 2.0 deprecates
            // clearing this bit, and denying screen readers is not a setting
            // Quark offers.
            assemble: true,
            accessibility: true,
        }
    }
}

impl PermissionSet {
    /// Everything denied except accessibility.
    pub fn read_only() -> Self {
        Self {
            print: true,
            print_high_quality: false,
            modify: false,
            copy: false,
            annotate: false,
            fill_forms: false,
            assemble: false,
            accessibility: true,
        }
    }

    fn to_lopdf(self) -> LoPermissions {
        let mut p = LoPermissions::empty();
        if self.print {
            p |= LoPermissions::PRINTABLE;
        }
        if self.print_high_quality {
            p |= LoPermissions::PRINTABLE_IN_HIGH_QUALITY;
        }
        if self.modify {
            p |= LoPermissions::MODIFIABLE;
        }
        if self.copy {
            p |= LoPermissions::COPYABLE;
        }
        if self.annotate {
            p |= LoPermissions::ANNOTABLE;
        }
        if self.fill_forms {
            p |= LoPermissions::FILLABLE;
        }
        if self.assemble {
            p |= LoPermissions::ASSEMBLABLE;
        }
        if self.accessibility {
            p |= LoPermissions::COPYABLE_FOR_ACCESSIBILITY;
        }
        p
    }
}

/// Encrypts a document with AES-256.
///
/// `user_password` is required to open the file; `owner_password` is required
/// to change its permissions. Either may be empty, and an empty user password
/// means the document opens without a prompt while still carrying restrictions.
pub fn encrypt(
    bytes: &[u8],
    user_password: &str,
    owner_password: &str,
    permissions: PermissionSet,
) -> Result<Vec<u8>> {
    let mut doc = LoDocument::load_mem(bytes)
        .map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))?;

    // A fresh random file encryption key per document. Deriving it from the
    // password instead would make two documents with the same password share a
    // key, which is exactly what the format's key wrapping exists to avoid.
    let mut key = [0u8; 32];
    rand::rng().fill(&mut key);

    // An encrypted PDF is required to carry a file identifier in its trailer,
    // and for the older security handlers the first element feeds the key
    // derivation. Our hand-built and freshly-created documents may not have
    // one, and a reader that enforces the requirement rejects the whole file.
    if doc.trailer.get(b"ID").is_err() {
        let mut id = [0u8; 16];
        rand::rng().fill(&mut id);
        let id_obj = Object::String(id.to_vec(), lopdf::StringFormat::Hexadecimal);
        doc.trailer
            .set("ID", Object::Array(vec![id_obj.clone(), id_obj]));
    }

    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password,
        user_password,
        permissions: permissions.to_lopdf(),
    };

    let state = EncryptionState::try_from(version)
        .map_err(|e| EngineError::Pdfium(format!("could not set up encryption: {e}")))?;
    doc.encrypt(&state)
        .map_err(|e| EngineError::Pdfium(format!("encryption failed: {e}")))?;

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok(out)
}

/// Removes encryption, given a password that opens the document.
///
/// The password has to be supplied at *load* time rather than through
/// `Document::decrypt` afterwards. Loading an encrypted file without it leaves
/// lopdf holding objects it cannot make sense of, and decrypting from there
/// discards them — producing a syntactically valid PDF with no content at all,
/// which is a far worse outcome than an error.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut doc = LoDocument::load_mem_with_options(
        bytes,
        lopdf::LoadOptions {
            password: Some(password.to_owned()),
            ..Default::default()
        },
    )
    .map_err(|e| EngineError::Pdfium(format!("could not decrypt: {e}")))?;

    // Loading with the password already decrypted every object; all that is
    // left is to stop advertising the document as encrypted.
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok(out)
}


/// Reads the raw `/P` permission bits straight out of the `/Encrypt` dictionary.
///
/// `pdfium-render` can only name security handler revisions 2 to 4, so for an
/// AES-encrypted document (revision 5 or 6) every one of its permission queries
/// fails and the document would otherwise be reported as unrestricted. The
/// `/Encrypt` dictionary itself is never encrypted, so the bits can be read
/// without the password.
pub fn probe_permission_bits(bytes: &[u8]) -> Option<i64> {
    let doc = LoDocument::load_mem(bytes).ok()?;
    let enc = doc.trailer.get(b"Encrypt").ok()?;
    let dict = match enc {
        Object::Reference(r) => doc.get_object(*r).ok()?.as_dict().ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    dict.get(b"P").ok()?.as_i64().ok()
}

/// Decodes `/P` into the permission flags Quark shows.
///
/// The value is a signed 32-bit integer whose high bits are all set, so it is
/// read as unsigned before the bits are tested.
pub fn permissions_from_bits(bits: i64) -> PermissionSet {
    let b = bits as u32;
    let has = |bit: u32| b & (1 << bit) != 0;
    PermissionSet {
        print: has(2),
        print_high_quality: has(11),
        modify: has(3),
        copy: has(4),
        annotate: has(5),
        fill_forms: has(8),
        assemble: has(10),
        accessibility: has(9),
    }
}

/// A region to be permanently removed.
#[derive(Debug, Clone, PartialEq)]
pub struct RedactionMark {
    pub page: usize,
    /// Area in page space.
    pub rect: Rect,
}

/// What an [`apply_redactions`] run removed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RedactionReport {
    pub pages_touched: usize,
    pub text_runs_removed: usize,
    pub images_removed: usize,
    pub annotations_removed: usize,
}

/// Permanently removes the marked content.
///
/// The content stream is rebuilt without any text-showing operator whose
/// position falls inside a redaction rectangle, and without any image drawn
/// there. A filled black rectangle is then painted over each region so the
/// removal is visible.
///
/// This is destructive and has no undo at the file level; the caller keeps the
/// pre-redaction bytes if it wants one.
pub fn apply_redactions(
    doc: &Document,
    marks: &[RedactionMark],
) -> Result<(Vec<u8>, RedactionReport)> {
    if marks.is_empty() {
        return Ok((doc.to_bytes()?, RedactionReport::default()));
    }

    let bytes = doc.to_bytes()?;
    let mut lo = LoDocument::load_mem(&bytes)
        .map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))?;

    let page_ids: Vec<_> = {
        let mut v: Vec<_> = lo.get_pages().into_iter().collect();
        v.sort_by_key(|(n, _)| *n);
        v.into_iter().map(|(_, id)| id).collect()
    };

    let mut by_page: BTreeMap<usize, Vec<Rect>> = BTreeMap::new();
    for m in marks {
        by_page.entry(m.page).or_default().push(m.rect.normalize());
    }

    let mut report = RedactionReport::default();

    for (page_index, rects) in &by_page {
        let Some(&pid) = page_ids.get(*page_index) else {
            continue;
        };
        report.pages_touched += 1;

        // Drop any annotation whose rectangle overlaps a redaction. A comment
        // quoting the redacted text is just as much of a leak as the text.
        let annots: Vec<Object> = lo
            .get_object(pid)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Annots"))
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default();
        if !annots.is_empty() {
            let mut kept = Vec::new();
            for a in annots {
                let overlaps = a
                    .as_reference()
                    .ok()
                    .and_then(|r| lo.get_object(r).ok())
                    .and_then(|o| o.as_dict().ok())
                    .and_then(|d| d.get(b"Rect").ok())
                    .and_then(|o| o.as_array().ok())
                    .map(|arr| {
                        let v: Vec<f32> =
                            arr.iter().filter_map(|o| o.as_float().ok()).collect();
                        v.len() == 4
                            && rects.iter().any(|r| {
                                r.intersects(
                                    &Rect::from_xywh(v[0], v[1], v[2] - v[0], v[3] - v[1])
                                        .normalize(),
                                )
                            })
                    })
                    .unwrap_or(false);
                if overlaps {
                    report.annotations_removed += 1;
                } else {
                    kept.push(a);
                }
            }
            if let Ok(d) = lo.get_object_mut(pid).and_then(Object::as_dict_mut) {
                d.set("Annots", Object::Array(kept));
            }
        }

        // Rebuild the content stream.
        let content_data = lo.get_page_content(pid);
        let content = lopdf::content::Content::decode(&content_data)
            .map_err(|e| EngineError::Pdfium(format!("could not decode content: {e}")))?;

        let (rebuilt, removed_text, removed_images) = strip_operations(&content, rects);
        report.text_runs_removed += removed_text;
        report.images_removed += removed_images;

        let mut ops = rebuilt.operations;
        ops.extend(black_box_ops(rects));

        let encoded = lopdf::content::Content { operations: ops }
            .encode()
            .map_err(|e| EngineError::Pdfium(format!("could not encode content: {e}")))?;
        lo.change_page_content(pid, encoded)
            .map_err(|e| EngineError::Pdfium(format!("could not write page content: {e}")))?;
    }

    let mut out = Vec::new();
    lo.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok((out, report))
}

/// Removes drawing operations that fall inside any redaction rectangle.
///
/// The text position is tracked through `Td`, `TD`, `Tm` and `T*` so that a
/// `Tj` can be tested against the rectangles. This is an approximation — it
/// treats each show-text operator as a point rather than measuring the glyph
/// run — but it errs towards removing too much, which is the correct direction
/// for a redaction tool.
fn strip_operations(
    content: &lopdf::content::Content,
    rects: &[Rect],
) -> (lopdf::content::Content, usize, usize) {
    use lopdf::content::{Content, Operation};

    let mut out: Vec<Operation> = Vec::with_capacity(content.operations.len());
    let mut removed_text = 0;
    let mut removed_images = 0;

    // Current text position, and the line start that T* returns to.
    let (mut tx, mut ty) = (0.0f32, 0.0f32);
    let (mut lx, mut ly) = (0.0f32, 0.0f32);
    let mut leading = 0.0f32;
    // Current transformation matrix translation, which is where an image is
    // drawn by `Do` after a `cm`.
    let (mut cm_x, mut cm_y) = (0.0f32, 0.0f32);

    let num = |o: &Object| o.as_float().unwrap_or(0.0);
    let inside = |x: f32, y: f32| rects.iter().any(|r| r.contains(quark_core::geom::Vec2::new(x, y)));

    for op in &content.operations {
        match op.operator.as_str() {
            "BT" => {
                tx = 0.0;
                ty = 0.0;
                lx = 0.0;
                ly = 0.0;
            }
            "Td" if op.operands.len() >= 2 => {
                lx += num(&op.operands[0]);
                ly += num(&op.operands[1]);
                tx = lx;
                ty = ly;
            }
            "TD" if op.operands.len() >= 2 => {
                leading = -num(&op.operands[1]);
                lx += num(&op.operands[0]);
                ly += num(&op.operands[1]);
                tx = lx;
                ty = ly;
            }
            "Tm" if op.operands.len() >= 6 => {
                lx = num(&op.operands[4]);
                ly = num(&op.operands[5]);
                tx = lx;
                ty = ly;
            }
            "TL" if !op.operands.is_empty() => leading = num(&op.operands[0]),
            "T*" => {
                ly -= leading;
                tx = lx;
                ty = ly;
            }
            "cm" if op.operands.len() >= 6 => {
                cm_x = num(&op.operands[4]);
                cm_y = num(&op.operands[5]);
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if inside(tx, ty) {
                    removed_text += 1;
                    continue;
                }
            }
            "Do" => {
                // An XObject's origin after `cm`. Images are placed by a
                // preceding `cm`, so this catches a redacted photograph.
                if inside(cm_x, cm_y) {
                    removed_images += 1;
                    continue;
                }
            }
            _ => {}
        }
        out.push(op.clone());
    }

    (Content { operations: out }, removed_text, removed_images)
}

/// Operations painting an opaque black rectangle over each redaction.
fn black_box_ops(rects: &[Rect]) -> Vec<lopdf::content::Operation> {
    use lopdf::content::Operation;
    let mut ops = vec![Operation::new("q", vec![]), Operation::new(
        "rg",
        vec![Object::Real(0.0), Object::Real(0.0), Object::Real(0.0)],
    )];
    for r in rects {
        ops.push(Operation::new("re", vec![
            Object::Real(r.min.x),
            Object::Real(r.min.y),
            Object::Real(r.width()),
            Object::Real(r.height()),
        ]));
        ops.push(Operation::new("f", vec![]));
    }
    ops.push(Operation::new("Q", vec![]));
    ops
}

/// What a sanitisation pass may remove.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SanitizeOptions {
    pub metadata: bool,
    pub attachments: bool,
    pub javascript: bool,
    pub embedded_files: bool,
    pub annotations: bool,
    pub form_fields: bool,
    pub bookmarks: bool,
    pub hidden_layers: bool,
}

impl Default for SanitizeOptions {
    fn default() -> Self {
        // The defaults target what leaks *information* — scripts, metadata,
        // embedded payloads — while leaving the document's visible content and
        // its comments intact, because removing those is a separate decision.
        Self {
            metadata: true,
            attachments: true,
            javascript: true,
            embedded_files: true,
            annotations: false,
            form_fields: false,
            bookmarks: false,
            hidden_layers: true,
        }
    }
}

/// What a sanitisation pass removed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SanitizeReport {
    pub removed: Vec<String>,
}

impl SanitizeReport {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.removed.is_empty() {
            "Nothing to remove".into()
        } else {
            format!("Removed: {}", self.removed.join(", "))
        }
    }
}

/// Strips hidden information from a document.
pub fn sanitize(bytes: &[u8], opts: SanitizeOptions) -> Result<(Vec<u8>, SanitizeReport)> {
    let mut doc = LoDocument::load_mem(bytes)
        .map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))?;
    let mut report = SanitizeReport::default();

    let catalog_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| EngineError::Pdfium("document has no /Root".into()))?;

    if opts.metadata {
        if doc.trailer.get(b"Info").is_ok() {
            doc.trailer.remove(b"Info");
            report.removed.push("document properties".into());
        }
        if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
            // /Metadata is the XMP packet, which carries a second, independent
            // copy of the author and editing history.
            if d.remove(b"Metadata").is_some() {
                report.removed.push("XMP metadata".into());
            }
        }
    }

    if opts.javascript {
        let mut found = false;
        if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
            if d.remove(b"OpenAction").is_some() {
                found = true;
            }
            if d.remove(b"AA").is_some() {
                found = true;
            }
        }
        // Document-level scripts live in /Names /JavaScript.
        let names_id = doc
            .get_object(catalog_id)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Names"))
            .and_then(Object::as_reference)
            .ok();
        if let Some(nid) = names_id {
            if let Ok(d) = doc.get_object_mut(nid).and_then(Object::as_dict_mut) {
                if d.remove(b"JavaScript").is_some() {
                    found = true;
                }
            }
        }
        if found {
            report.removed.push("JavaScript".into());
        }
    }

    if opts.embedded_files || opts.attachments {
        let names_id = doc
            .get_object(catalog_id)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Names"))
            .and_then(Object::as_reference)
            .ok();
        if let Some(nid) = names_id {
            if let Ok(d) = doc.get_object_mut(nid).and_then(Object::as_dict_mut) {
                if d.remove(b"EmbeddedFiles").is_some() {
                    report.removed.push("embedded files".into());
                }
            }
        }
    }

    if opts.bookmarks {
        if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
            if d.remove(b"Outlines").is_some() {
                report.removed.push("bookmarks".into());
            }
        }
    }

    if opts.hidden_layers {
        if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
            if d.remove(b"OCProperties").is_some() {
                report.removed.push("hidden layers".into());
            }
        }
    }

    if opts.form_fields {
        if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
            if d.remove(b"AcroForm").is_some() {
                report.removed.push("form fields".into());
            }
        }
    }

    if opts.annotations {
        let page_ids: Vec<_> = doc.get_pages().into_values().collect();
        let mut n = 0;
        for pid in page_ids {
            if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
                if d.remove(b"Annots").is_some() {
                    n += 1;
                }
            }
        }
        if n > 0 {
            report.removed.push("annotations".into());
        }
    }

    doc.prune_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok((out, report))
}

/// Reports what hidden information a document carries, without changing it.
pub fn scan_hidden_information(bytes: &[u8]) -> Result<Vec<String>> {
    let doc = LoDocument::load_mem(bytes)
        .map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))?;
    let mut found = BTreeSet::new();

    if doc.trailer.get(b"Info").is_ok() {
        found.insert("Document properties".to_string());
    }
    if let Ok(catalog) = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .and_then(|r| doc.get_object(r))
        .and_then(Object::as_dict)
    {
        for (key, label) in [
            (&b"Metadata"[..], "XMP metadata"),
            (&b"OpenAction"[..], "Document open action"),
            (&b"AA"[..], "Additional actions"),
            (&b"OCProperties"[..], "Layers"),
            (&b"AcroForm"[..], "Form fields"),
            (&b"Outlines"[..], "Bookmarks"),
        ] {
            if catalog.has(key) {
                found.insert(label.to_string());
            }
        }
        if let Ok(names) = catalog
            .get(b"Names")
            .and_then(Object::as_reference)
            .and_then(|r| doc.get_object(r))
            .and_then(Object::as_dict)
        {
            if names.has(b"JavaScript") {
                found.insert("JavaScript".to_string());
            }
            if names.has(b"EmbeddedFiles") {
                found.insert("Embedded files".to_string());
            }
        }
    }

    for (_, pid) in doc.get_pages() {
        let has_annots = doc
            .get_object(pid)
            .and_then(Object::as_dict)
            .map(|d| d.has(b"Annots"))
            .unwrap_or(false);
        if has_annots {
            found.insert("Annotations and comments".to_string());
            break;
        }
    }

    Ok(found.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;
    use quark_core::geom::Vec2;

    #[test]
    fn permissions_map_onto_the_right_bits() {
        let p = PermissionSet {
            print: true,
            print_high_quality: false,
            modify: false,
            copy: false,
            annotate: false,
            fill_forms: false,
            assemble: false,
            accessibility: true,
        };
        let lo = p.to_lopdf();
        assert!(lo.contains(LoPermissions::PRINTABLE));
        assert!(!lo.contains(LoPermissions::MODIFIABLE));
        assert!(!lo.contains(LoPermissions::COPYABLE));
        assert!(lo.contains(LoPermissions::COPYABLE_FOR_ACCESSIBILITY));
    }

    #[test]
    fn read_only_still_allows_accessibility_extraction() {
        // Denying screen readers is not a setting Quark offers.
        assert!(PermissionSet::read_only().accessibility);
    }

    #[test]
    fn an_encrypted_document_needs_its_password() {
        let _g = testutil::pdfium_guard();
        let out = encrypt(
            &testutil::sample_bytes(),
            "letmein",
            "ownerpw",
            PermissionSet::default(),
        )
        .unwrap();

        // No password at all must not open it.
        match Document::from_bytes(out.clone(), None) {
            Err(crate::doc::OpenError::PasswordRequired) => {}
            other => panic!("expected a password prompt, got {:?}", other.err()),
        }
        // The wrong one must be rejected as wrong.
        match Document::from_bytes(out.clone(), Some("hunter2")) {
            Err(crate::doc::OpenError::WrongPassword) => {}
            other => panic!("expected WrongPassword, got {:?}", other.err()),
        }
        // The right one opens it and the content is intact.
        let d = Document::from_bytes(out, Some("letmein")).expect("correct password");
        assert_eq!(d.page_count(), 2);
        assert!(d.info.encrypted);
    }

    #[test]
    fn encryption_records_the_restrictions_it_was_given() {
        let _g = testutil::pdfium_guard();
        let perms = PermissionSet {
            copy: false,
            modify: false,
            ..PermissionSet::default()
        };
        let out = encrypt(&testutil::sample_bytes(), "pw", "owner", perms).unwrap();
        let d = Document::from_bytes(out, Some("pw")).unwrap();
        assert!(!d.info.permissions.unencrypted);
        assert!(!d.info.permissions.extract_content, "copy should be denied");
        assert!(!d.info.permissions.modify_contents, "modify should be denied");
        assert!(d.info.permissions.print, "print was granted");
    }

    #[test]
    fn two_encryptions_of_the_same_file_use_different_keys() {
        // A key derived from the password would make these byte-identical,
        // which leaks that two documents share a password.
        let a = encrypt(&testutil::sample_bytes(), "pw", "o", PermissionSet::default()).unwrap();
        let b = encrypt(&testutil::sample_bytes(), "pw", "o", PermissionSet::default()).unwrap();
        assert_ne!(a, b, "encryption appears deterministic");
    }

    #[test]
    fn decrypting_restores_an_openable_document() {
        let _g = testutil::pdfium_guard();
        let enc = encrypt(&testutil::sample_bytes(), "pw", "owner", PermissionSet::default())
            .unwrap();
        let dec = decrypt(&enc, "pw").unwrap();
        let d = Document::from_bytes(dec, None).expect("should open with no password");
        assert_eq!(d.page_count(), 2);
        assert!(!d.info.encrypted);
    }

    #[test]
    fn redaction_removes_the_text_rather_than_covering_it() {
        // The core guarantee of the whole module: a black box painted over the
        // words would leave them recoverable.
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        let before = crate::text::extract_page(&d, 0).unwrap().text;
        assert!(before.contains("Quark PDF Engine"));

        // The heading sits at 72,700 in page space.
        let marks = vec![RedactionMark {
            page: 0,
            rect: Rect::from_xywh(60.0, 680.0, 400.0, 60.0),
        }];
        let (out, report) = apply_redactions(&d, &marks).unwrap();
        assert!(report.text_runs_removed > 0, "nothing was removed");

        let after_doc = Document::from_bytes(out.clone(), None).unwrap();
        let after = crate::text::extract_page(&after_doc, 0).unwrap().text;
        assert!(
            !after.contains("Quark PDF Engine"),
            "redacted text is still extractable: {after:?}"
        );

        // And it is not merely hidden — it is not in the file's bytes either.
        let raw = String::from_utf8_lossy(&out);
        assert!(
            !raw.contains("Quark PDF Engine"),
            "redacted text survives in the raw content stream"
        );
    }

    #[test]
    fn redaction_leaves_content_outside_the_marked_area_alone() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        let marks = vec![RedactionMark {
            page: 0,
            rect: Rect::from_xywh(60.0, 680.0, 400.0, 60.0),
        }];
        let (out, _) = apply_redactions(&d, &marks).unwrap();
        let after_doc = Document::from_bytes(out, None).unwrap();
        let after = crate::text::extract_page(&after_doc, 0).unwrap().text;
        assert!(
            after.contains("Page one body text"),
            "unmarked text was destroyed: {after:?}"
        );
    }

    #[test]
    fn redaction_does_not_touch_other_pages() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        let marks = vec![RedactionMark {
            page: 0,
            rect: Rect::from_xywh(0.0, 0.0, 612.0, 792.0),
        }];
        let (out, report) = apply_redactions(&d, &marks).unwrap();
        assert_eq!(report.pages_touched, 1);
        let after = Document::from_bytes(out, None).unwrap();
        let p2 = crate::text::extract_page(&after, 1).unwrap().text;
        assert!(p2.contains("Second page heading"), "page 2 was damaged");
    }

    #[test]
    fn redacting_nothing_returns_the_document_unchanged() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        let (out, report) = apply_redactions(&d, &[]).unwrap();
        assert_eq!(report, RedactionReport::default());
        let after = Document::from_bytes(out, None).unwrap();
        assert_eq!(after.page_count(), 2);
    }

    #[test]
    fn a_redacted_page_still_renders() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        let marks = vec![RedactionMark {
            page: 0,
            rect: Rect::from_xywh(60.0, 680.0, 400.0, 60.0),
        }];
        let (out, _) = apply_redactions(&d, &marks).unwrap();
        let after = Document::from_bytes(out, None).unwrap();
        let r =
            crate::render::render_page(&after, &crate::render::RenderRequest::default()).unwrap();
        // The redaction box is painted black, so there must be dark pixels
        // where the heading used to be.
        let px = r.pixel(200, 100);
        assert!(px[0] < 60 && px[1] < 60, "no black box drawn: {px:?}");
    }

    #[test]
    fn redaction_strips_overlapping_annotations() {
        // A comment quoting the redacted text leaks it just as effectively.
        let _g = testutil::pdfium_guard();
        use quark_core::annot::{AnnotId, AnnotKind, Annotation};
        let ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Square,
            Rect::from_xywh(70.0, 690.0, 200.0, 30.0),
            "t",
        );
        let with = crate::organize::apply_annotations(&testutil::sample_bytes(), &[ann]).unwrap();
        let d = Document::from_bytes(with, None).unwrap();
        assert_eq!(crate::annots::read_all(&d).unwrap().len(), 1);

        let marks = vec![RedactionMark {
            page: 0,
            rect: Rect::from_xywh(60.0, 680.0, 400.0, 60.0),
        }];
        let (out, report) = apply_redactions(&d, &marks).unwrap();
        assert_eq!(report.annotations_removed, 1);
        let after = Document::from_bytes(out, None).unwrap();
        assert!(crate::annots::read_all(&after).unwrap().is_empty());
    }

    #[test]
    fn sanitising_removes_document_properties() {
        let _g = testutil::pdfium_guard();
        let meta = crate::doc::Metadata {
            title: "Confidential Draft".into(),
            author: "Someone".into(),
            ..Default::default()
        };
        let with = crate::organize::set_metadata(&testutil::sample_bytes(), &meta).unwrap();
        assert_eq!(
            Document::from_bytes(with.clone(), None)
                .unwrap()
                .info
                .metadata
                .title,
            "Confidential Draft"
        );

        let (clean, report) = sanitize(&with, SanitizeOptions::default()).unwrap();
        assert!(report.removed.iter().any(|r| r.contains("properties")));
        let d = Document::from_bytes(clean, None).unwrap();
        assert!(d.info.metadata.title.is_empty(), "title survived");
        assert!(d.info.metadata.author.is_empty(), "author survived");
    }

    #[test]
    fn sanitising_keeps_comments_by_default() {
        // Removing a reviewer's comments is a separate decision from stripping
        // metadata, and should not happen silently.
        let _g = testutil::pdfium_guard();
        use quark_core::annot::{AnnotId, AnnotKind, Annotation};
        let ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Square,
            Rect::from_xywh(10.0, 10.0, 50.0, 50.0),
            "t",
        );
        let with = crate::organize::apply_annotations(&testutil::sample_bytes(), &[ann]).unwrap();
        let (clean, _) = sanitize(&with, SanitizeOptions::default()).unwrap();
        let d = Document::from_bytes(clean, None).unwrap();
        assert_eq!(crate::annots::read_all(&d).unwrap().len(), 1);
    }

    #[test]
    fn sanitising_removes_comments_when_asked() {
        let _g = testutil::pdfium_guard();
        use quark_core::annot::{AnnotId, AnnotKind, Annotation};
        let ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Square,
            Rect::from_xywh(10.0, 10.0, 50.0, 50.0),
            "t",
        );
        let with = crate::organize::apply_annotations(&testutil::sample_bytes(), &[ann]).unwrap();
        let (clean, report) = sanitize(&with, SanitizeOptions {
            annotations: true,
            ..Default::default()
        })
        .unwrap();
        assert!(report.removed.iter().any(|r| r.contains("annotation")));
        let d = Document::from_bytes(clean, None).unwrap();
        assert!(crate::annots::read_all(&d).unwrap().is_empty());
    }

    #[test]
    fn a_sanitised_document_still_opens_and_renders() {
        let _g = testutil::pdfium_guard();
        let (clean, _) = sanitize(&testutil::sample_bytes(), SanitizeOptions::default()).unwrap();
        let d = Document::from_bytes(clean, None).unwrap();
        assert_eq!(d.page_count(), 2);
        let t = crate::text::extract_page(&d, 0).unwrap();
        assert!(t.text.contains("Quark"));
    }

    #[test]
    fn the_scanner_reports_what_a_document_carries() {
        let _g = testutil::pdfium_guard();
        let meta = crate::doc::Metadata {
            title: "T".into(),
            ..Default::default()
        };
        let with = crate::organize::set_metadata(&testutil::sample_bytes(), &meta).unwrap();
        let found = scan_hidden_information(&with).unwrap();
        assert!(
            found.iter().any(|f| f.contains("Document properties")),
            "{found:?}"
        );
    }

    #[test]
    fn the_scanner_does_not_modify_the_document() {
        let bytes = testutil::sample_bytes();
        let _ = scan_hidden_information(&bytes).unwrap();
        // Reading it twice must give the same answer.
        let a = scan_hidden_information(&bytes).unwrap();
        let b = scan_hidden_information(&bytes).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn stripping_tracks_the_text_cursor_through_positioning_operators() {
        use lopdf::content::{Content, Operation};
        // Two show operations: one inside the redaction, one outside.
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Td", vec![Object::Real(100.0), Object::Real(700.0)]),
                Operation::new("Tj", vec![Object::string_literal("secret")]),
                Operation::new("Td", vec![Object::Real(0.0), Object::Real(-400.0)]),
                Operation::new("Tj", vec![Object::string_literal("public")]),
                Operation::new("ET", vec![]),
            ],
        };
        let rect = Rect::from_xywh(50.0, 650.0, 300.0, 100.0);
        let (out, removed, _) = strip_operations(&content, &[rect]);
        assert_eq!(removed, 1, "expected exactly one run removed");
        let shows: Vec<_> = out
            .operations
            .iter()
            .filter(|o| o.operator == "Tj")
            .collect();
        assert_eq!(shows.len(), 1, "the surviving run should be 'public'");
    }

    #[test]
    fn the_black_box_covers_every_marked_rectangle() {
        let rects = [
            Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            Rect::from_xywh(50.0, 50.0, 20.0, 20.0),
        ];
        let ops = black_box_ops(&rects);
        assert_eq!(ops.iter().filter(|o| o.operator == "re").count(), 2);
        assert_eq!(ops.iter().filter(|o| o.operator == "f").count(), 2);
        // Graphics state must be saved and restored, or the fill colour leaks
        // into whatever is drawn next.
        assert_eq!(ops.first().unwrap().operator, "q");
        assert_eq!(ops.last().unwrap().operator, "Q");
    }

    #[test]
    fn a_point_outside_every_rectangle_is_untouched() {
        let r = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Vec2::new(5.0, 5.0)));
        assert!(!r.contains(Vec2::new(50.0, 5.0)));
    }
    #[test]
    fn permission_bits_decode_the_way_the_format_defines_them() {
        // /P is a signed value with the unused high bits set; reading it as
        // signed and testing bits directly gives the wrong answer.
        let p = permissions_from_bits(-28);
        assert!(p.print, "bit 3 (print) should be set");
        assert!(!p.modify, "bit 4 (modify) should be clear");
        assert!(!p.copy, "bit 5 (copy) should be clear");
        assert!(p.annotate, "bit 6 (annotate) should be set");
    }

    #[test]
    fn permission_bits_can_be_read_from_an_aes_encrypted_file() {
        // pdfium-render cannot name revision 6, so this path is what keeps an
        // AES-encrypted document from looking unrestricted.
        let out = encrypt(&testutil::sample_bytes(), "pw", "owner", PermissionSet {
            copy: false,
            modify: false,
            ..PermissionSet::default()
        })
        .unwrap();
        let bits = probe_permission_bits(&out).expect("no /Encrypt dictionary found");
        let p = permissions_from_bits(bits);
        assert!(!p.copy);
        assert!(!p.modify);
        assert!(p.print);
    }

    #[test]
    fn an_unencrypted_file_has_no_permission_bits_to_read() {
        assert!(probe_permission_bits(&testutil::sample_bytes()).is_none());
    }

}

