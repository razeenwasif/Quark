//! Text extraction, selection and search.
//!
//! PDF has no notion of a text selection: a page is a bag of positioned glyphs,
//! and "select from here to there" has to be reconstructed from their
//! coordinates. This module does that reconstruction and keeps the result in
//! *character* indices, matching how PDFium addresses the extracted text.

use pdfium_render::prelude::*;
use quark_core::geom::{Rect, Vec2};
use quark_core::search::{SearchHit, SearchOptions, context_for, find_matches};

use crate::doc::Document;
use crate::engine::EngineError;

/// One character's position on the page, in page space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharBox {
    pub index: usize,
    pub rect: Rect,
}

/// A page's extracted text plus the geometry needed to select it.
#[derive(Debug, Clone, Default)]
pub struct PageText {
    pub page: usize,
    /// The page's text as characters, in reading order.
    pub text: String,
    /// One entry per character in `text`.
    pub chars: Vec<CharBox>,
}

impl PageText {
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// The character nearest a page-space point.
    ///
    /// Returns the *insertion* index, so clicking past the right-hand edge of a
    /// character selects the gap after it rather than the character itself —
    /// which is what makes click-and-drag selection feel correct at the end of
    /// a line.
    pub fn index_at(&self, p: Vec2) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for cb in &self.chars {
            // Distance to the character's box, zero when inside it.
            let dx = (cb.rect.min.x - p.x).max(0.0).max(p.x - cb.rect.max.x);
            let dy = (cb.rect.min.y - p.y).max(0.0).max(p.y - cb.rect.max.y);
            // Vertical distance dominates: a click in the margin belongs to the
            // nearest line, not to whichever character happens to be closest in
            // a straight line.
            let d = dy * 4.0 + dx;
            if best.map_or(true, |(bd, _)| d < bd) {
                let past_middle = p.x > cb.rect.center().x;
                best = Some((d, cb.index + usize::from(past_middle)));
            }
        }
        best.map(|(_, i)| i.min(self.chars.len()))
    }

    /// The rectangles covering characters `[start, end)`, merged per line.
    ///
    /// Merging matters: a 400-character selection drawn as 400 rectangles both
    /// looks wrong at the seams and costs 400 draw calls per frame.
    pub fn quads_for_range(&self, start: usize, end: usize) -> Vec<Rect> {
        let (start, end) = (start.min(end), start.max(end));
        let mut out: Vec<Rect> = Vec::new();
        for cb in self
            .chars
            .iter()
            .filter(|c| c.index >= start && c.index < end)
        {
            match out.last_mut() {
                // Same line if the vertical spans substantially overlap.
                Some(last) if lines_overlap(last, &cb.rect) => {
                    *last = last.union(&cb.rect);
                }
                _ => out.push(cb.rect),
            }
        }
        out
    }

    /// The substring covered by a character range.
    pub fn slice(&self, start: usize, end: usize) -> String {
        let (start, end) = (start.min(end), start.max(end));
        self.text
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect()
    }

    /// Expands a range to whole words, for double-click selection.
    pub fn expand_to_word(&self, index: usize) -> (usize, usize) {
        let chars: Vec<char> = self.text.chars().collect();
        if chars.is_empty() {
            return (0, 0);
        }
        let i = index.min(chars.len().saturating_sub(1));
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
        if !is_word(chars[i]) {
            return (i, i + 1);
        }
        let mut a = i;
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        let mut b = i;
        while b + 1 < chars.len() && is_word(chars[b + 1]) {
            b += 1;
        }
        (a, b + 1)
    }

    /// Expands a range to the whole line, for triple-click selection.
    pub fn expand_to_line(&self, index: usize) -> (usize, usize) {
        let chars: Vec<char> = self.text.chars().collect();
        if chars.is_empty() {
            return (0, 0);
        }
        let i = index.min(chars.len().saturating_sub(1));
        let mut a = i;
        while a > 0 && chars[a - 1] != '\n' && chars[a - 1] != '\r' {
            a -= 1;
        }
        // Looks at the *next* character, so the line break itself is excluded
        // from the selection. Including it means a triple-click copies a
        // trailing newline that was never visible.
        let mut b = i;
        while b + 1 < chars.len() && chars[b + 1] != '\n' && chars[b + 1] != '\r' {
            b += 1;
        }
        (a, b + 1)
    }
}

