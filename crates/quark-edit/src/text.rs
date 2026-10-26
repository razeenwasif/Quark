//! What a page says, and where: its text in the order it is drawn, each
//! character's box in the page's own space beside it — to read and search,
//! and to mark a passage found by its words. The text itself is PDFium's
//! (`quark-pdf`), which reads a page better than any lopdf walk; what is here
//! is what is done with it.

use lopdf::Document;

/// A page's text, and where each piece of it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    /// From one.
    pub page: u32,
    pub text: String,
    /// Each drawn character's byte range in `text`, and its box,
    /// `[x1, y1, x2, y2]` in the page's points from its bottom left.
    pub boxes: Vec<(std::ops::Range<usize>, [f32; 4])>,
}

/// A page's crop box, `[x1, y1, x2, y2]` in its own points, and how far it
/// is turned to be shown.
pub fn page_box(doc: &Document, page: u32) -> Result<([f32; 4], i64), String> {
    let id = *crate::pages::ids(doc, &[page])?.first().ok_or("no page")?;
    Ok(crate::ocr::shown_as(doc, id))
}

/// A bookmark: how deep it is, from one, what it says, and its page.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub level: usize,
    pub title: String,
    pub page: usize,
}

/// The document's bookmarks, in order; none when it has none, or none that
/// can be read.
pub fn outline(doc: &Document) -> Vec<Bookmark> {
    crate::guarded("the bookmarks", || {
        Ok(doc.get_toc().map(|t| t.toc).unwrap_or_default())
    })
    .unwrap_or_default()
    .into_iter()
    .map(|t| Bookmark {
        level: t.level,
        title: t.title,
        page: t.page,
    })
    .collect()
}

/// Where `query` is in a page's text, its case and spacing ignored: the
/// byte ranges of each time it is.
pub fn find(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    // Both folded the same way — lower case, each run of space one space —
    // with where each folded character came from.
    let fold = |s: &str| -> (Vec<char>, Vec<std::ops::Range<usize>>) {
        let (mut chars, mut from) = (Vec::new(), Vec::new());
        let mut blank = false;
        for (at, c) in s.char_indices() {
            if c.is_whitespace() {
                if !blank && !chars.is_empty() {
                    chars.push(' ');
                    from.push(at..at + c.len_utf8());
                }
                blank = true;
                continue;
            }
            blank = false;
            for low in c.to_lowercase() {
                chars.push(low);
                from.push(at..at + c.len_utf8());
            }
        }
        (chars, from)
    };
    let (hay, from) = fold(text);
    let (needle, _) = fold(query.trim());
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            found.push(from[i].start..from[i + needle.len() - 1].end);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

/// The boxes over a stretch of a page's text: one per line it runs over,
/// as a highlight's are.
pub fn boxes(page: &PageText, range: &std::ops::Range<usize>) -> Vec<[f32; 4]> {
    let mut lines: Vec<[f32; 4]> = Vec::new();
    for (_, b) in page
        .boxes
        .iter()
        .filter(|(r, _)| r.start < range.end && r.end > range.start)
    {
        match lines.last_mut() {
            // On the same line: its middle within the line's height.
            Some(line) if ((b[1] + b[3]) / 2.0) > line[1] && ((b[1] + b[3]) / 2.0) < line[3] => {
                line[0] = line[0].min(b[0]);
                line[1] = line[1].min(b[1]);
                line[2] = line[2].max(b[2]);
                line[3] = line[3].max(b[3]);
            }
            _ => lines.push(*b),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two lines as PDFium reads them, each character boxed: the first at
    /// (72, 720), the second 18 points under it.
    fn two_lines() -> PageText {
        let text = "Quark reads this\nand then this".to_string();
        let mut boxes = Vec::new();
        let (mut x, mut y) = (72.0f32, 720.0f32);
        for (at, c) in text.char_indices() {
            if c == '\n' {
                x = 72.0;
                y -= 18.0;
                continue;
            }
            boxes.push((at..at + c.len_utf8(), [x, y - 3.0, x + 7.0, y + 10.0]));
            x += 7.0;
        }
        PageText {
            page: 1,
            text,
            boxes,
        }
    }

    #[test]
    fn a_passage_is_found_whatever_its_case_and_spacing_and_boxed_a_line_at_a_time() {
        let one = two_lines();
        let found = find(&one.text, "THIS   and");
        assert_eq!(found.len(), 1);
        assert_eq!(&one.text[found[0].clone()], "this\nand");
        let lines = boxes(&one, &found[0]);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0][1] > lines[1][3] - 4.0 && lines[0][0] > lines[1][0],
            "the second line is under the first, starting further left: {lines:?}"
        );
        assert_eq!(find(&one.text, "this").len(), 2);
        assert!(find(&one.text, "absent").is_empty() && find(&one.text, "  ").is_empty());
    }

    #[test]
    fn a_pages_box_and_the_bookmarks_are_read() {
        let doc = Document::load_mem(&crate::fixture::fixture(&["One", "Two"])).unwrap();
        assert_eq!(page_box(&doc, 2).unwrap(), ([0.0, 0.0, 595.0, 842.0], 0));
        assert!(page_box(&doc, 3).is_err());
        assert!(outline(&doc).is_empty());
        let marked = Document::load_mem(
            &crate::combine::combine(&[
                ("a.pdf".into(), crate::fixture::fixture(&["One"])),
                ("b.pdf".into(), crate::fixture::fixture(&["Two", "Three"])),
            ])
            .unwrap(),
        )
        .unwrap();
        let titles: Vec<(String, usize)> = outline(&marked)
            .into_iter()
            .map(|b| (b.title, b.page))
            .collect();
        assert_eq!(titles, [("a.pdf".to_string(), 1), ("b.pdf".to_string(), 2)]);
    }
}
