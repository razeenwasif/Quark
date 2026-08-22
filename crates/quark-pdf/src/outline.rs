//! Bookmarks (the document outline), attachments and layers.

use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::engine::EngineError;

/// One entry in the outline, with its children.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    /// Zero-based destination page, when the bookmark resolves to one.
    pub page: Option<usize>,
    pub children: Vec<Bookmark>,
    /// Depth from the root, so the panel can indent without recursing.
    pub depth: usize,
}

impl Bookmark {
    /// Total entries in this subtree, counting itself.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Bookmark::count).sum::<usize>()
    }

    /// Flattens the tree into display order.
    pub fn flatten(&self, out: &mut Vec<Bookmark>) {
        let mut node = self.clone();
        node.children = Vec::new();
        out.push(node);
        for c in &self.children {
            c.flatten(out);
        }
    }
}

/// The maximum outline depth Quark will follow.
///
/// A malformed file can contain a cycle in the outline tree; without a limit
/// the walk never terminates and the application hangs on open.
const MAX_DEPTH: usize = 32;

/// Reads the document outline.
///
/// PDFium's `root()` is not an invisible container: it is the *first* top-level
/// bookmark, so the remaining top-level entries are its siblings. Treating it
/// as a root and reading its children instead silently returns only the first
/// chapter's subsections.
pub fn read_outline(doc: &Document) -> Vec<Bookmark> {
    let bookmarks = doc.inner().bookmarks();
    let Some(first) = bookmarks.root() else {
        return Vec::new();
    };
    siblings(first, 0)
}

/// Walks a bookmark and everything after it at the same level.
///
/// The step count is capped because `/Next` can form a cycle in a damaged
/// file, and following one would hang the application on open.
fn siblings(first: PdfBookmark<'_>, depth: usize) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let mut cur = Some(first);
    let mut steps = 0;
    while let Some(b) = cur {
        out.push(read_node(&b, depth));
        steps += 1;
        if steps > 10_000 {
            tracing::warn!("outline sibling chain looks cyclic; stopping");
            break;
        }
        cur = b.next_sibling();
    }
    out
}

fn read_node(b: &PdfBookmark, depth: usize) -> Bookmark {
    let children = match b.first_child() {
        Some(c) if depth + 1 < MAX_DEPTH => siblings(c, depth + 1),
        _ => Vec::new(),
    };
    Bookmark {
        title: b.title().unwrap_or_else(|| "Untitled".into()),
        page: b
            .destination()
            .and_then(|d| d.page_index().ok())
            .map(|i| i as usize),
        children,
        depth,
    }
}

/// The outline flattened into display order.
pub fn flat_outline(doc: &Document) -> Vec<Bookmark> {
    let mut out = Vec::new();
    for b in read_outline(doc) {
        b.flatten(&mut out);
    }
    out
}


/// A clickable region on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub page: usize,
    /// The clickable area, in page space.
    pub rect: crate::doc::LinkRect,
    /// Where the link goes within this document.
    pub target_page: usize,
    /// An external URI, when the link leaves the document.
    pub uri: Option<String>,
}

/// Reads every internal link in the document.
///
/// Only links with a destination inside the document are returned with a
/// target page; external URIs are reported separately so the caller can decide
/// whether to open a browser rather than silently navigating nowhere.
pub fn read_links(doc: &Document) -> Vec<Link> {
    let mut out = Vec::new();
    for (page_index, page) in doc.inner().pages().iter().enumerate() {
        for link in page.links().iter() {
            let Ok(r) = link.rect() else { continue };
            let rect = crate::doc::LinkRect {
                min_x: r.left().value,
                min_y: r.bottom().value,
                max_x: r.right().value,
                max_y: r.top().value,
            };
            let target = link.destination().and_then(|d| d.page_index().ok());
            let uri = link.action().and_then(|a| {
                a.as_uri_action().and_then(|u| u.uri().ok())
            });
            // A link with neither a destination nor a URI does nothing; keeping
            // it would give the viewer an invisible hotspot.
            let Some(target_page) = target.map(|t| t as usize).or(Some(page_index)) else {
                continue;
            };
            if target.is_none() && uri.is_none() {
                continue;
            }
            out.push(Link {
                page: page_index,
                rect,
                target_page,
                uri,
            });
        }
    }
    out
}

