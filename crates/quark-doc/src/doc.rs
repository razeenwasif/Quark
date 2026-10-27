//! A document: its blocks, and the comments and suggestions attached to them.

use serde::{Deserialize, Serialize};

use crate::block::{Block, BlockKind, Style};

/// A place between two characters: which block, and how many `char`s in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pos {
    pub block: usize,
    pub offset: usize,
}

impl Pos {
    pub const fn new(block: usize, offset: usize) -> Pos {
        Pos { block, offset }
    }
}

/// What is selected: from where the drag started to where it is now. The two
/// are the same for a plain caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub fn caret(at: Pos) -> Selection {
        Selection {
            anchor: at,
            head: at,
        }
    }

    pub fn new(anchor: Pos, head: Pos) -> Selection {
        Selection { anchor, head }
    }

    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }

    /// The selection's two ends, earlier first.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn start(&self) -> Pos {
        self.ordered().0
    }

    pub fn end(&self) -> Pos {
        self.ordered().1
    }
}

/// Who wrote a note on a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Author {
    You,
    Claude,
}

/// One message in a comment's thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub author: Author,
    pub text: String,
    /// RFC 3339.
    pub at: String,
}

impl Note {
    pub fn now(author: Author, text: impl Into<String>) -> Note {
        Note {
            author,
            text: text.into(),
            at: chrono::Local::now().to_rfc3339(),
        }
    }
}

/// A conversation about a phrase in the document.
///
/// Where the phrase is lives in the text itself, as an anchor on each of its
/// characters (see [`Style::anchors`]); this holds what is said about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u32,
    /// The text commented on, as it was when the comment was made — shown
    /// above the thread, and how the anchor is found again in a file that was
    /// edited somewhere else.
    pub quote: String,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub resolved: bool,
}