/// Whether two character boxes sit on the same text line.
fn lines_overlap(a: &Rect, b: &Rect) -> bool {
    let overlap = a.max.y.min(b.max.y) - a.min.y.max(b.min.y);
    let shorter = a.height().min(b.height()).max(0.1);
    overlap > shorter * 0.4
}

/// Extracts a page's text and per-character geometry.
pub fn extract_page(doc: &Document, page_index: usize) -> Result<PageText, EngineError> {
    let pages = doc.inner().pages();
    let page = pages
        .get(page_index as i32)
        .map_err(|_| EngineError::Pdfium(format!("no page {page_index}")))?;
    let text = page.text().map_err(EngineError::from)?;

    let all = text.all();
    let mut chars = Vec::with_capacity(all.chars().count());
    for (i, ch) in text.chars().iter().enumerate() {
        // A character with no glyph box (a soft hyphen, say) still occupies an
        // index in the extracted string, so it must occupy one here too or
        // every later index shifts.
        let r = ch
            .tight_bounds()
            .ok()
            .map(|b| {
                Rect::new(
                    Vec2::new(b.left().value, b.bottom().value),
                    Vec2::new(b.right().value, b.top().value),
                )
                .normalize()
            })
            .unwrap_or(Rect::ZERO);
        chars.push(CharBox { index: i, rect: r });
    }

    Ok(PageText {
        page: page_index,
        text: all,
        chars,
    })
}

/// Extracts plain text from every page, joined with form feeds.
pub fn extract_all(doc: &Document) -> Result<String, EngineError> {
    let mut out = String::new();
    for i in 0..doc.page_count() {
        if i > 0 {
            // A form feed is the conventional page break in extracted text and
            // survives a round trip through most tools.
            out.push('\u{c}');
        }
        let t = extract_page(doc, i)?;
        out.push_str(&t.text);
    }
    Ok(out)
}

/// Searches one page, returning hits with context.
pub fn search_page(
    doc: &Document,
    page: usize,
    query: &str,
    opts: &SearchOptions,
    doc_index: usize,
) -> Result<Vec<SearchHit>, EngineError> {
    let t = extract_page(doc, page)?;
    Ok(search_text(&t.text, page, query, opts, doc_index))
}

