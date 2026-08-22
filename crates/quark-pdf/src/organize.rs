//! Structural editing: pages, metadata, and applying annotations.
//!
//! This is the lopdf half of the engine. PDFium renders and reads; everything
//! that changes the file's object graph happens here, because PDFium's writing
//! API does not reach the page tree, the document information dictionary, or
//! most annotation subtypes.
//!
//! Operations take and return whole PDF byte buffers. That is deliberate: the
//! caller already has to reload the document into PDFium afterwards for the
//! change to appear on screen, and passing bytes keeps the two libraries from
//! ever holding the same file open at once.

use std::collections::BTreeSet;

use lopdf::{Dictionary, Document as LoDocument, Object, ObjectId};
use quark_core::annot::Annotation;
use quark_core::geom::Rot;

use crate::annots;
use crate::doc::Metadata;
use crate::engine::EngineError;

type Result<T> = std::result::Result<T, EngineError>;

fn load(bytes: &[u8]) -> Result<LoDocument> {
    LoDocument::load_mem(bytes).map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))
}

fn save(mut doc: LoDocument) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok(out)
}

/// Looks up a page attribute, following `/Parent` up the page tree.
///
/// `/Resources`, `/MediaBox`, `/CropBox` and `/Rotate` are inheritable: a page
/// may omit them and take the value from an ancestor node. lopdf has no helper
/// for this, and reading the page dictionary alone would report a page as
/// having no resources at all — which is how a flattened tree ends up rendering
/// blank pages.
///
/// The walk is depth-limited because a malformed file can contain a `/Parent`
/// cycle, and following one would hang the application on open.
fn inherited(doc: &LoDocument, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut id = page;
    for _ in 0..64 {
        let dict = doc.get_object(id).and_then(Object::as_dict).ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(v.clone());
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// The document's pages, in order.
fn page_ids(doc: &LoDocument) -> Vec<ObjectId> {
    let pages = doc.get_pages();
    let mut ids: Vec<_> = pages.into_iter().collect();
    ids.sort_by_key(|(n, _)| *n);
    ids.into_iter().map(|(_, id)| id).collect()
}

/// Rewrites the page tree so that `order` becomes the document's page order.
///
/// Every page-level operation funnels through here rather than editing `/Kids`
/// in place, because a page tree can be nested arbitrarily and an in-place edit
/// has to handle intermediate nodes, inherited attributes and `/Count` at every
/// level. Flattening to a single-level tree is correct for all of them and is
/// what most producers emit anyway.
fn set_page_order(doc: &mut LoDocument, order: &[ObjectId]) -> Result<()> {
    let catalog_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| EngineError::Pdfium("document has no /Root".into()))?;

    let pages_id = doc
        .get_object(catalog_id)
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|_| EngineError::Pdfium("catalog has no /Pages".into()))?;

    // Inherited attributes live on the tree nodes, so a page that relied on
    // inheriting /Resources or /MediaBox from a parent we are about to discard
    // must have them written onto itself first.
    let inheritable: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
    for &pid in order {
        for key in inheritable {
            if doc
                .get_object(pid)
                .and_then(Object::as_dict)
                .map(|d| d.has(key))
                .unwrap_or(false)
            {
                continue;
            }
            if let Some(v) = inherited(doc, pid, key) {
                if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
                    d.set(key.to_vec(), v);
                }
            }
        }
        // Re-parent onto the root node.
        if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
            d.set("Parent", Object::Reference(pages_id));
        }
    }

    let kids: Vec<Object> = order.iter().map(|id| Object::Reference(*id)).collect();
    let count = order.len() as i64;
    let node = doc
        .get_object_mut(pages_id)
        .and_then(Object::as_dict_mut)
        .map_err(|_| EngineError::Pdfium("/Pages is not a dictionary".into()))?;
    node.set("Kids", Object::Array(kids));
    node.set("Count", Object::Integer(count));
    Ok(())
}