/// A change Claude proposes: the anchored text replaced by `markdown`.
///
/// Nothing in the document changes until it is accepted. Until then the
/// anchored text is shown struck through and the replacement beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: u32,
    pub markdown: String,
    /// Claude's one-line account of the change, if it gave one.
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Never empty: a document with nothing in it is one empty paragraph.
    pub blocks: Vec<Block>,
    pub comments: Vec<Comment>,
    pub suggestions: Vec<Suggestion>,
    next_id: u32,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl Document {
    pub fn new() -> Document {
        Document {
            blocks: vec![Block::new(BlockKind::Paragraph)],
            comments: Vec::new(),
            suggestions: Vec::new(),
            next_id: 1,
        }
    }

    pub fn from_blocks(blocks: Vec<Block>) -> Document {
        let mut d = Document::new();
        if !blocks.is_empty() {
            d.blocks = blocks;
        }
        d
    }

    /// An id for a new comment or suggestion, unused by any so far.
    pub fn fresh_id(&mut self) -> u32 {
        let used = self
            .comments
            .iter()
            .map(|c| c.id)
            .chain(self.suggestions.iter().map(|s| s.id))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(used + 1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn block(&self, i: usize) -> &Block {
        &self.blocks[i]
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.len() == 1
            && self.blocks[0].is_empty()
            && self.blocks[0].kind == BlockKind::Paragraph
    }

    pub fn start(&self) -> Pos {
        Pos::new(0, 0)
    }

    pub fn end(&self) -> Pos {
        let last = self.blocks.len() - 1;
        Pos::new(last, self.blocks[last].len())
    }

    /// The nearest real position to one that may have gone stale.
    pub fn clamp(&self, p: Pos) -> Pos {
        let block = p.block.min(self.blocks.len() - 1);
        Pos::new(block, p.offset.min(self.blocks[block].len()))
    }

    /// The text between two positions, blocks joined by newlines.
    pub fn text_between(&self, a: Pos, b: Pos) -> String {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        if a.block == b.block {
            return self.blocks[a.block].slice(a.offset..b.offset).to_string();
        }
        let mut out = String::new();
        let first = &self.blocks[a.block];
        out.push_str(first.slice(a.offset..first.len()));
        for i in a.block + 1..b.block {
            out.push('\n');
            out.push_str(self.blocks[i].text());
        }
        out.push('\n');
        out.push_str(self.blocks[b.block].slice(0..b.offset));
        out
    }

    /// The whole document as plain text.
    pub fn plain_text(&self) -> String {
        self.text_between(self.start(), self.end())
    }

    /// The blocks between two positions, cut to them: what a copy takes.
    pub fn fragment(&self, a: Pos, b: Pos) -> Vec<Block> {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        let mut out = Vec::new();
        for i in a.block..=b.block {
            let src = &self.blocks[i];
            let from = if i == a.block { a.offset } else { 0 };
            let to = if i == b.block { b.offset } else { src.len() };
            let mut block = Block::new(src.kind.clone());
            block.insert_styled(0, src.slice(from..to), &src.styles()[from..to]);
            out.push(block);
        }
        out
    }

    /// Removes everything between two positions, joining the blocks at either
    /// end. Returns where the cut closed up.
    pub fn delete(&mut self, a: Pos, b: Pos) -> Pos {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        if a.block == b.block {
            self.blocks[a.block].remove(a.offset..b.offset);
            return a;
        }
        let (tail, styles) = self.blocks[b.block].split_off(b.offset);
        let first = &mut self.blocks[a.block];
        first.remove(a.offset..first.len());
        first.append(&tail, &styles);
        // A selection that began at the very start of a block takes the kind
        // of the block it ended in, so deleting a heading and the paragraph
        // after it leaves a paragraph rather than an empty heading.
        if a.offset == 0 && first.len() == tail.chars().count() {
            let kind = self.blocks[b.block].kind.clone();
            self.blocks[a.block].set_kind(kind);
        }
        self.blocks.drain(a.block + 1..=b.block);
        a
    }

    /// Splits a block in two at `at`; the second half takes `kind`. Returns
    /// the start of the new block.
    pub fn split(&mut self, at: Pos, kind: BlockKind) -> Pos {
        let at = self.clamp(at);
        let (text, styles) = self.blocks[at.block].split_off(at.offset);
        let mut next = Block::new(kind);
        next.append(&text, &styles);
        self.blocks.insert(at.block + 1, next);
        Pos::new(at.block + 1, 0)
    }

    /// Joins block `i` onto the end of block `i - 1`. Returns the join.
    pub fn merge_up(&mut self, i: usize) -> Pos {
        if i == 0 || i >= self.blocks.len() {
            return self.clamp(Pos::new(i, 0));
        }
        let removed = self.blocks.remove(i);
        let prev = &mut self.blocks[i - 1];
        let join = Pos::new(i - 1, prev.len());
        if prev.kind == BlockKind::Rule {
            // Backspacing into a rule removes the rule.
            self.blocks[i - 1] = removed;
            return Pos::new(i - 1, 0);
        }
        prev.append(removed.text(), removed.styles());
        join
    }

    /// Puts blocks in at a position: the first joins the text before it, the
    /// last the text after it. Returns the end of what was put in.
    pub fn insert_fragment(&mut self, at: Pos, fragment: Vec<Block>) -> Pos {
        let at = self.writable(at);
        let mut fragment = fragment;
        match fragment.len() {
            0 => at,
            1 => {
                let piece = fragment.pop().unwrap();
                let block = &mut self.blocks[at.block];
                if block.is_empty() && block.kind == BlockKind::Paragraph {
                    block.set_kind(piece.kind.clone());
                }
                block.insert_styled(at.offset, piece.text(), piece.styles());
                Pos::new(at.block, at.offset + piece.len())
            }
            n => {
                let (tail, tail_styles) = self.blocks[at.block].split_off(at.offset);
                let mut rest = fragment.split_off(1);
                let first = fragment.pop().unwrap();
                {
                    let head = &mut self.blocks[at.block];
                    if head.is_empty() {
                        head.set_kind(first.kind.clone());
                    }
                    head.append(first.text(), first.styles());
                }
                let mut last = rest.pop().unwrap();
                let end = Pos::new(at.block + n - 1, last.len());
                last.append(&tail, &tail_styles);
                let mut index = at.block + 1;
                for b in rest {
                    self.blocks.insert(index, b);
                    index += 1;
                }
                self.blocks.insert(index, last);
                end
            }
        }
    }

    /// Where text meant for `at` can go. A rule holds none, so text aimed at
    /// one goes into a new paragraph after it.
    pub fn writable(&mut self, at: Pos) -> Pos {
        let at = self.clamp(at);
        if self.blocks[at.block].kind != BlockKind::Rule {
            return at;
        }
        self.blocks
            .insert(at.block + 1, Block::new(BlockKind::Paragraph));
        Pos::new(at.block + 1, 0)
    }

    /// Changes the style of everything between two positions.
    pub fn restyle(&mut self, a: Pos, b: Pos, mut f: impl FnMut(&mut Style)) {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        for i in a.block..=b.block {
            let len = self.blocks[i].len();
            let from = if i == a.block { a.offset } else { 0 };
            let to = if i == b.block { b.offset } else { len };
            self.blocks[i].restyle(from..to, &mut f);
        }
    }

    /// Every character's style between two positions.
    pub fn styles_between(&self, a: Pos, b: Pos) -> impl Iterator<Item = &Style> {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        (a.block..=b.block).flat_map(move |i| {
            let block = &self.blocks[i];
            let from = if i == a.block { a.offset } else { 0 };
            let to = if i == b.block { b.offset } else { block.len() };
            block.styles()[from..to].iter()
        })
    }

    /// Where an anchor's text is now: its first and last character. `None`
    /// once all of it has been deleted.
    pub fn anchor_range(&self, id: u32) -> Option<(Pos, Pos)> {
        let mut first = None;
        let mut last = None;
        for (bi, block) in self.blocks.iter().enumerate() {
            for (ci, s) in block.styles().iter().enumerate() {
                if s.has_anchor(id) {
                    if first.is_none() {
                        first = Some(Pos::new(bi, ci));
                    }
                    last = Some(Pos::new(bi, ci + 1));
                }
            }
        }
        first.zip(last)
    }

    /// Removes an anchor from every character carrying it.
    pub fn clear_anchor(&mut self, id: u32) {
        for block in &mut self.blocks {
            if block.styles().iter().any(|s| s.has_anchor(id)) {
                let len = block.len();
                block.restyle(0..len, |s| s.remove_anchor(id));
            }
        }
    }

    pub fn comment(&self, id: u32) -> Option<&Comment> {
        self.comments.iter().find(|c| c.id == id)
    }

    pub fn comment_mut(&mut self, id: u32) -> Option<&mut Comment> {
        self.comments.iter_mut().find(|c| c.id == id)
    }

    pub fn suggestion(&self, id: u32) -> Option<&Suggestion> {
        self.suggestions.iter().find(|s| s.id == id)
    }

    /// Deletes a comment and its anchor.
    pub fn remove_comment(&mut self, id: u32) {
        self.comments.retain(|c| c.id != id);
        self.clear_anchor(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Marks;

    fn doc(lines: &[&str]) -> Document {
        Document::from_blocks(lines.iter().map(|l| Block::paragraph(l)).collect())
    }

    fn texts(d: &Document) -> Vec<&str> {
        d.blocks.iter().map(|b| b.text()).collect()
    }

    #[test]
    fn deleting_across_blocks_joins_the_ends() {
        let mut d = doc(&["first line", "middle", "last line"]);
        let at = d.delete(Pos::new(0, 5), Pos::new(2, 4));
        assert_eq!(at, Pos::new(0, 5));
        assert_eq!(texts(&d), vec!["first line"]);
    }

    #[test]
    fn deleting_a_whole_heading_leaves_the_paragraph_kind() {
        let mut d = doc(&["Title", "body"]);
        d.blocks[0].set_kind(BlockKind::Heading(1));
        d.delete(Pos::new(0, 0), Pos::new(1, 0));
        assert_eq!(d.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(texts(&d), vec!["body"]);
    }

    #[test]
    fn a_fragment_of_several_blocks_splits_its_target() {
        let mut d = doc(&["abcdef"]);
        let frag = vec![
            Block::paragraph("1"),
            Block::with_text(BlockKind::Heading(2), "2"),
            Block::paragraph("3"),
        ];
        let end = d.insert_fragment(Pos::new(0, 3), frag);
        assert_eq!(texts(&d), vec!["abc1", "2", "3def"]);
        assert_eq!(d.blocks[1].kind, BlockKind::Heading(2));
        assert_eq!(end, Pos::new(2, 1));
    }

    #[test]
    fn a_fragment_copied_out_goes_back_in_the_same() {
        let mut d = doc(&["one two", "three four"]);
        d.restyle(Pos::new(0, 4), Pos::new(1, 5), |s| {
            s.marks.insert(Marks::BOLD)
        });
        let frag = d.fragment(Pos::new(0, 4), Pos::new(1, 5));
        assert_eq!(frag.len(), 2);
        assert_eq!(frag[0].text(), "two");
        assert!(
            frag[1]
                .styles()
                .iter()
                .all(|s| s.marks.contains(Marks::BOLD))
        );
    }

    #[test]
    fn anchors_follow_their_text() {
        let mut d = doc(&["the quick brown fox"]);
        d.restyle(Pos::new(0, 4), Pos::new(0, 9), |s| s.add_anchor(7));
        d.blocks[0].insert(0, "see ", &Style::plain());
        assert_eq!(d.anchor_range(7), Some((Pos::new(0, 8), Pos::new(0, 13))));
        d.delete(Pos::new(0, 8), Pos::new(0, 13));
        assert_eq!(d.anchor_range(7), None);
    }

    #[test]
    fn merging_into_a_rule_replaces_the_rule() {
        let mut d = doc(&["above", "", "below"]);
        d.blocks[1].set_kind(BlockKind::Rule);
        let at = d.merge_up(2);
        assert_eq!(texts(&d), vec!["above", "below"]);
        assert_eq!(at, Pos::new(1, 0));
    }

    #[test]
    fn fresh_ids_never_repeat_a_loaded_one() {
        let mut d = Document::new();
        d.comments.push(Comment {
            id: 9,
            quote: String::new(),
            notes: vec![],
            resolved: false,
        });
        assert_eq!(d.fresh_id(), 10);
        assert_eq!(d.fresh_id(), 11);
    }
}
