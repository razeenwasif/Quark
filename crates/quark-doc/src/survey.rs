//! What can be said about a document as a whole: its outline, its length,
//! and where a phrase occurs in it.

use crate::block::BlockKind;
use crate::doc::{Document, Pos};
use crate::motion;

/// A heading, for the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub block: usize,
    pub level: u8,
    pub text: String,
}

pub fn outline(doc: &Document) -> Vec<Heading> {
    doc.blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match b.kind {
            BlockKind::Heading(level) if !b.text().trim().is_empty() => Some(Heading {
                block: i,
                level,
                text: b.text().replace('\n', " "),
            }),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub words: usize,
    pub characters: usize,
    /// At 230 words a minute, rounded up; zero only for an empty document.
    pub reading_minutes: usize,
}

pub fn stats(doc: &Document) -> Stats {
    let words: usize = doc
        .blocks
        .iter()
        .map(|b| motion::word_count(b.text()))
        .sum();
    let characters = doc.blocks.iter().map(|b| b.len()).sum();
    Stats {
        words,
        characters,
        reading_minutes: words.div_ceil(230),
    }
}

/// Every place `query` occurs, within single blocks, in order.
pub fn find(doc: &Document, query: &str, match_case: bool) -> Vec<(Pos, Pos)> {
    if query.is_empty() {
        return Vec::new();
    }
    // Folded one char to one char, so offsets in the folded text are offsets
    // in the real one. Full Unicode folding changes lengths (ß → ss), which
    // would put every hit after it in the wrong place.
    let fold = |c: char| {
        if match_case {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut out = Vec::new();
    for (bi, block) in doc.blocks.iter().enumerate() {
        let hay: Vec<char> = block.text().chars().map(fold).collect();
        let mut i = 0;
        while i + needle.len() <= hay.len() {
            if hay[i..i + needle.len()] == needle[..] {
                out.push((Pos::new(bi, i), Pos::new(bi, i + needle.len())));
                i += needle.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;

    #[test]
    fn the_outline_is_the_headings() {
        let doc = markdown::read("# Plan\n\ntext\n\n## Goals\n\n##\n\n### Detail\n");
        let o = outline(&doc);
        assert_eq!(
            o.iter()
                .map(|h| (h.block, h.level, h.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, 1, "Plan"), (2, 2, "Goals"), (4, 3, "Detail")]
        );
    }

    #[test]
    fn words_and_reading_time() {
        let doc = markdown::read("one two three\n\n- four five\n");
        let s = stats(&doc);
        assert_eq!(s.words, 5);
        assert_eq!(s.reading_minutes, 1);
        assert_eq!(stats(&markdown::read("")).reading_minutes, 0);
    }

    #[test]
    fn find_ignores_case_unless_asked() {
        let doc = markdown::read("Quark quark QUARK\n\nquarks");
        assert_eq!(find(&doc, "quark", false).len(), 4);
        assert_eq!(
            find(&doc, "quark", true),
            vec![
                (Pos::new(0, 6), Pos::new(0, 11)),
                (Pos::new(1, 0), Pos::new(1, 5))
            ]
        );
    }
}