/// Deletes pages by zero-based index.
pub fn delete_pages(bytes: &[u8], indices: &[usize]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);
    let drop: BTreeSet<usize> = indices.iter().copied().collect();

    if drop.len() >= ids.len() {
        return Err(EngineError::Pdfium(
            "a document must keep at least one page".into(),
        ));
    }

    let kept: Vec<ObjectId> = ids
        .iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, id)| *id)
        .collect();

    set_page_order(&mut doc, &kept)?;
    // The removed page objects are now unreachable; pruning them is what makes
    // deletion actually reduce the file size.
    doc.prune_objects();
    save(doc)
}

/// Moves one page to a new index.
pub fn move_page(bytes: &[u8], from: usize, to: usize) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let mut ids = page_ids(&doc);
    if from >= ids.len() {
        return Err(EngineError::Pdfium(format!("no page {from}")));
    }
    let id = ids.remove(from);
    // Clamping rather than erroring: dragging a page past the end of the list
    // is a normal gesture and should land it last.
    let to = to.min(ids.len());
    ids.insert(to, id);
    set_page_order(&mut doc, &ids)?;
    save(doc)
}

/// Reorders the whole document. `order` holds the old index of each new slot.
pub fn reorder_pages(bytes: &[u8], order: &[usize]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);
    if order.len() != ids.len() {
        return Err(EngineError::Pdfium(format!(
            "reorder needs {} entries, got {}",
            ids.len(),
            order.len()
        )));
    }
    let mut seen = vec![false; ids.len()];
    let mut new_order = Vec::with_capacity(ids.len());
    for &i in order {
        let id = *ids
            .get(i)
            .ok_or_else(|| EngineError::Pdfium(format!("page {i} out of range")))?;
        if std::mem::replace(&mut seen[i], true) {
            return Err(EngineError::Pdfium(format!("page {i} listed twice")));
        }
        new_order.push(id);
    }
    set_page_order(&mut doc, &new_order)?;
    save(doc)
}

/// Rotates pages by a quarter-turn multiple, relative to their current angle.
pub fn rotate_pages(bytes: &[u8], indices: &[usize], by: Rot) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);
    for &i in indices {
        let Some(&pid) = ids.get(i) else { continue };
        // /Rotate is inheritable, so the existing angle may come from a parent
        // node rather than from the page itself.
        let current = inherited(&doc, pid, b"Rotate")
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let next = (current as i32 + by.degrees()).rem_euclid(360);
        if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
            d.set("Rotate", Object::Integer(next as i64));
        }
    }
    save(doc)
}

/// Extracts the given pages into a new document, in the order listed.
pub fn extract_pages(bytes: &[u8], indices: &[usize]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);
    let mut keep = Vec::new();
    for &i in indices {
        let id = *ids
            .get(i)
            .ok_or_else(|| EngineError::Pdfium(format!("no page {i}")))?;
        keep.push(id);
    }
    if keep.is_empty() {
        return Err(EngineError::Pdfium("no pages selected".into()));
    }
    set_page_order(&mut doc, &keep)?;
    doc.prune_objects();
    save(doc)
}

/// Concatenates documents into one.
///
/// Object numbers collide between any two PDFs, so each document after the
/// first is renumbered above everything already merged before its objects are
/// taken in.
pub fn merge(documents: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut docs: Vec<LoDocument> = documents
        .iter()
        .map(|b| load(b))
        .collect::<Result<Vec<_>>>()?;

    if docs.is_empty() {
        return Err(EngineError::Pdfium("nothing to merge".into()));
    }
    if docs.len() == 1 {
        return save(docs.remove(0));
    }

    let mut base = docs.remove(0);
    let mut order = page_ids(&base);

    for mut other in docs {
        let offset = base.max_id + 1;
        other.renumber_objects_with(offset);
        let others_pages = page_ids(&other);
        base.objects.extend(other.objects);
        base.max_id = base.max_id.max(other.max_id);
        order.extend(others_pages);
    }

    set_page_order(&mut base, &order)?;
    save(base)
}

