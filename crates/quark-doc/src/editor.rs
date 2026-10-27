//! Editing: a document, the selection in it, and the history of both.
//!
//! Everything a key press, a toolbar button or a reply from Claude does to a
//! document is a method here, so all of it can be tested without a window. The
//! UI's part is to turn input into these calls and to say where a caret goes
//! when the move depends on how lines wrapped (up, down, the line's ends).

use crate::block::{Block, BlockKind, Marks, Style};
use crate::doc::{Author, Comment, Document, Note, Pos, Selection, Suggestion};
use crate::markdown;
use crate::motion;

/// How the next change joins the undo history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Characters typed in a row: undone a word at a time.
    Typing,
    /// Backspace or Delete held down: undone as one.
    Deleting,
    /// Everything else: one change, one step.
    Other,
}

/// What the document was before a change: enough to put it back.
#[derive(Debug, Clone)]
struct Snapshot {
    blocks: Vec<Block>,
    suggestions: Vec<Suggestion>,
    selection: Selection,
    state: u64,
}

/// Undo by snapshot. A document is a few hundred blocks at most, and cloning
/// them is far cheaper than getting an inverse right for every edit; it also
/// means undo can never put the text somewhere it never was.
#[derive(Debug, Default)]
struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    open: Option<Group>,
}

/// The most steps kept.
const HISTORY: usize = 500;

#[derive(Debug)]
pub struct Editor {
    pub doc: Document,
    selection: Selection,
    history: History,
    /// The style the next typed character takes, chosen while the caret had
    /// nothing selected — ⌘B, then typing. Dropped when the caret moves.
    pending: Option<Style>,
    /// Names the document's current content. Undo goes back to the state it
    /// restores, so undoing to what was saved makes the document clean again.
    state: u64,
    next_state: u64,
    saved: u64,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new(Document::new())
    }
}

impl Editor {
    pub fn new(doc: Document) -> Editor {
        Editor {
            doc,
            selection: Selection::default(),
            history: History::default(),
            pending: None,
            state: 0,
            next_state: 1,
            saved: 0,
        }
    }

    // --- state ---

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Moves the caret or selection. Typing after this starts a new undo step.
    pub fn set_selection(&mut self, sel: Selection) {
        let sel = Selection::new(self.doc.clamp(sel.anchor), self.doc.clamp(sel.head));
        if sel != self.selection {
            self.pending = None;
            self.history.open = None;
        }
        self.selection = sel;
    }

    pub fn caret_to(&mut self, at: Pos) {
        self.set_selection(Selection::caret(at));
    }

    /// The selection's head moved, its anchor kept: shift-click and shift-arrows.
    pub fn extend_to(&mut self, at: Pos) {
        let anchor = self.selection.anchor;
        self.set_selection(Selection::new(anchor, at));
    }

    /// Changes whenever the content does, and only then.
    pub fn state(&self) -> u64 {
        self.state
    }