/// Searches already-extracted text. Split out so it is testable without a file.
pub fn search_text(
    text: &str,
    page: usize,
    query: &str,
    opts: &SearchOptions,
    doc_index: usize,
) -> Vec<SearchHit> {
    find_matches(text, query, opts)
        .into_iter()
        .map(|(char_index, char_len)| {
            let (context, highlight) = context_for(text, char_index, char_len, 40);
            SearchHit {
                page,
                char_index,
                char_len,
                context,
                highlight,
                doc: doc_index,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn doc() -> Document {
        Document::open(testutil::sample_pdf(), None).unwrap()
    }

    #[test]
    fn extracts_the_text_on_a_page() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        assert!(t.text.contains("Quark PDF Engine"), "got {:?}", t.text);
        assert!(!t.is_empty());
    }

    #[test]
    fn every_character_has_a_geometry_entry() {
        let _g = crate::testutil::pdfium_guard();
        // Any mismatch here desynchronises selection from the text.
        let t = extract_page(&doc(), 0).unwrap();
        assert_eq!(t.chars.len(), t.text.chars().count());
        for (i, c) in t.chars.iter().enumerate() {
            assert_eq!(c.index, i);
        }
    }

    #[test]
    fn character_boxes_are_positioned_on_the_page() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        // The heading sits at y=700 in a 792pt page.
        let first = t.chars.iter().find(|c| c.rect.area() > 0.0).unwrap();
        assert!(first.rect.min.y > 600.0, "unexpected y {:?}", first.rect);
        assert!(first.rect.min.x > 50.0);
    }

    #[test]
    fn extract_all_separates_pages() {
        let _g = crate::testutil::pdfium_guard();
        let all = extract_all(&doc()).unwrap();
        assert!(all.contains("Quark PDF Engine"));
        assert!(all.contains("Second page heading"));
        assert_eq!(all.matches('\u{c}').count(), 1, "expected one page break");
    }

    #[test]
    fn clicking_on_a_character_finds_its_index() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        let target = &t.chars[3];
        // Click the left half, so the insertion point lands before it.
        let p = Vec2::new(target.rect.min.x + 0.5, target.rect.center().y);
        assert_eq!(t.index_at(p), Some(3));
    }

    #[test]
    fn clicking_past_a_character_selects_the_gap_after_it() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        let target = &t.chars[3];
        let p = Vec2::new(target.rect.max.x - 0.2, target.rect.center().y);
        assert_eq!(t.index_at(p), Some(4));
    }

    #[test]
    fn selection_quads_merge_into_one_rect_per_line() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        // "Quark" — five characters, all on the heading line.
        let q = t.quads_for_range(0, 5);
        assert_eq!(q.len(), 1, "expected one merged quad, got {}", q.len());
        assert!(q[0].width() > 20.0);
    }

    #[test]
    fn a_multi_line_selection_produces_one_quad_per_line() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        // The whole page spans the heading and the body line.
        let q = t.quads_for_range(0, t.char_count());
        assert!(q.len() >= 2, "expected several lines, got {}", q.len());
    }

    #[test]
    fn quads_for_a_reversed_range_are_the_same_as_forwards() {
        let _g = crate::testutil::pdfium_guard();
        // Dragging a selection right-to-left must not produce nothing.
        let t = extract_page(&doc(), 0).unwrap();
        assert_eq!(t.quads_for_range(10, 2), t.quads_for_range(2, 10));
    }

    #[test]
    fn slice_returns_the_selected_substring() {
        let _g = crate::testutil::pdfium_guard();
        let t = extract_page(&doc(), 0).unwrap();
        assert_eq!(t.slice(0, 5), "Quark");
        assert_eq!(t.slice(5, 0), "Quark", "reversed range still works");
    }

    #[test]
    fn double_click_expands_to_the_whole_word() {
        let _g = crate::testutil::pdfium_guard();
        let t = PageText {
            page: 0,
            text: "alpha beta gamma".into(),
            chars: Vec::new(),
        };
        assert_eq!(t.expand_to_word(7), (6, 10)); // inside "beta"
        assert_eq!(t.slice(6, 10), "beta");
        // Landing on the space selects just the space.
        assert_eq!(t.expand_to_word(5), (5, 6));
    }

    #[test]
    fn triple_click_expands_to_the_whole_line() {
        let _g = crate::testutil::pdfium_guard();
        let t = PageText {
            page: 0,
            text: "first line\nsecond line".into(),
            chars: Vec::new(),
        };
        let (a, b) = t.expand_to_line(3);
        assert_eq!(t.slice(a, b), "first line");
    }

    #[test]
    fn word_expansion_on_empty_text_is_harmless() {
        let _g = crate::testutil::pdfium_guard();
        let t = PageText::default();
        assert_eq!(t.expand_to_word(5), (0, 0));
        assert_eq!(t.expand_to_line(5), (0, 0));
    }

    #[test]
    fn search_finds_a_token_on_the_right_page() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let hits = search_page(&d, 1, "findable-token", &SearchOptions::default(), 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        assert!(hits[0].context.contains("findable-token"));
        let (a, b) = hits[0].highlight;
        assert_eq!(&hits[0].context[a..b], "findable-token");
    }

    #[test]
    fn search_finds_every_repetition() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let hits = search_page(&d, 1, "repeated", &SearchOptions::default(), 0).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn search_reports_char_offsets_that_index_the_page_text() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let t = extract_page(&d, 1).unwrap();
        let hits = search_page(&d, 1, "findable", &SearchOptions::default(), 0).unwrap();
        let h = &hits[0];
        assert_eq!(t.slice(h.char_index, h.char_index + h.char_len), "findable");
    }

    #[test]
    fn a_query_that_is_absent_returns_no_hits() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let hits = search_page(&d, 0, "zzzznotpresent", &SearchOptions::default(), 0).unwrap();
        assert!(hits.is_empty());
    }
}
