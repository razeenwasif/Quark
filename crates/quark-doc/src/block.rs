//! A block — a paragraph, a heading, a list item — and the styled text in it.
//!
//! # Why a style per character
//!
//! Every character carries its own [`Style`]. Runs are what renderers and
//! Markdown want, and they are derived on demand; what editing wants is that
//! inserting, deleting and restyling never has to split, merge or re-normalise
//! anything. Documents are written by people, so a block is rarely more than a
//! few hundred characters, and a `Style` with no link and no anchors is a byte
//! and two empty pointers.
//!
//! Offsets are in `char`s throughout, the unit egui's text cursor counts in.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Inline formatting that is simply on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Marks(u8);

impl Marks {
    pub const NONE: Marks = Marks(0);
    pub const BOLD: Marks = Marks(1);
    pub const ITALIC: Marks = Marks(1 << 1);
    pub const UNDERLINE: Marks = Marks(1 << 2);
    pub const STRIKE: Marks = Marks(1 << 3);
    pub const CODE: Marks = Marks(1 << 4);
    /// Every mark at once.
    pub const EVERY: Marks = Marks(0x1f);

    /// Every mark, in the order Markdown nests them: outermost first.
    pub const ALL: [Marks; 5] = [
        Marks::BOLD,
        Marks::ITALIC,
        Marks::STRIKE,
        Marks::UNDERLINE,
        Marks::CODE,
    ];

    pub fn contains(self, other: Marks) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub fn insert(&mut self, other: Marks) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Marks) {
        self.0 &= !other.0;
    }

    pub fn with(mut self, other: Marks, on: bool) -> Marks {
        if on {
            self.insert(other);
        } else {
            self.remove(other);
        }
        self
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// How one character is drawn, and what is attached to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Style {
    pub marks: Marks,
    pub link: Option<Arc<str>>,
    /// Comments and suggestions this character belongs to, by id, sorted.
    ///
    /// An anchor is a property of the text rather than a pair of positions
    /// beside it, so it moves with every edit for free: typing inside a
    /// commented phrase extends the comment, and deleting the phrase leaves
    /// the comment with nothing to point at, which is what it should do.
    pub anchors: Vec<u32>,
}

impl Style {
    pub fn plain() -> Style {
        Style::default()
    }

    pub fn has_anchor(&self, id: u32) -> bool {
        self.anchors.binary_search(&id).is_ok()
    }

    pub fn add_anchor(&mut self, id: u32) {
        if let Err(i) = self.anchors.binary_search(&id) {
            self.anchors.insert(i, id);
        }
    }

    pub fn remove_anchor(&mut self, id: u32) {
        if let Ok(i) = self.anchors.binary_search(&id) {
            self.anchors.remove(i);
        }
    }
}

/// What a block is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Paragraph,
    /// Level 1 to 6. The editor offers 1 to 3 plus the title; Markdown read
    /// from elsewhere may use the rest, and they are kept.
    Heading(u8),
    Bullet {
        depth: u8,
    },
    Numbered {
        depth: u8,
    },
    Task {
        checked: bool,
        depth: u8,
    },
    Quote,
    Code {
        lang: String,
    },
    /// A horizontal rule. It has no text.
    Rule,
}

impl BlockKind {
    /// The nesting depth of a list item, or `None` for anything else.
    pub fn depth(&self) -> Option<u8> {
        match self {
            BlockKind::Bullet { depth }
            | BlockKind::Numbered { depth }
            | BlockKind::Task { depth, .. } => Some(*depth),
            _ => None,
        }
    }

    pub fn is_list(&self) -> bool {
        self.depth().is_some()
    }

    /// The same kind of list item at another depth.
    pub fn at_depth(&self, depth: u8) -> BlockKind {
        match self {
            BlockKind::Bullet { .. } => BlockKind::Bullet { depth },
            BlockKind::Numbered { .. } => BlockKind::Numbered { depth },
            BlockKind::Task { checked, .. } => BlockKind::Task {
                checked: *checked,
                depth,
            },
            other => other.clone(),
        }
    }

    /// Whether text in this block takes inline formatting. Code is verbatim.
    pub fn takes_marks(&self) -> bool {
        !matches!(self, BlockKind::Code { .. } | BlockKind::Rule)
    }