/// Inserts every page of `source` into `bytes` at `at`.
pub fn insert_pages(bytes: &[u8], source: &[u8], at: usize) -> Result<Vec<u8>> {
    let mut base = load(bytes)?;
    let mut other = load(source)?;

    let offset = base.max_id + 1;
    other.renumber_objects_with(offset);
    let incoming = page_ids(&other);
    base.objects.extend(other.objects);
    base.max_id = base.max_id.max(other.max_id);

    let mut order = page_ids(&base);
    // page_ids now reports only the base's own pages, because the incoming
    // page tree was never linked in. Splice the incoming ids in by hand.
    order.retain(|id| !incoming.contains(id));
    let at = at.min(order.len());
    for (n, id) in incoming.into_iter().enumerate() {
        order.insert(at + n, id);
    }

    set_page_order(&mut base, &order)?;
    save(base)
}

/// Sets the crop box on the given pages.
pub fn crop_pages(bytes: &[u8], indices: &[usize], box_: [f32; 4]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);
    for &i in indices {
        let Some(&pid) = ids.get(i) else { continue };
        if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
            d.set(
                "CropBox",
                Object::Array(box_.iter().map(|v| Object::Real(*v)).collect()),
            );
        }
    }
    save(doc)
}

/// Writes the document information dictionary.
pub fn set_metadata(bytes: &[u8], meta: &Metadata) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;

    let info_id = match doc.trailer.get(b"Info").and_then(Object::as_reference) {
        Ok(id) => id,
        Err(_) => {
            let id = doc.add_object(Object::Dictionary(Dictionary::new()));
            doc.trailer.set("Info", Object::Reference(id));
            id
        }
    };

    let mut d = Dictionary::new();
    let mut put = |k: &str, v: &str| {
        if !v.is_empty() {
            d.set(k, Object::string_literal(v));
        }
    };
    put("Title", &meta.title);
    put("Author", &meta.author);
    put("Subject", &meta.subject);
    put("Keywords", &meta.keywords);
    put("Creator", &meta.creator);
    // The producer names the software that last wrote the file, which is now us.
    d.set("Producer", Object::string_literal("Quark"));
    if !meta.creation_date.is_empty() {
        d.set("CreationDate", Object::string_literal(meta.creation_date.clone()));
    }
    d.set(
        "ModDate",
        Object::string_literal(pdf_now()),
    );

    doc.set_object(info_id, Object::Dictionary(d));
    save(doc)
}

fn pdf_now() -> String {
    let rfc = quark_core::annot::now_rfc3339();
    let digits: String = rfc.chars().filter(|c| c.is_ascii_digit()).collect();
    format!("D:{}Z", &digits[..digits.len().min(14)])
}

/// Adds annotations to their pages.
///
/// Annotations already in the file are left untouched; only the ones passed in
/// are appended. The caller is responsible for having removed anything it means
/// to replace, via [`remove_annotations`].
pub fn apply_annotations(bytes: &[u8], anns: &[Annotation]) -> Result<Vec<u8>> {
    if anns.is_empty() {
        return Ok(bytes.to_vec());
    }
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);

    for ann in anns {
        let Some(&pid) = ids.get(ann.page) else {
            continue;
        };
        let dict = annots::to_dict(ann);
        let aid = doc.add_object(Object::Dictionary(dict));
        // Link the annotation back to its page. /P is optional but several
        // readers use it to decide which page a comment belongs to when the
        // annotation is reached through the comments list rather than the page.
        if let Ok(d) = doc.get_object_mut(aid).and_then(Object::as_dict_mut) {
            d.set("P", Object::Reference(pid));
        }

        let existing: Vec<Object> = doc
            .get_object(pid)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Annots"))
            .map(|o| match o {
                Object::Array(a) => a.clone(),
                // /Annots may be an indirect reference to an array.
                Object::Reference(r) => doc
                    .get_object(*r)
                    .and_then(Object::as_array)
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            })
            .unwrap_or_default();

        let mut list = existing;
        list.push(Object::Reference(aid));
        if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
            d.set("Annots", Object::Array(list));
        }
    }

    save(doc)
}

/// Removes every annotation from the given pages, or from all pages when
/// `pages` is empty.
pub fn remove_annotations(bytes: &[u8], pages: &[usize]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    let ids = page_ids(&doc);
    for (i, &pid) in ids.iter().enumerate() {
        if !pages.is_empty() && !pages.contains(&i) {
            continue;
        }
        if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
            d.remove(b"Annots");
        }
    }
    doc.prune_objects();
    save(doc)
}