    pub fn is_dirty(&self) -> bool {
        self.state != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.state;
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Ends the current undo step, so the next change starts its own.
    pub fn seal(&mut self) {
        self.history.open = None;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            blocks: self.doc.blocks.clone(),
            suggestions: self.doc.suggestions.clone(),
            selection: self.selection,
            state: self.state,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.doc.blocks = s.blocks;
        self.doc.suggestions = s.suggestions;
        self.selection = s.selection;
        self.state = s.state;
        self.pending = None;
        self.history.open = None;
    }

    /// Opens an undo step for a change about to be made, or folds the change
    /// into the step already open when it is more of the same.
    fn begin(&mut self, group: Group) {
        let joins = self.history.open == Some(group) && group != Group::Other;
        if !joins {
            let snap = self.snapshot();
            self.history.undo.push(snap);
            if self.history.undo.len() > HISTORY {
                self.history.undo.remove(0);
            }
            self.history.open = Some(group);
        }
        self.history.redo.clear();
    }

    /// Records that the content changed.
    fn changed(&mut self) {
        self.state = self.next_state;
        self.next_state += 1;
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.history.undo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.history.redo.push(now);
        self.restore(prev);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.history.redo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.history.undo.push(now);
        self.restore(next);
        true
    }

    // --- what is under the caret ---

    /// The style a character typed at `at` takes: the one before it, except
    /// that links and comments only continue when the caret is inside them,
    /// not at their edge, and a suggestion never grows.
    pub fn typing_style(&self, at: Pos) -> Style {
        if let Some(p) = &self.pending {
            return p.clone();
        }
        let at = self.doc.clamp(at);
        let block = &self.doc.blocks[at.block];
        let before = at.offset.checked_sub(1).and_then(|i| block.style_at(i));
        let after = block.style_at(at.offset);
        let mut s = before.or(after).cloned().unwrap_or_default();
        match (before, after) {
            (Some(b), Some(a)) => {
                s.anchors.retain(|id| a.has_anchor(*id));
                if b.link != a.link {
                    s.link = None;
                }
            }
            _ => {
                s.anchors.clear();
                s.link = None;
            }
        }
        let doc = &self.doc;
        s.anchors.retain(|id| doc.suggestion(*id).is_none());
        s
    }

    /// The marks the toolbar shows as on: those on every selected character,
    /// or those the next typed character would take.
    pub fn current_marks(&self) -> Marks {
        let (a, b) = self.selection.ordered();
        if a == b {
            return self.typing_style(a).marks;
        }
        let mut styles = self.doc.styles_between(a, b).peekable();
        if styles.peek().is_none() {
            return Marks::NONE;
        }
        let mut common = Marks::EVERY;
        for s in styles {
            for m in Marks::ALL {
                if !s.marks.contains(m) {
                    common.remove(m);
                }
            }
        }
        common
    }

    /// The kind of the block the caret is in.
    pub fn current_kind(&self) -> &BlockKind {
        &self.doc.blocks[self.selection.head.block].kind
    }

    /// The link around a position, if it is in one: its extent and address.
    pub fn link_at(&self, at: Pos) -> Option<(Pos, Pos, String)> {
        let block = &self.doc.blocks[at.block];
        let styles = block.styles();
        let probe = if at.offset < styles.len() && styles[at.offset].link.is_some() {
            at.offset
        } else if at.offset > 0 && styles[at.offset - 1].link.is_some() {
            at.offset - 1
        } else {
            return None;
        };
        let link = styles[probe].link.clone()?;
        let mut start = probe;
        while start > 0 && styles[start - 1].link.as_ref() == Some(&link) {
            start -= 1;
        }
        let mut end = probe + 1;
        while end < styles.len() && styles[end].link.as_ref() == Some(&link) {
            end += 1;
        }
        Some((
            Pos::new(at.block, start),
            Pos::new(at.block, end),
            link.to_string(),
        ))
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection.ordered();
        self.doc.text_between(a, b)
    }

    /// The selection as Markdown, for the clipboard and for Claude.
    pub fn selected_markdown(&self) -> String {
        let (a, b) = self.selection.ordered();
        markdown::write_blocks(&self.doc.fragment(a, b))
    }

    // --- selecting ---

    pub fn select_all(&mut self) {
        let (s, e) = (self.doc.start(), self.doc.end());
        self.set_selection(Selection::new(s, e));
    }

    pub fn select_word_at(&mut self, at: Pos) {
        let at = self.doc.clamp(at);
        let (s, e) = motion::word_at(self.doc.blocks[at.block].text(), at.offset);
        self.set_selection(Selection::new(Pos::new(at.block, s), Pos::new(at.block, e)));
    }

    pub fn select_block(&mut self, i: usize) {
        let i = i.min(self.doc.len() - 1);
        let len = self.doc.blocks[i].len();
        self.set_selection(Selection::new(Pos::new(i, 0), Pos::new(i, len)));
    }

    // --- moving, where it does not depend on layout ---

    /// One character or word left, across block boundaries.
    pub fn left_of(&self, at: Pos, word: bool) -> Pos {
        if at.offset == 0 {
            if at.block == 0 {
                return at;
            }
            return Pos::new(at.block - 1, self.doc.blocks[at.block - 1].len());
        }
        let text = self.doc.blocks[at.block].text();
        let offset = if word {
            motion::prev_word(text, at.offset)
        } else {
            motion::prev_char(text, at.offset)
        };
        Pos::new(at.block, offset)
    }

    pub fn right_of(&self, at: Pos, word: bool) -> Pos {
        let block = &self.doc.blocks[at.block];
        if at.offset >= block.len() {
            if at.block + 1 >= self.doc.len() {
                return at;
            }
            return Pos::new(at.block + 1, 0);
        }
        let offset = if word {
            motion::next_word(block.text(), at.offset)
        } else {
            motion::next_char(block.text(), at.offset)
        };
        Pos::new(at.block, offset)
    }

    /// ← and →, with or without ⌥ and ⇧. Without ⇧, a selection collapses
    /// to its near end rather than moving.
    pub fn move_horizontal(&mut self, right: bool, word: bool, extend: bool) {
        let head = self.selection.head;
        if !extend && !self.selection.is_caret() {
            let (a, b) = self.selection.ordered();
            self.caret_to(if right { b } else { a });
            return;
        }
        let to = if right {
            self.right_of(head, word)
        } else {
            self.left_of(head, word)
        };
        if extend {
            self.extend_to(to);
        } else {
            self.caret_to(to);
        }
    }

    // --- typing ---

    /// Types text at the caret, replacing any selection. A newline starts a
    /// new block, except in code, where it is part of the text.
    pub fn type_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        // A space after a word ends the undo step, so undo takes back a word
        // at a time rather than a whole paragraph.
        if text.starts_with(char::is_whitespace) && !self.selection.is_caret() {
            self.seal();
        } else if text.starts_with(char::is_whitespace) {
            let head = self.selection.head;
            let prev = head
                .offset
                .checked_sub(1)
                .and_then(|i| self.doc.blocks[head.block].text().chars().nth(i));
            if prev.is_some_and(|c| !c.is_whitespace()) {
                self.seal();
            }
        }
        self.begin(Group::Typing);
        let mut at = self.delete_selection_in_place();
        at = self.doc.writable(at);
        let style = self.typing_style(at);
        self.pending = None;

        let code = matches!(self.doc.blocks[at.block].kind, BlockKind::Code { .. });
        if code || !text.contains('\n') {
            self.doc.blocks[at.block].insert(at.offset, &text, &style);
            at.offset += text.chars().count();
        } else {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    let kind = self.doc.blocks[at.block].kind.continued();
                    at = self.doc.split(at, kind);
                }
                self.doc.blocks[at.block].insert(at.offset, line, &style);
                at.offset += line.chars().count();
            }
        }
        self.selection = Selection::caret(at);
        self.changed();
        if text == " " {
            self.autoformat();
        }
    }

    /// A line break inside the block: ⇧↩.
    pub fn line_break(&mut self) {
        self.begin(Group::Other);
        let at = self.delete_selection_in_place();
        let at = self.doc.writable(at);
        let style = self.typing_style(at);
        self.doc.blocks[at.block].insert(at.offset, "\n", &style);
        self.selection = Selection::caret(Pos::new(at.block, at.offset + 1));
        self.changed();
        self.seal();
    }

    /// The Markdown a person types out of habit, turned into what it means
    /// as soon as the space after it is typed: `# `, `- `, `1. `, `[ ] `, `> `.
    /// Its own undo step, so ⌘Z gives back the characters as typed.
    fn autoformat(&mut self) {
        let at = self.selection.head;
        let block = &self.doc.blocks[at.block];
        let before: String = block.text().chars().take(at.offset).collect();
        let kind = match (&block.kind, before.as_str()) {
            (BlockKind::Paragraph, "# ") => BlockKind::Heading(1),
            (BlockKind::Paragraph, "## ") => BlockKind::Heading(2),
            (BlockKind::Paragraph, "### ") => BlockKind::Heading(3),
            (BlockKind::Paragraph, "- " | "* ") => BlockKind::Bullet { depth: 0 },
            (BlockKind::Paragraph, "> ") => BlockKind::Quote,
            (BlockKind::Paragraph | BlockKind::Bullet { .. }, "[] " | "[ ] ") => BlockKind::Task {
                checked: false,
                depth: block.kind.depth().unwrap_or(0),
            },
            (BlockKind::Paragraph | BlockKind::Bullet { .. }, "[x] " | "[X] ") => BlockKind::Task {
                checked: true,
                depth: block.kind.depth().unwrap_or(0),
            },
            (BlockKind::Paragraph, s) if is_ordinal(s) => BlockKind::Numbered { depth: 0 },
            _ => return,
        };
        self.seal();
        self.begin(Group::Other);
        let block = &mut self.doc.blocks[at.block];
        block.remove(0..at.offset);
        block.set_kind(kind);
        self.selection = Selection::caret(Pos::new(at.block, 0));
        self.changed();
        self.seal();
    }

    /// Return.
    pub fn enter(&mut self) {
        self.seal();
        self.begin(Group::Other);
        let at = self.delete_selection_in_place();
        let block = &self.doc.blocks[at.block];
        let kind = block.kind.clone();
        let len = block.len();
        let text = block.text().to_string();

        let caret = match kind {
            BlockKind::Code { .. } => {
                // Return on an empty last line leaves the code block.
                if at.offset == len && text.ends_with('\n') {
                    self.doc.blocks[at.block].remove(len - 1..len);
                    self.doc
                        .split(Pos::new(at.block, len - 1), BlockKind::Paragraph)
                } else {
                    self.doc.blocks[at.block].insert(at.offset, "\n", &Style::plain());
                    Pos::new(at.block, at.offset + 1)
                }
            }
            // Return in an empty list item or quote steps out of it.
            ref k if (k.is_list() || *k == BlockKind::Quote) && len == 0 => {
                let next = match k.depth() {
                    Some(d) if d > 0 => k.at_depth(d - 1),
                    _ => BlockKind::Paragraph,
                };
                self.doc.blocks[at.block].set_kind(next);
                at
            }
            BlockKind::Paragraph
                if at.offset == len && text.starts_with("```") && !text[3..].contains('`') =>
            {
                let lang = text[3..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let block = &mut self.doc.blocks[at.block];
                block.remove(0..len);
                block.set_kind(BlockKind::Code { lang });
                Pos::new(at.block, 0)
            }
            BlockKind::Paragraph if at.offset == len && (text == "---" || text == "***") => {
                let block = &mut self.doc.blocks[at.block];
                block.remove(0..len);
                block.set_kind(BlockKind::Rule);
                self.doc
                    .blocks
                    .insert(at.block + 1, Block::new(BlockKind::Paragraph));
                Pos::new(at.block + 1, 0)
            }
            BlockKind::Rule => {
                self.doc
                    .blocks
                    .insert(at.block + 1, Block::new(BlockKind::Paragraph));
                Pos::new(at.block + 1, 0)
            }
            ref k => {
                if at.offset == 0 && len > 0 {
                    // At the start of a block, Return opens a line above it and
                    // the block itself stays as it is.
                    let above = if k.is_list() || *k == BlockKind::Quote {
                        k.continued()
                    } else {
                        BlockKind::Paragraph
                    };
                    self.doc.blocks.insert(at.block, Block::new(above));
                    Pos::new(at.block + 1, 0)
                } else {
                    let next = if at.offset == len {
                        k.continued()
                    } else if let BlockKind::Heading(_) = k {
                        k.clone()
                    } else {
                        k.continued()
                    };
                    self.doc.split(at, next)
                }
            }
        };
        self.selection = Selection::caret(caret);
        self.changed();
        self.seal();
    }

    /// Backspace, or ⌥⌫ for a word.
    pub fn backspace(&mut self, word: bool) {
        if !self.selection.is_caret() {
            self.delete_selection();
            return;
        }
        let at = self.selection.head;
        if at.offset == 0 {
            let kind = self.doc.blocks[at.block].kind.clone();
            self.seal();
            if kind != BlockKind::Paragraph && kind != BlockKind::Rule {
                // At the start of a list item, heading or quote, Backspace
                // takes off the formatting first, the way every word
                // processor does; only a plain paragraph joins the one above.
                self.begin(Group::Other);
                let next = match kind.depth() {
                    Some(d) if d > 0 => kind.at_depth(d - 1),
                    _ => BlockKind::Paragraph,
                };
                self.doc.blocks[at.block].set_kind(next);
                self.changed();
            } else if at.block > 0 {
                self.begin(Group::Other);
                let join = self.doc.merge_up(at.block);
                self.selection = Selection::caret(join);
                self.changed();
            }
            self.seal();
            return;
        }
        self.begin(Group::Deleting);
        let text = self.doc.blocks[at.block].text();
        let from = if word {
            motion::prev_word(text, at.offset)
        } else {
            motion::prev_char(text, at.offset)
        };
        self.doc.blocks[at.block].remove(from..at.offset);
        self.selection = Selection::caret(Pos::new(at.block, from));
        self.changed();
    }

    /// Forward delete, or ⌥⌦ for a word.
    pub fn delete_forward(&mut self, word: bool) {
        if !self.selection.is_caret() {
            self.delete_selection();
            return;
        }
        let at = self.selection.head;
        let len = self.doc.blocks[at.block].len();
        if at.offset >= len {
            if at.block + 1 < self.doc.len() {
                self.seal();
                self.begin(Group::Other);
                if self.doc.blocks[at.block + 1].kind == BlockKind::Rule {
                    self.doc.blocks.remove(at.block + 1);
                } else {
                    self.doc.merge_up(at.block + 1);
                }
                self.changed();
                self.seal();
            }
            return;
        }
        self.begin(Group::Deleting);
        let text = self.doc.blocks[at.block].text();
        let to = if word {
            motion::next_word(text, at.offset)
        } else {
            motion::next_char(text, at.offset)
        };
        self.doc.blocks[at.block].remove(at.offset..to);
        self.changed();
    }

    /// Deletes the selection as its own undo step.
    pub fn delete_selection(&mut self) {
        if self.selection.is_caret() {
            return;
        }
        self.seal();
        self.begin(Group::Other);
        self.delete_selection_in_place();
        self.changed();
        self.seal();
    }

    /// Deletes the selection inside an undo step already open; returns the
    /// caret.
    fn delete_selection_in_place(&mut self) -> Pos {
        let (a, b) = self.selection.ordered();
        if a == b {
            return a;
        }
        // A selection that took in a whole rule takes the rule with it.
        let at = self.doc.delete(a, b);
        self.selection = Selection::caret(at);
        at
    }

    // --- clipboard ---

    /// Pastes blocks — from Markdown on the clipboard, or Claude's.
    pub fn paste_blocks(&mut self, mut blocks: Vec<Block>) {
        if blocks.is_empty() {
            return;
        }
        // Comments and suggestions stay where they were made; a copy of
        // their text is just text.
        for b in &mut blocks {
            let len = b.len();
            b.restyle(0..len, |s| s.anchors.clear());
        }
        self.seal();
        self.begin(Group::Other);
        let at = self.delete_selection_in_place();
        let end = self.doc.insert_fragment(at, blocks);
        self.selection = Selection::caret(end);
        self.changed();
        self.seal();
    }

    /// Pastes Markdown, which plain text also is.
    pub fn paste_markdown(&mut self, src: &str) {
        let blocks = markdown::read_blocks(src);
        // A single line pasted into a block keeps that block's kind and the
        // style around the caret, rather than arriving as a paragraph of its
        // own with formatting nobody asked for.
        if let [only] = blocks.as_slice() {
            let plain = only
                .styles()
                .iter()
                .all(|s| s.marks.is_empty() && s.link.is_none());
            if only.kind == BlockKind::Paragraph && plain && !src.contains('\n') {
                let text = only.text().to_string();
                self.seal();
                self.type_text(&text);
                self.seal();
                return;
            }
        }
        self.paste_blocks(blocks);
    }

    // --- formatting ---

    /// Bold, italic and the rest: on for all of the selection, unless all of
    /// it has it already, in which case off.
    pub fn toggle_mark(&mut self, mark: Marks) {
        let (a, b) = self.selection.ordered();
        if a == b {
            let mut s = self.typing_style(a);
            let on = !s.marks.contains(mark);
            s.marks = s.marks.with(mark, on);
            self.pending = Some(s);
            return;
        }
        let on = !self
            .doc
            .styles_between(a, b)
            .all(|s| s.marks.contains(mark));
        self.seal();
        self.begin(Group::Other);
        self.doc.restyle(a, b, |s| s.marks = s.marks.with(mark, on));
        self.changed();
        self.seal();
    }

    /// Makes the selection a link, or with `None` takes the link off. With
    /// nothing selected, the link around the caret.
    pub fn set_link(&mut self, url: Option<&str>) {
        let (mut a, mut b) = self.selection.ordered();
        if a == b {
            match (url, self.link_at(a)) {
                (_, Some((s, e, _))) => {
                    a = s;
                    b = e;
                }
                (Some(url), None) => {
                    // Nothing to link: the address goes in as its own text.
                    self.seal();
                    self.begin(Group::Other);
                    let a = self.doc.writable(a);
                    let mut style = self.typing_style(a);
                    style.link = Some(url.into());
                    self.doc.blocks[a.block].insert(a.offset, url, &style);
                    self.selection =
                        Selection::caret(Pos::new(a.block, a.offset + url.chars().count()));
                    self.changed();
                    self.seal();
                    return;
                }
                (None, None) => return,
            }
        }
        self.seal();
        self.begin(Group::Other);
        let link: Option<std::sync::Arc<str>> = url.map(Into::into);
        self.doc.restyle(a, b, |s| s.link = link.clone());
        self.changed();
        self.seal();
    }

    pub fn clear_formatting(&mut self) {
        let (a, b) = self.selection.ordered();
        if a == b {
            self.pending = Some(Style {
                anchors: self.typing_style(a).anchors,
                ..Style::default()
            });
            return;
        }
        self.seal();
        self.begin(Group::Other);
        self.doc.restyle(a, b, |s| {
            s.marks = Marks::NONE;
            s.link = None;
        });
        let (first, last) = (a.block, b.block);
        for i in first..=last {
            self.doc.blocks[i].set_kind(BlockKind::Paragraph);
        }
        self.changed();
        self.seal();
    }

    /// Sets the kind of every block the selection touches.
    pub fn set_kind(&mut self, kind: BlockKind) {
        let (a, b) = self.selection.ordered();
        self.seal();
        self.begin(Group::Other);
        for i in a.block..=b.block {
            let block = &mut self.doc.blocks[i];
            if block.kind == BlockKind::Rule {
                continue;
            }
            // A list keeps its depth when it becomes another kind of list.
            let kind = match (block.kind.depth(), kind.depth()) {
                (Some(d), Some(_)) => kind.at_depth(d),
                _ => kind.clone(),
            };
            block.set_kind(kind);
        }
        self.changed();
        self.seal();
    }

    /// The list and quote buttons: the kind on, or off again if every
    /// selected block already has it.
    pub fn toggle_kind(&mut self, kind: BlockKind) {
        let (a, b) = self.selection.ordered();
        let all = (a.block..=b.block).all(|i| self.doc.blocks[i].kind.same_family(&kind));
        self.set_kind(if all { BlockKind::Paragraph } else { kind });
    }

    /// Tab and ⇧Tab in a list: deeper or shallower. Returns whether the
    /// selection was in a list at all, so Tab can mean something else outside.
    pub fn indent(&mut self, deeper: bool) -> bool {
        let (a, b) = self.selection.ordered();
        if !(a.block..=b.block).any(|i| self.doc.blocks[i].kind.is_list()) {
            return false;
        }
        self.seal();
        self.begin(Group::Other);
        for i in a.block..=b.block {
            let block = &mut self.doc.blocks[i];
            if let Some(d) = block.kind.depth() {
                let d = if deeper {
                    (d + 1).min(5)
                } else {
                    d.saturating_sub(1)
                };
                let kind = block.kind.at_depth(d);
                block.set_kind(kind);
            }
        }
        self.changed();
        self.seal();
        true
    }

    /// Ticks or unticks a checklist item.
    pub fn toggle_task(&mut self, block: usize) {
        if let Some(BlockKind::Task { checked, depth }) =
            self.doc.blocks.get(block).map(|b| b.kind.clone())
        {
            self.seal();
            self.begin(Group::Other);
            self.doc.blocks[block].set_kind(BlockKind::Task {
                checked: !checked,
                depth,
            });
            self.changed();
            self.seal();
        }
    }

    /// Puts a rule in after the caret's block.
    pub fn insert_rule(&mut self) {
        self.seal();
        self.begin(Group::Other);
        let at = self.delete_selection_in_place();
        let index = if self.doc.blocks[at.block].is_empty() {
            at.block
        } else {
            at.block + 1
        };
        if index == at.block {
            self.doc.blocks[at.block].set_kind(BlockKind::Rule);
        } else {
            self.doc.blocks.insert(index, Block::new(BlockKind::Rule));
        }
        if index + 1 >= self.doc.len() || self.doc.blocks[index + 1].kind == BlockKind::Rule {
            self.doc
                .blocks
                .insert(index + 1, Block::new(BlockKind::Paragraph));
        }
        self.selection = Selection::caret(Pos::new(index + 1, 0));
        self.changed();
        self.seal();
    }

    /// Replaces a range with plain text in the style of what was there: find
    /// and replace.
    pub fn replace(&mut self, a: Pos, b: Pos, text: &str) {
        let (a, b) = (self.doc.clamp(a.min(b)), self.doc.clamp(a.max(b)));
        let style = self.doc.blocks[a.block]
            .style_at(a.offset)
            .cloned()
            .unwrap_or_default();
        self.seal();
        self.begin(Group::Other);
        let at = self.doc.delete(a, b);
        let at = self.doc.writable(at);
        self.doc.blocks[at.block].insert(at.offset, text, &style);
        self.selection = Selection::caret(Pos::new(at.block, at.offset + text.chars().count()));
        self.changed();
        self.seal();
    }

    // --- comments ---

    /// Comments on the selection. `None` when nothing is selected.
    pub fn add_comment(&mut self, text: &str) -> Option<u32> {
        let (a, b) = self.selection.ordered();
        if a == b {
            return None;
        }
        let id = self.doc.fresh_id();
        let quote = self.doc.text_between(a, b);
        self.seal();
        self.begin(Group::Other);
        self.doc.restyle(a, b, |s| s.add_anchor(id));
        self.changed();
        self.seal();
        self.doc.comments.push(Comment {
            id,
            quote,
            notes: vec![Note::now(Author::You, text)],
            resolved: false,
        });
        Some(id)
    }

    // --- suggestions ---

    /// Claude proposes replacing a range with Markdown. Nothing changes until
    /// it is accepted.
    pub fn suggest(&mut self, a: Pos, b: Pos, markdown: String, why: String) -> Option<u32> {
        let (a, b) = (self.doc.clamp(a.min(b)), self.doc.clamp(a.max(b)));
        if a == b {
            return None;
        }
        let id = self.doc.fresh_id();
        self.seal();
        self.begin(Group::Other);
        self.doc.restyle(a, b, |s| s.add_anchor(id));
        self.doc.suggestions.push(Suggestion { id, markdown, why });
        self.changed();
        self.seal();
        Some(id)
    }

    /// Takes a suggestion: its text replaces what it was anchored to.
    pub fn accept(&mut self, id: u32) -> bool {
        let Some(s) = self.doc.suggestion(id).cloned() else {
            return false;
        };
        let Some((a, b)) = self.doc.anchor_range(id) else {
            self.doc.suggestions.retain(|s| s.id != id);
            return false;
        };
        self.seal();
        self.begin(Group::Other);
        let at = self.doc.delete(a, b);
        let blocks = markdown::read_blocks(&s.markdown);
        let end = self.doc.insert_fragment(at, blocks);
        self.doc.suggestions.retain(|s| s.id != id);
        self.selection = Selection::caret(end);
        self.changed();
        self.seal();
        true
    }

    /// Turns a suggestion down: the text stays as it was.
    pub fn reject(&mut self, id: u32) {
        if self.doc.suggestion(id).is_none() {
            return;
        }
        self.seal();
        self.begin(Group::Other);
        self.doc.clear_anchor(id);
        self.doc.suggestions.retain(|s| s.id != id);
        self.changed();
        self.seal();
    }

    /// Puts Claude's Markdown in at the caret as real text — a drafted
    /// section, which there is nothing to compare against.
    pub fn insert_markdown(&mut self, src: &str) {
        self.paste_blocks(markdown::read_blocks(src));
    }
}