    /// The kind the block after this one gets when Return splits it.
    pub fn continued(&self) -> BlockKind {
        match self {
            BlockKind::Task { depth, .. } => BlockKind::Task {
                checked: false,
                depth: *depth,
            },
            BlockKind::Bullet { .. } | BlockKind::Numbered { .. } | BlockKind::Quote => {
                self.clone()
            }
            _ => BlockKind::Paragraph,
        }
    }

    /// The same kind with the parts a user does not pick by name — depth,
    /// checked state, language — set aside, for "is this block a bullet?"
    pub fn same_family(&self, other: &BlockKind) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
            && match (self, other) {
                (BlockKind::Heading(a), BlockKind::Heading(b)) => a == b,
                _ => true,
            }
    }
}

/// Every change to any block anywhere takes a new stamp, so a stamp names
/// one exact content and a layout cached against it can never be stale — not
/// even after an undo brings an older block back, because that block brings
/// its old stamp with it.
static STAMPS: AtomicU64 = AtomicU64::new(1);

fn next_stamp() -> u64 {
    STAMPS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    text: String,
    /// One per `char` of `text`.
    styles: Vec<Style>,
    stamp: u64,
}

impl Block {
    pub fn new(kind: BlockKind) -> Block {
        Block {
            kind,
            text: String::new(),
            styles: Vec::new(),
            stamp: next_stamp(),
        }
    }

    pub fn with_text(kind: BlockKind, text: &str) -> Block {
        let mut b = Block::new(kind);
        b.insert(0, text, &Style::plain());
        b
    }

    pub fn paragraph(text: &str) -> Block {
        Block::with_text(BlockKind::Paragraph, text)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn styles(&self) -> &[Style] {
        &self.styles
    }

    /// Length in `char`s.
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    /// Marks this block changed, for anything caching what it looks like.
    pub fn touch(&mut self) {
        self.stamp = next_stamp();
    }

    pub fn set_kind(&mut self, kind: BlockKind) {
        if self.kind != kind {
            if !kind.takes_marks() {
                for s in &mut self.styles {
                    s.marks = Marks::NONE;
                    s.link = None;
                }
            }
            if matches!(kind, BlockKind::Heading(_)) && self.text.contains('\n') {
                // Same length, so the styles still line up.
                self.text = self.text.replace('\n', " ");
            }
            self.kind = kind;
            self.touch();
        }
    }

    /// A heading is one line; a break typed or pasted into one is a space.
    fn fit<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if matches!(self.kind, BlockKind::Heading(_)) && text.contains('\n') {
            text.replace('\n', " ").into()
        } else {
            text.into()
        }
    }