/// Number of pages, without going through PDFium.
pub fn page_count(bytes: &[u8]) -> Result<usize> {
    Ok(load(bytes)?.get_pages().len())
}

/// Rewrites the file with object streams and compression.
pub fn optimize(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    // Unreferenced objects accumulate through every edit; a document that has
    // had pages deleted and re-added can carry several times its own weight.
    doc.prune_objects();
    doc.compress();
    save(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use crate::testutil;
    use quark_core::annot::{AnnotId, AnnotKind, Annotation};
    use quark_core::geom::Rect;

    fn sample() -> Vec<u8> {
        testutil::sample_bytes()
    }

    fn many(n: usize) -> Vec<u8> {
        std::fs::read(testutil::many_page_pdf(n)).unwrap()
    }

    /// Reopens through PDFium, which is the real test that a file we wrote is
    /// one a renderer will accept.
    fn reopen(bytes: &[u8]) -> Document {
        Document::from_bytes(bytes.to_vec(), None).expect("PDFium rejected our output")
    }

    #[test]
    fn deleting_a_page_removes_exactly_one() {
        let _g = testutil::pdfium_guard();
        let out = delete_pages(&sample(), &[0]).unwrap();
        assert_eq!(page_count(&out).unwrap(), 1);
        let d = reopen(&out);
        assert_eq!(d.page_count(), 1);
        // The surviving page must be the second one.
        let t = crate::text::extract_page(&d, 0).unwrap();
        assert!(t.text.contains("Second page"), "kept the wrong page: {:?}", t.text);
    }

    #[test]
    fn deleting_every_page_is_refused() {
        // A zero-page PDF is invalid, and silently keeping one would be worse
        // than saying no.
        assert!(delete_pages(&sample(), &[0, 1]).is_err());
    }

    #[test]
    fn deleting_several_pages_at_once_keeps_the_rest_in_order() {
        let _g = testutil::pdfium_guard();
        let out = delete_pages(&many(6), &[1, 3]).unwrap();
        let d = reopen(&out);
        assert_eq!(d.page_count(), 4);
        let markers: Vec<String> = (0..4)
            .map(|i| crate::text::extract_page(&d, i).unwrap().text)
            .collect();
        assert!(markers[0].contains("Page 1"), "{markers:?}");
        assert!(markers[1].contains("Page 3"), "{markers:?}");
        assert!(markers[2].contains("Page 5"), "{markers:?}");
    }

    #[test]
    fn moving_a_page_puts_it_where_it_was_asked_for() {
        let _g = testutil::pdfium_guard();
        let out = move_page(&many(4), 0, 2).unwrap();
        let d = reopen(&out);
        let markers: Vec<String> = (0..4)
            .map(|i| crate::text::extract_page(&d, i).unwrap().text)
            .collect();
        assert!(markers[0].contains("Page 2"), "{markers:?}");
        assert!(markers[2].contains("Page 1"), "{markers:?}");
    }

    #[test]
    fn moving_a_page_past_the_end_lands_it_last() {
        let _g = testutil::pdfium_guard();
        let out = move_page(&many(3), 0, 99).unwrap();
        let d = reopen(&out);
        let last = crate::text::extract_page(&d, 2).unwrap().text;
        assert!(last.contains("Page 1"), "{last:?}");
    }

    #[test]
    fn reordering_reverses_a_document() {
        let _g = testutil::pdfium_guard();
        let out = reorder_pages(&many(4), &[3, 2, 1, 0]).unwrap();
        let d = reopen(&out);
        let first = crate::text::extract_page(&d, 0).unwrap().text;
        assert!(first.contains("Page 4"), "{first:?}");
    }

    #[test]
    fn a_reorder_that_drops_or_duplicates_a_page_is_refused() {
        // Silently losing a page here would be data loss.
        assert!(reorder_pages(&many(4), &[0, 1, 2]).is_err(), "wrong length");
        assert!(reorder_pages(&many(4), &[0, 0, 1, 2]).is_err(), "duplicate");
    }

    #[test]
    fn rotation_accumulates_and_wraps() {
        let _g = testutil::pdfium_guard();
        let out = rotate_pages(&sample(), &[0], Rot::D90).unwrap();
        let d = reopen(&out);
        assert_eq!(d.info.pages[0].rotation, Rot::D90);
        // Rotating three more quarter-turns returns to zero.
        let out = rotate_pages(&out, &[0], Rot::D270).unwrap();
        let d = reopen(&out);
        assert_eq!(d.info.pages[0].rotation, Rot::D0);
    }

    #[test]
    fn rotating_swaps_the_reported_page_size() {
        let _g = testutil::pdfium_guard();
        let out = rotate_pages(&sample(), &[0], Rot::D90).unwrap();
        let d = reopen(&out);
        let s = d.info.page_size(0);
        assert!(s.width > s.height, "expected landscape, got {s:?}");
    }

    #[test]
    fn extracting_pages_produces_a_document_of_just_those_pages() {
        let _g = testutil::pdfium_guard();
        let out = extract_pages(&many(5), &[3, 1]).unwrap();
        let d = reopen(&out);
        assert_eq!(d.page_count(), 2);
        let first = crate::text::extract_page(&d, 0).unwrap().text;
        assert!(first.contains("Page 4"), "order not honoured: {first:?}");
    }

    #[test]
    fn merging_concatenates_every_page() {
        let _g = testutil::pdfium_guard();
        let out = merge(&[many(2), many(3), sample()]).unwrap();
        let d = reopen(&out);
        assert_eq!(d.page_count(), 7);
    }

    #[test]
    fn merged_content_survives_from_both_documents() {
        // Object-number collisions between two PDFs are the classic merge bug;
        // it shows up as pages that render as copies of each other.
        let _g = testutil::pdfium_guard();
        let out = merge(&[many(2), sample()]).unwrap();
        let d = reopen(&out);
        let all = crate::text::extract_all(&d).unwrap();
        assert!(all.contains("Page 1 marker"), "lost the first document");
        assert!(all.contains("Quark PDF Engine"), "lost the second document");
    }

    #[test]
    fn merging_one_document_returns_it_unchanged_in_page_count() {
        let _g = testutil::pdfium_guard();
        let out = merge(&[sample()]).unwrap();
        assert_eq!(page_count(&out).unwrap(), 2);
    }

    #[test]
    fn merging_nothing_is_an_error() {
        assert!(merge(&[]).is_err());
    }

    #[test]
    fn inserting_splices_pages_at_the_requested_index() {
        let _g = testutil::pdfium_guard();
        let out = insert_pages(&many(3), &sample(), 1).unwrap();
        let d = reopen(&out);
        assert_eq!(d.page_count(), 5);
        let at1 = crate::text::extract_page(&d, 1).unwrap().text;
        assert!(at1.contains("Quark PDF Engine"), "{at1:?}");
        let at3 = crate::text::extract_page(&d, 3).unwrap().text;
        assert!(at3.contains("Page 2"), "{at3:?}");
    }

    #[test]
    fn metadata_round_trips_through_the_info_dictionary() {
        let _g = testutil::pdfium_guard();
        let meta = Metadata {
            title: "Quarterly Report".into(),
            author: "R. Wasif".into(),
            subject: "Finance".into(),
            keywords: "q3,revenue".into(),
            ..Default::default()
        };
        let out = set_metadata(&sample(), &meta).unwrap();
        let d = reopen(&out);
        assert_eq!(d.info.metadata.title, "Quarterly Report");
        assert_eq!(d.info.metadata.author, "R. Wasif");
        assert_eq!(d.info.metadata.subject, "Finance");
        assert_eq!(d.info.metadata.producer, "Quark");
    }

    #[test]
    fn cropping_sets_the_crop_box() {
        let _g = testutil::pdfium_guard();
        let out = crop_pages(&sample(), &[0], [72.0, 72.0, 540.0, 720.0]).unwrap();
        let d = reopen(&out);
        let s = d.info.page_size(0);
        // PDFium reports the crop box as the page size.
        assert!((s.width - 468.0).abs() < 1.0, "got {}", s.width);
        assert!((s.height - 648.0).abs() < 1.0, "got {}", s.height);
    }

    #[test]
    fn an_applied_annotation_is_readable_again() {
        let _g = testutil::pdfium_guard();
        let quad = Rect::from_xywh(72.0, 690.0, 200.0, 24.0);
        let mut ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Highlight { quads: vec![quad] },
            quad,
            "tester",
        );
        ann.contents = "look here".into();

        let out = apply_annotations(&sample(), &[ann]).unwrap();
        let d = reopen(&out);
        let read = crate::annots::read_all(&d).unwrap();
        assert_eq!(read.len(), 1);
        assert!(matches!(read[0].kind, AnnotKind::Highlight { .. }));
        assert_eq!(read[0].contents, "look here");
        assert_eq!(read[0].author, "tester");
    }

    #[test]
    fn annotations_land_on_the_page_they_name() {
        let _g = testutil::pdfium_guard();
        let anns: Vec<Annotation> = (0..2)
            .map(|p| {
                Annotation::new(
                    AnnotId(p as u64),
                    p,
                    AnnotKind::Square,
                    Rect::from_xywh(20.0, 20.0, 60.0, 60.0),
                    "t",
                )
            })
            .collect();
        let out = apply_annotations(&sample(), &anns).unwrap();
        let d = reopen(&out);
        let read = crate::annots::read_all(&d).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].page, 0);
        assert_eq!(read[1].page, 1);
    }

    #[test]
    fn shapes_pdfium_cannot_create_still_round_trip_through_lopdf() {
        // This is the whole reason writing goes through lopdf.
        let _g = testutil::pdfium_guard();
        let ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Circle,
            Rect::from_xywh(100.0, 100.0, 200.0, 120.0),
            "t",
        );
        let out = apply_annotations(&sample(), &[ann]).unwrap();
        let d = reopen(&out);
        let read = crate::annots::read_all(&d).unwrap();
        assert_eq!(read.len(), 1);
        assert!(matches!(read[0].kind, AnnotKind::Circle), "{:?}", read[0].kind);
    }

    #[test]
    fn ink_is_written_as_an_ink_list() {
        let _g = testutil::pdfium_guard();
        use quark_core::annot::InkStroke;
        use quark_core::geom::Vec2;
        let stroke = InkStroke {
            points: (0..10)
                .map(|i| Vec2::new(100.0 + i as f32 * 10.0, 400.0))
                .collect(),
        };
        let mut ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Ink {
                strokes: vec![stroke],
            },
            Rect::ZERO,
            "t",
        );
        ann.recompute_rect();
        let out = apply_annotations(&sample(), &[ann]).unwrap();
        // Assert on the bytes: /InkList is the representation the format wants.
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("/InkList"), "no /InkList in output");
        let d = reopen(&out);
        assert_eq!(crate::annots::read_all(&d).unwrap().len(), 1);
    }

    #[test]
    fn removing_annotations_clears_them() {
        let _g = testutil::pdfium_guard();
        let ann = Annotation::new(
            AnnotId(1),
            0,
            AnnotKind::Square,
            Rect::from_xywh(10.0, 10.0, 50.0, 50.0),
            "t",
        );
        let with = apply_annotations(&sample(), &[ann]).unwrap();
        assert_eq!(crate::annots::read_all(&reopen(&with)).unwrap().len(), 1);
        let without = remove_annotations(&with, &[]).unwrap();
        assert!(crate::annots::read_all(&reopen(&without)).unwrap().is_empty());
    }

    #[test]
    fn optimising_keeps_the_document_readable() {
        let _g = testutil::pdfium_guard();
        let out = optimize(&sample()).unwrap();
        let d = reopen(&out);
        assert_eq!(d.page_count(), 2);
        let t = crate::text::extract_page(&d, 0).unwrap();
        assert!(t.text.contains("Quark"));
    }

    #[test]
    fn page_count_agrees_with_pdfium() {
        let _g = testutil::pdfium_guard();
        let b = many(7);
        assert_eq!(page_count(&b).unwrap(), 7);
        assert_eq!(reopen(&b).page_count(), 7);
    }
}