/// `1. `, `2. `, … as typed at the start of a line.
fn is_ordinal(s: &str) -> bool {
    let Some(rest) = s.strip_suffix(". ") else {
        return false;
    };
    !rest.is_empty() && rest.len() <= 3 && rest.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(src: &str) -> Editor {
        Editor::new(markdown::read(src))
    }

    fn md(e: &Editor) -> String {
        markdown::write(&e.doc)
    }

    fn at(block: usize, offset: usize) -> Pos {
        Pos::new(block, offset)
    }

    #[test]
    fn typing_and_undo_by_words() {
        let mut e = Editor::default();
        for c in "hello big world".chars() {
            e.type_text(&c.to_string());
        }
        assert_eq!(e.doc.plain_text(), "hello big world");
        assert!(e.is_dirty());
        e.undo();
        assert_eq!(e.doc.plain_text(), "hello big");
        e.undo();
        assert_eq!(e.doc.plain_text(), "hello");
        e.redo();
        assert_eq!(e.doc.plain_text(), "hello big");
    }

    #[test]
    fn undoing_back_to_the_save_is_clean() {
        let mut e = editor("text");
        e.caret_to(at(0, 4));
        e.mark_saved();
        e.type_text("!");
        assert!(e.is_dirty());
        e.undo();
        assert!(!e.is_dirty());
    }

    #[test]
    fn return_splits_and_continues_a_list() {
        let mut e = editor("- one");
        e.caret_to(at(0, 3));
        e.enter();
        e.type_text("two");
        assert_eq!(md(&e), "- one\n- two\n");
        // Return in the empty item after leaves the list.
        e.enter();
        e.enter();
        assert_eq!(e.doc.blocks[2].kind, BlockKind::Paragraph);
    }

    #[test]
    fn return_after_a_heading_starts_a_paragraph() {
        let mut e = editor("# Title");
        e.caret_to(at(0, 5));
        e.enter();
        assert_eq!(e.doc.blocks[1].kind, BlockKind::Paragraph);
        assert_eq!(e.selection(), Selection::caret(at(1, 0)));
    }

    #[test]
    fn return_at_the_start_opens_a_line_above() {
        let mut e = editor("# Title");
        e.caret_to(at(0, 0));
        e.enter();
        assert_eq!(e.doc.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(e.doc.blocks[1].kind, BlockKind::Heading(1));
        assert_eq!(e.selection(), Selection::caret(at(1, 0)));
    }

    #[test]
    fn markdown_typed_out_becomes_formatting() {
        let mut e = Editor::default();
        for c in "## Plan".chars() {
            e.type_text(&c.to_string());
        }
        assert_eq!(e.doc.blocks[0].kind, BlockKind::Heading(2));
        assert_eq!(e.doc.plain_text(), "Plan");
        e.enter();
        for c in "1. first".chars() {
            e.type_text(&c.to_string());
        }
        assert_eq!(e.doc.blocks[1].kind, BlockKind::Numbered { depth: 0 });
        // Undo gives back the characters as typed, before the text after.
        let mut e = Editor::default();
        for c in "- ".chars() {
            e.type_text(&c.to_string());
        }
        e.undo();
        assert_eq!(e.doc.plain_text(), "- ");
        assert_eq!(e.doc.blocks[0].kind, BlockKind::Paragraph);
    }

    #[test]
    fn a_checklist_from_a_bullet() {
        let mut e = Editor::default();
        for c in "- [ ] milk".chars() {
            e.type_text(&c.to_string());
        }
        assert_eq!(
            e.doc.blocks[0].kind,
            BlockKind::Task {
                checked: false,
                depth: 0
            }
        );
        e.toggle_task(0);
        assert_eq!(md(&e), "- [x] milk\n");
    }

    #[test]
    fn backspace_takes_off_formatting_before_joining() {
        let mut e = editor("para\n\n- item");
        e.caret_to(at(1, 0));
        e.backspace(false);
        assert_eq!(e.doc.blocks[1].kind, BlockKind::Paragraph);
        e.backspace(false);
        assert_eq!(e.doc.plain_text(), "paraitem");
        assert_eq!(e.selection(), Selection::caret(at(0, 4)));
    }

    #[test]
    fn bold_on_a_selection_and_off_again() {
        let mut e = editor("make this bold");
        e.set_selection(Selection::new(at(0, 5), at(0, 9)));
        e.toggle_mark(Marks::BOLD);
        assert_eq!(md(&e), "make **this** bold\n");
        assert!(e.current_marks().contains(Marks::BOLD));
        e.toggle_mark(Marks::BOLD);
        assert_eq!(md(&e), "make this bold\n");
    }

    #[test]
    fn bold_with_a_caret_applies_to_what_is_typed_next() {
        let mut e = editor("a");
        e.caret_to(at(0, 1));
        e.toggle_mark(Marks::BOLD);
        e.type_text("b");
        e.type_text("c");
        assert_eq!(md(&e), "a**bc**\n");
    }

    #[test]
    fn typing_at_the_end_of_a_link_does_not_extend_it() {
        let mut e = editor("[site](https://example.com)");
        e.caret_to(at(0, 4));
        e.type_text(" now");
        assert_eq!(md(&e), "[site](https://example.com) now\n");
        e.caret_to(at(0, 2));
        e.type_text("X");
        assert_eq!(md(&e), "[siXte](https://example.com) now\n");
    }

    #[test]
    fn comments_anchor_to_what_was_selected() {
        let mut e = editor("one figure that carries it");
        e.set_selection(Selection::new(at(0, 4), at(0, 10)));
        let id = e.add_comment("Which figure?").unwrap();
        assert_eq!(e.doc.comment(id).unwrap().quote, "figure");
        e.caret_to(at(0, 0));
        e.type_text("Just ");
        assert_eq!(e.doc.anchor_range(id), Some((at(0, 9), at(0, 15))));
    }

    #[test]
    fn a_suggestion_changes_nothing_until_accepted() {
        let mut e = editor("Is the scaling enough?");
        let id = e
            .suggest(
                at(0, 7),
                at(0, 14),
                "*linear growth*".into(),
                "concrete".into(),
            )
            .unwrap();
        assert_eq!(e.doc.plain_text(), "Is the scaling enough?");
        assert!(e.accept(id));
        assert_eq!(md(&e), "Is the *linear growth* enough?\n");
        assert!(e.doc.suggestions.is_empty());
        // Undo brings the suggestion back, still pending.
        e.undo();
        assert_eq!(e.doc.plain_text(), "Is the scaling enough?");
        assert_eq!(e.doc.suggestions.len(), 1);
        e.reject(id);
        assert_eq!(e.doc.anchor_range(id), None);
        assert_eq!(e.doc.plain_text(), "Is the scaling enough?");
    }

    #[test]
    fn a_suggestion_over_several_paragraphs() {
        let mut e = editor("keep\n\nold one\n\nold two\n\nkeep too");
        let id = e
            .suggest(at(1, 0), at(2, 7), "new text".into(), String::new())
            .unwrap();
        e.accept(id);
        assert_eq!(md(&e), "keep\n\nnew text\n\nkeep too\n");
    }

    #[test]
    fn pasting_markdown_brings_its_structure() {
        let mut e = editor("before  after");
        e.caret_to(at(0, 7));
        e.paste_markdown("**bold** start\n\n- a\n- b\n\nend");
        assert_eq!(md(&e), "before **bold** start\n\n- a\n- b\n\nend after\n");
    }

    #[test]
    fn pasting_one_plain_line_types_it() {
        let mut e = editor("# Heading");
        e.caret_to(at(0, 7));
        e.paste_markdown("X_Y");
        assert_eq!(e.doc.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(e.doc.plain_text(), "HeadingX_Y");
    }

    #[test]
    fn tab_indents_list_items_only() {
        let mut e = editor("- a\n- b");
        e.caret_to(at(1, 1));
        assert!(e.indent(true));
        assert_eq!(md(&e), "- a\n  - b\n");
        let mut e = editor("plain");
        assert!(!e.indent(true));
    }

    #[test]
    fn code_fences_typed_out_start_a_code_block() {
        let mut e = Editor::default();
        e.type_text("```rust");
        e.enter();
        e.type_text("fn main() {}");
        e.enter();
        e.enter();
        e.type_text("after");
        assert_eq!(md(&e), "```rust\nfn main() {}\n```\n\nafter\n");
    }

    #[test]
    fn three_dashes_make_a_rule() {
        let mut e = editor("above");
        e.caret_to(at(0, 5));
        e.enter();
        e.type_text("---");
        e.enter();
        e.type_text("below");
        assert_eq!(md(&e), "above\n\n---\n\nbelow\n");
    }

    #[test]
    fn arrows_collapse_a_selection_first() {
        let mut e = editor("abc def");
        e.set_selection(Selection::new(at(0, 1), at(0, 5)));
        e.move_horizontal(false, false, false);
        assert_eq!(e.selection(), Selection::caret(at(0, 1)));
        e.move_horizontal(true, true, true);
        assert_eq!(e.selection(), Selection::new(at(0, 1), at(0, 3)));
    }
}

/// Random edits, checked against the invariants every other test takes for
/// granted. Deterministic: the same seed makes the same document.
#[cfg(test)]
mod random {
    use super::*;

    fn editor(src: &str) -> Editor {
        Editor::new(markdown::read(src))
    }

    fn md(e: &Editor) -> String {
        markdown::write(&e.doc)
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    fn any_pos(e: &Editor, r: &mut Rng) -> Pos {
        let block = r.below(e.doc.len());
        Pos::new(block, r.below(e.doc.blocks[block].len() + 1))
    }

    fn check(e: &Editor) {
        assert!(!e.doc.blocks.is_empty());
        for b in &e.doc.blocks {
            assert_eq!(b.text().chars().count(), b.styles().len());
            if !b.kind.takes_marks() {
                assert!(
                    b.styles()
                        .iter()
                        .all(|s| s.marks.is_empty() && s.link.is_none())
                );
            }
        }
        assert_eq!(e.doc.clamp(e.selection().head), e.selection().head);
        assert_eq!(e.doc.clamp(e.selection().anchor), e.selection().anchor);
    }

    #[test]
    fn random_editing_keeps_every_invariant() {
        let words = [
            "word",
            " ",
            "é",
            "**",
            "# ",
            "- ",
            "1. ",
            "[ ] ",
            "> ",
            "`",
            "\n",
            "line\nbreak",
            "_x_",
            "🦀",
            "---",
            "```",
            "a\u{301}",
        ];
        for seed in 1..=40u64 {
            let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let mut e = editor("# Start\n\nSome *text* here.\n\n- one\n- two\n");
            let original = md(&e);
            let mut steps = 0;
            for _ in 0..150 {
                match r.below(16) {
                    0..=4 => e.type_text(words[r.below(words.len())]),
                    5 => e.enter(),
                    6 => e.backspace(r.below(2) == 0),
                    7 => e.delete_forward(r.below(2) == 0),
                    8 => {
                        let (a, b) = (any_pos(&e, &mut r), any_pos(&e, &mut r));
                        e.set_selection(Selection::new(a, b));
                    }
                    9 => e.toggle_mark(Marks::ALL[r.below(Marks::ALL.len())]),
                    10 => {
                        let kinds = [
                            BlockKind::Paragraph,
                            BlockKind::Heading(2),
                            BlockKind::Bullet { depth: 0 },
                            BlockKind::Numbered { depth: 0 },
                            BlockKind::Task {
                                checked: false,
                                depth: 0,
                            },
                            BlockKind::Quote,
                            BlockKind::Code {
                                lang: String::new(),
                            },
                        ];
                        e.toggle_kind(kinds[r.below(kinds.len())].clone());
                    }
                    11 => {
                        e.indent(r.below(2) == 0);
                    }
                    12 => {
                        let copied = e.selected_markdown();
                        let to = any_pos(&e, &mut r);
                        e.caret_to(to);
                        e.paste_markdown(&copied);
                    }
                    13 => {
                        let (a, b) = (any_pos(&e, &mut r), any_pos(&e, &mut r));
                        if let Some(id) = e.suggest(a, b, "*new* text".into(), String::new()) {
                            if r.below(2) == 0 {
                                e.accept(id);
                            } else {
                                e.reject(id);
                            }
                        }
                    }
                    14 => {
                        e.move_horizontal(r.below(2) == 0, r.below(2) == 0, r.below(2) == 0);
                    }
                    _ => {
                        e.undo();
                    }
                }
                steps += 1;
                check(&e);
            }
            // Saving is a fixed point after one round: what is read back
            // writes out the same again.
            let once = markdown::write(&e.doc);
            let twice = markdown::write(&markdown::read(&once));
            assert_eq!(once, twice, "seed {seed} after {steps} steps");
            // And undo goes all the way back.
            while e.undo() {}
            assert_eq!(md(&e), original, "seed {seed}");
        }
    }
}