    /// The byte index of a char offset.
    pub fn byte(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .nth(offset)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// The text of a char range.
    pub fn slice(&self, range: Range<usize>) -> &str {
        let (a, b) = (self.byte(range.start), self.byte(range.end));
        &self.text[a..b]
    }

    pub fn style_at(&self, offset: usize) -> Option<&Style> {
        self.styles.get(offset)
    }

    pub fn insert(&mut self, at: usize, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        let at = at.min(self.len());
        let byte = self.byte(at);
        let text = self.fit(text);
        self.text.insert_str(byte, &text);
        let mut style = style.clone();
        if !self.kind.takes_marks() {
            style.marks = Marks::NONE;
            style.link = None;
        }
        let n = text.chars().count();
        self.styles.splice(at..at, std::iter::repeat_n(style, n));
        self.touch();
    }

    /// Inserts already-styled characters, as cut from another block.
    pub fn insert_styled(&mut self, at: usize, text: &str, styles: &[Style]) {
        debug_assert_eq!(text.chars().count(), styles.len());
        if text.is_empty() {
            return;
        }
        let at = at.min(self.len());
        let byte = self.byte(at);
        let text = self.fit(text);
        self.text.insert_str(byte, &text);
        let plain = !self.kind.takes_marks();
        self.styles.splice(
            at..at,
            styles.iter().cloned().map(|mut s| {
                if plain {
                    s.marks = Marks::NONE;
                    s.link = None;
                }
                s
            }),
        );
        self.touch();
    }

    pub fn remove(&mut self, range: Range<usize>) {
        let end = range.end.min(self.len());
        let start = range.start.min(end);
        if start == end {
            return;
        }
        let (a, b) = (self.byte(start), self.byte(end));
        self.text.replace_range(a..b, "");
        self.styles.drain(start..end);
        self.touch();
    }

    /// Cuts everything from `at` on and returns it as text and styles.
    pub fn split_off(&mut self, at: usize) -> (String, Vec<Style>) {
        let at = at.min(self.len());
        let byte = self.byte(at);
        let text = self.text.split_off(byte);
        let styles = self.styles.split_off(at);
        self.touch();
        (text, styles)
    }

    pub fn append(&mut self, text: &str, styles: &[Style]) {
        let at = self.len();
        self.insert_styled(at, text, styles);
    }

    /// Changes the style of a range in place.
    pub fn restyle(&mut self, range: Range<usize>, mut f: impl FnMut(&mut Style)) {
        let end = range.end.min(self.len());
        let start = range.start.min(end);
        if start == end {
            return;
        }
        let takes = self.kind.takes_marks();
        for s in &mut self.styles[start..end] {
            f(s);
            if !takes {
                s.marks = Marks::NONE;
                s.link = None;
            }
        }
        self.touch();
    }

    /// Maximal runs of identically styled text: (char range, text, style).
    pub fn runs(&self) -> Vec<(Range<usize>, &str, &Style)> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut start_byte = 0;
        let mut chars = self.text.char_indices().enumerate().peekable();
        while let Some((i, (_, _))) = chars.next() {
            let next = chars.peek().map(|&(j, (b, _))| (j, b));
            let boundary = match next {
                None => true,
                Some((j, _)) => self.styles[j] != self.styles[i],
            };
            if boundary {
                let end_byte = next.map(|(_, b)| b).unwrap_or(self.text.len());
                out.push((
                    start..i + 1,
                    &self.text[start_byte..end_byte],
                    &self.styles[i],
                ));
                start = i + 1;
                start_byte = end_byte;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> Style {
        Style {
            marks: Marks::BOLD,
            ..Style::default()
        }
    }

    #[test]
    fn inserting_counts_characters_not_bytes() {
        let mut b = Block::paragraph("naïve café");
        assert_eq!(b.len(), 10);
        b.insert(5, "—", &Style::plain());
        assert_eq!(b.text(), "naïve— café");
        assert_eq!(b.styles().len(), 11);
    }

    #[test]
    fn runs_split_where_the_style_changes() {
        let mut b = Block::paragraph("one two three");
        b.restyle(4..7, |s| s.marks.insert(Marks::BOLD));
        let runs: Vec<_> = b
            .runs()
            .into_iter()
            .map(|(r, t, s)| (r, t.to_string(), s.marks))
            .collect();
        assert_eq!(
            runs,
            vec![
                (0..4, "one ".to_string(), Marks::NONE),
                (4..7, "two".to_string(), Marks::BOLD),
                (7..13, " three".to_string(), Marks::NONE),
            ]
        );
    }

    #[test]
    fn code_blocks_drop_inline_formatting() {
        let mut b = Block::new(BlockKind::Code {
            lang: "rust".into(),
        });
        b.insert(0, "fn main()", &bold());
        assert!(b.styles().iter().all(|s| s.marks.is_empty()));
    }

    #[test]
    fn headings_hold_one_line() {
        let mut b = Block::paragraph("one\ntwo");
        b.set_kind(BlockKind::Heading(1));
        assert_eq!(b.text(), "one two");
        b.insert(3, "\n", &Style::plain());
        assert_eq!(b.text(), "one  two");
    }

    #[test]
    fn every_change_takes_a_new_stamp() {
        let mut b = Block::paragraph("a");
        let first = b.stamp();
        b.insert(1, "b", &Style::plain());
        assert_ne!(b.stamp(), first);
        let copy = b.clone();
        assert_eq!(copy.stamp(), b.stamp());
    }

    #[test]
    fn split_and_append_round_trip() {
        let mut b = Block::paragraph("hello world");
        b.restyle(6..11, |s| s.marks.insert(Marks::ITALIC));
        let (tail, styles) = b.split_off(5);
        assert_eq!(b.text(), "hello");
        assert_eq!(tail, " world");
        b.append(&tail, &styles);
        assert_eq!(b.text(), "hello world");
        assert!(b.style_at(7).unwrap().marks.contains(Marks::ITALIC));
    }

    #[test]
    fn anchors_stay_sorted_and_unique() {
        let mut s = Style::plain();
        s.add_anchor(5);
        s.add_anchor(2);
        s.add_anchor(5);
        assert_eq!(s.anchors, vec![2, 5]);
        s.remove_anchor(2);
        assert_eq!(s.anchors, vec![5]);
    }
}