/// An embedded file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub size: usize,
}

/// Lists the document's embedded files.
pub fn read_attachments(doc: &Document) -> Vec<Attachment> {
    doc.inner()
        .attachments()
        .iter()
        .enumerate()
        .map(|(i, a)| Attachment {
            index: i,
            name: a.name(),
            // The length is only knowable by fetching the bytes, and an
            // attachment can be large, so this reports what it costs to know.
            size: a.save_to_bytes().map(|b| b.len()).unwrap_or(0),
        })
        .collect()
}

/// Extracts one attachment's bytes.
pub fn extract_attachment(doc: &Document, index: usize) -> Result<Vec<u8>, EngineError> {
    let attachments = doc.inner().attachments();
    let a = attachments
        .get(index as u16)
        .map_err(|_| EngineError::Pdfium(format!("no attachment {index}")))?;
    a.save_to_bytes().map_err(EngineError::from)
}

/// Writes a bookmark tree into a document's object graph.
///
/// PDFium cannot create outline entries, so this goes through lopdf like the
/// other structural edits.
pub fn write_outline(bytes: &[u8], roots: &[Bookmark]) -> Result<Vec<u8>, EngineError> {
    use lopdf::{Dictionary, Document as LoDocument, Object, ObjectId};

    let mut doc = LoDocument::load_mem(bytes)
        .map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))?;

    let catalog_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| EngineError::Pdfium("document has no /Root".into()))?;

    let pages: Vec<ObjectId> = {
        let mut v: Vec<_> = doc.get_pages().into_iter().collect();
        v.sort_by_key(|(n, _)| *n);
        v.into_iter().map(|(_, id)| id).collect()
    };

    if roots.is_empty() {
        if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
            d.remove(b"Outlines");
        }
        let mut out = Vec::new();
        doc.save_to(&mut out)
            .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
        return Ok(out);
    }

    // Reserve the outline root first: every entry needs to point back at its
    // parent, so the parent's id has to exist before its children are built.
    let outline_root = doc.add_object(Object::Dictionary(Dictionary::new()));

    fn build(
        doc: &mut LoDocument,
        items: &[Bookmark],
        parent: ObjectId,
        pages: &[ObjectId],
    ) -> Option<(ObjectId, ObjectId, i64)> {
        let mut ids = Vec::new();
        for item in items {
            let id = doc.add_object(Object::Dictionary(Dictionary::new()));
            ids.push((id, item));
        }
        if ids.is_empty() {
            return None;
        }

        let mut total = 0i64;
        for (i, (id, item)) in ids.iter().enumerate() {
            let mut d = Dictionary::new();
            d.set("Title", Object::string_literal(item.title.clone()));
            d.set("Parent", Object::Reference(parent));
            if i > 0 {
                d.set("Prev", Object::Reference(ids[i - 1].0));
            }
            if i + 1 < ids.len() {
                d.set("Next", Object::Reference(ids[i + 1].0));
            }
            if let Some(p) = item.page.and_then(|p| pages.get(p)) {
                // An explicit /Fit destination, which lands the reader on the
                // page without imposing a zoom level on them.
                d.set("Dest", Object::Array(vec![
                    Object::Reference(*p),
                    Object::Name(b"Fit".to_vec()),
                ]));
            }

            if let Some((first, last, count)) = build(doc, &item.children, *id, pages) {
                d.set("First", Object::Reference(first));
                d.set("Last", Object::Reference(last));
                // A positive /Count means the entry is open; negative means
                // collapsed. Quark writes them open.
                d.set("Count", Object::Integer(count));
                total += count;
            }
            total += 1;
            doc.set_object(*id, Object::Dictionary(d));
        }

        Some((ids[0].0, ids[ids.len() - 1].0, total))
    }

    let built = build(&mut doc, roots, outline_root, &pages);

    let mut root_dict = Dictionary::new();
    root_dict.set("Type", Object::Name(b"Outlines".to_vec()));
    if let Some((first, last, count)) = built {
        root_dict.set("First", Object::Reference(first));
        root_dict.set("Last", Object::Reference(last));
        root_dict.set("Count", Object::Integer(count));
    }
    doc.set_object(outline_root, Object::Dictionary(root_dict));

    if let Ok(d) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
        d.set("Outlines", Object::Reference(outline_root));
    }

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn bm(title: &str, page: usize, children: Vec<Bookmark>) -> Bookmark {
        Bookmark {
            title: title.into(),
            page: Some(page),
            children,
            depth: 0,
        }
    }

    #[test]
    fn a_document_without_an_outline_reports_none() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(read_outline(&d).is_empty());
        assert!(read_attachments(&d).is_empty());
    }

    #[test]
    fn counting_a_subtree_includes_every_descendant() {
        let tree = bm("A", 0, vec![bm("B", 1, vec![bm("C", 1, vec![])]), bm("D", 2, vec![])]);
        assert_eq!(tree.count(), 4);
    }

    #[test]
    fn flattening_preserves_depth_first_order() {
        let tree = bm("A", 0, vec![bm("B", 1, vec![bm("C", 1, vec![])]), bm("D", 2, vec![])]);
        let mut out = Vec::new();
        tree.flatten(&mut out);
        let titles: Vec<&str> = out.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn a_written_outline_reads_back_with_its_structure() {
        let _g = testutil::pdfium_guard();
        let tree = vec![
            bm("Introduction", 0, vec![bm("Background", 0, vec![])]),
            bm("Results", 1, vec![]),
        ];
        let out = write_outline(&testutil::sample_bytes(), &tree).unwrap();
        let d = Document::from_bytes(out, None).expect("PDFium rejected our outline");

        let read = read_outline(&d);
        assert_eq!(read.len(), 2, "expected two top-level entries");
        assert_eq!(read[0].title, "Introduction");
        assert_eq!(read[1].title, "Results");
        assert_eq!(read[0].children.len(), 1);
        assert_eq!(read[0].children[0].title, "Background");
    }

    #[test]
    fn bookmark_destinations_resolve_to_the_right_page() {
        let _g = testutil::pdfium_guard();
        let tree = vec![bm("First", 0, vec![]), bm("Second", 1, vec![])];
        let out = write_outline(&testutil::sample_bytes(), &tree).unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        let read = read_outline(&d);
        assert_eq!(read[0].page, Some(0));
        assert_eq!(read[1].page, Some(1), "second bookmark points at page 2");
    }

    #[test]
    fn depth_is_recorded_for_indentation() {
        let _g = testutil::pdfium_guard();
        let tree = vec![bm("Top", 0, vec![bm("Nested", 0, vec![])])];
        let out = write_outline(&testutil::sample_bytes(), &tree).unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        let flat = flat_outline(&d);
        assert_eq!(flat[0].depth, 0);
        assert_eq!(flat[1].depth, 1);
    }

    #[test]
    fn writing_an_empty_outline_removes_it() {
        let _g = testutil::pdfium_guard();
        let with = write_outline(&testutil::sample_bytes(), &[bm("X", 0, vec![])]).unwrap();
        assert!(!read_outline(&Document::from_bytes(with.clone(), None).unwrap()).is_empty());
        let without = write_outline(&with, &[]).unwrap();
        let d = Document::from_bytes(without, None).unwrap();
        assert!(read_outline(&d).is_empty());
    }

    #[test]
    fn the_document_still_renders_after_an_outline_is_added() {
        // A malformed outline can make PDFium reject the whole file.
        let _g = testutil::pdfium_guard();
        let out = write_outline(&testutil::sample_bytes(), &[bm("A", 0, vec![])]).unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        assert_eq!(d.page_count(), 2);
        let r = crate::render::render_page(&d, &crate::render::RenderRequest::default()).unwrap();
        assert!(r.width > 0);
    }

    #[test]
    fn extracting_a_missing_attachment_is_an_error() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(extract_attachment(&d, 0).is_err());
    }
}
