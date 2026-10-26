//! A buffer: text, a cursor, and the history that got it here.
//!
//! `ropey` does the heavy lifting. It is behind a thin surface on purpose —
//! nothing outside this file knows the store is a rope, so replacing it with a
//! summary tree later (folded height, diagnostic counts) is a change to one
//! module rather than to everything that reads text.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::edit::{Cursor, Edit, Position, Range};
use crate::history::History;
use crate::motion;

/// How the next edit joins the undo history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// Each edit is its own undo step.
    None,
    /// The next edit starts a group; the ones after it join it.
    Pending,
    /// Edits fold into the current history node.
    Joined,
}

#[derive(Debug)]
pub struct Buffer {
    text: Rope,
    path: Option<PathBuf>,
    history: History,
    /// The history revision the file on disk matches.
    saved_at: usize,
    /// When the file was last modified as of the read or write that made this
    /// buffer agree with it.
    ///
    /// Kept so a write can notice that something else got there first.
    /// Formatters, language servers and `git checkout` rewrite files under an
    /// open editor constantly, and a `:w` that silently discards that is the
    /// one data-loss bug an editor must not have.
    disk: Option<std::time::SystemTime>,
    /// Whether edits are currently folding into one undoable group.
    group: Group,
    /// Bumped on every change so the front end can drop stale deltas.
    version: u64,
    /// A bounded log of what changed, for consumers that can update
    /// themselves more cheaply than they can start again — the parser, above
    /// all. Bounded because it is an optimisation: when it runs out, the
    /// answer is "start over", which is always correct.
    changes: VecDeque<(u64, Change)>,
    /// Named positions, `ma` to set and ``a` to return. Held here, not in the
    /// editor, because a mark is a place in *this* text and has to move when
    /// the text around it does.
    marks: HashMap<char, usize>,
    pub cursor: Cursor,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl Buffer {
    pub fn from_text(text: &str) -> Self {
        Self {
            text: Rope::from_str(text),
            path: None,
            history: History::new(),
            saved_at: 0,
            disk: None,
            group: Group::None,
            version: 0,
            changes: VecDeque::new(),
            marks: HashMap::new(),
            cursor: Cursor::default(),
        }
    }

    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            disk: modified_at(path),
            ..Self::from_text(&text)
        })
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "buffer has no path",
            ));
        };
        write_whole(&path, &self.text.to_string())?;
        self.saved_at = self.history.revision();
        self.disk = modified_at(&path);
        // The saved revision must stay immutable. More typing belongs to a
        // new node, or joining it would leave changed text marked clean.
        self.end_group();
        Ok(())
    }

    /// Write the text somewhere else, leaving this buffer alone.
    ///
    /// `:w {file}` in vim is a copy, not a rename — the buffer keeps its own
    /// name and stays modified. [`Buffer::rename`] is the other half.
    pub fn write_to(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, self.text.to_string())
    }

    /// Point the buffer at a different file. Does not write it.
    pub fn rename(&mut self, path: impl Into<PathBuf>) {
        self.path = Some(path.into());
        // Whatever is at the new path, this buffer has not agreed with it.
        self.disk = None;
    }

    /// Whether the file changed underneath us since we last read or wrote it.
    ///
    /// `false` when it cannot be told — an unnamed buffer, an unreadable
    /// directory, a filesystem with no useful timestamps. This guards a write,
    /// and a guard that fires when it does not know is a guard people learn to
    /// force past.
    pub fn changed_on_disk(&self) -> bool {
        let (Some(path), Some(known)) = (&self.path, self.disk) else {
            return false;
        };
        match modified_at(path) {
            Some(now) => now != known,
            None => false,
        }
    }

    /// Re-read the file, replacing everything.
    ///
    /// Goes through `apply` like any other change rather than rebuilding the
    /// buffer, which is what keeps the parse tree incremental, the marks
    /// shifted, and the reload itself undoable — `u` after an `:e!` gets your
    /// work back, which a rebuilt buffer could not offer.
    pub fn reload(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "buffer has no file to re-read",
            ));
        };
        let text = std::fs::read_to_string(&path)?;

        let whole = Range {
            start: 0,
            end: self.text.len_chars(),
        };
        let at = self.cursor.offset.min(text.chars().count());
        self.apply(vec![Edit::replace(whole, text)], self.cursor.moved_to(at));
        self.cursor = self.clamp(self.cursor);

        self.saved_at = self.history.revision();
        self.disk = modified_at(&path);
        Ok(())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[No Name]".into())
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn is_modified(&self) -> bool {
        self.history.revision() != self.saved_at
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn contents(&self) -> String {
        self.text.to_string()
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.text.len_lines()
    }

    /// One line, without its newline.
    pub fn line(&self, index: usize) -> String {
        if index >= self.text.len_lines() {
            return String::new();
        }
        let line = self.text.line(index);
        let mut s = line.to_string();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    /// A window of lines, for a virtualised view.
    pub fn lines(&self, from: usize, count: usize) -> Vec<String> {
        let last = (from + count).min(self.text.len_lines());
        (from..last).map(|i| self.line(i)).collect()
    }

    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.text.len_chars());
        let line = self.text.char_to_line(offset);
        Position {
            line,
            column: offset - self.text.line_to_char(line),
        }
    }

    pub fn offset_of(&self, position: Position) -> usize {
        let line = position.line.min(self.text.len_lines().saturating_sub(1));
        let start = self.text.line_to_char(line);
        (start + position.column).min(motion::line_end(self.text.slice(..), start))
    }

    /// LSP speaks UTF-16 code units, so the conversion belongs here rather
    /// than being rediscovered by every caller.
    pub fn utf16_of(&self, offset: usize) -> usize {
        self.text
            .char_to_utf16_cu(offset.min(self.text.len_chars()))
    }

    /// The column at `offset`, counted in UTF-16 code units from the start of
    /// its line — which is what an LSP position's `character` actually is. Not
    /// bytes, not characters: an emoji before the cursor moves this by two.
    pub fn utf16_column(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len_chars());
        let start = self.text.line_to_char(self.text.char_to_line(offset));
        self.utf16_of(offset) - self.utf16_of(start)
    }

    /// The inverse, for a position a server sent us. Clamped to the line, so a
    /// diagnostic that arrives one edit late lands somewhere real instead of
    /// panicking.
    pub fn offset_at_utf16(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.text.len_lines().saturating_sub(1));
        let start = self.text.line_to_char(line);
        let end = motion::line_end(self.text.slice(..), start);
        let target = self.utf16_of(start) + column;
        if target >= self.text.len_utf16_cu() {
            return end;
        }
        self.text.utf16_cu_to_char(target).min(end)
    }

    /* ── editing ──────────────────────────────────────────────────── */

    /// Apply edits and record them as one undoable change.
    ///
    /// Callers pass edits in natural document order and they must not overlap.
    /// Everything is applied back to front so earlier offsets stay valid.
    pub fn apply(&mut self, mut edits: Vec<Edit>, cursor_after: Cursor) {
        if edits.is_empty() {
            return;
        }
        edits.sort_by_key(|e| e.start);
        let before = self.cursor;

        // The inverses are computed against the text as it will be *after* the
        // whole change, not against the intermediate states — otherwise every
        // inverse but the first carries offsets that no longer exist, and a
        // multi-edit undo reads off the end. `delta` is how far each later
        // edit has shifted by the time the change is done.
        let mut inverse = Vec::with_capacity(edits.len());
        let mut delta: isize = 0;
        for edit in &edits {
            let removed = self.text.slice(edit.start..edit.end).to_string();
            let start = (edit.start as isize + delta) as usize;
            inverse.push(Edit {
                start,
                end: start + edit.inserted_len(),
                text: removed,
            });
            delta += edit.inserted_len() as isize - (edit.end - edit.start) as isize;
        }

        // Both directions are stored in the order they apply: highest
        // offset first within one change, and for a group, chronological
        // across changes. Replay then never has to sort — and must not,
        // because a group's edits are sequential and depend on each other.
        edits.sort_by_key(|e| std::cmp::Reverse(e.start));
        inverse.sort_by_key(|e| std::cmp::Reverse(e.start));

        self.splice(&edits);
        self.cursor = self.clamp(cursor_after);

        match self.group {
            Group::Joined => self.history.join(edits, inverse, self.cursor),
            _ => {
                self.history.commit(edits, inverse, before, self.cursor);
                if self.group == Group::Pending {
                    self.group = Group::Joined;
                }
            }
        }
    }

    /// Start folding edits into one undoable change.
    ///
    /// `join_current` is true when the action that opened the group already
    /// made an edit: `c` deletes before insert mode starts, and that deletion
    /// belongs to the same undo step as the typing that follows.
    pub fn begin_group(&mut self, join_current: bool) {
        self.group = if join_current && self.history.revision() != self.saved_at {
            Group::Joined
        } else {
            Group::Pending
        };
    }

    pub fn end_group(&mut self) {
        self.group = Group::None;
    }

    /// Continue a typing run, starting a fresh step after a boundary such as
    /// a save or cursor movement. Unlike `begin_group`, this is idempotent.
    pub fn ensure_group(&mut self) {
        if self.group == Group::None {
            self.group = Group::Pending;
        }
    }

    /// Apply edits in the order given. Callers order them highest offset
    /// first so that earlier offsets stay valid.
    fn splice(&mut self, edits: &[Edit]) {
        // Recorded here rather than in `apply`, because this is the one place
        // the text actually moves — which means undo and redo are journalled
        // too, for free, and a consumer following along cannot miss one.
        let mut recorded = Vec::with_capacity(edits.len());

        for edit in edits {
            let start_byte = self.text.char_to_byte(edit.start);
            let old_end_byte = self.text.char_to_byte(edit.end);
            let start_point = self.point_of(edit.start);
            let old_end_point = self.point_of(edit.end);

            self.text.remove(edit.start..edit.end);
            if !edit.text.is_empty() {
                self.text.insert(edit.start, &edit.text);
            }
            self.shift_marks(edit);

            let new_end = edit.start + edit.inserted_len();
            recorded.push(Change {
                start_byte,
                old_end_byte,
                new_end_byte: self.text.char_to_byte(new_end),
                start_point,
                old_end_point,
                new_end_point: self.point_of(new_end),
            });
        }

        self.version += 1;
        for change in recorded {
            if self.changes.len() >= JOURNAL {
                self.changes.pop_front();
            }
            self.changes.push_back((self.version, change));
        }
    }

    /* ── marks ────────────────────────────────────────────────────── */

    /// Remember where the cursor is, under a name.
    pub fn set_mark(&mut self, name: char, offset: usize) {
        self.marks.insert(name, offset.min(self.text.len_chars()));
    }

    /// Where a mark is now — which is not necessarily where it was set.
    pub fn mark(&self, name: char) -> Option<usize> {
        self.marks
            .get(&name)
            .map(|offset| (*offset).min(self.text.len_chars()))
    }

    pub fn marks(&self) -> impl Iterator<Item = (char, usize)> + '_ {
        self.marks.iter().map(|(name, offset)| (*name, *offset))
    }

    /// Move every mark that an edit displaced.
    ///
    /// This is the whole difficulty with marks. A mark set at offset 400 and
    /// then left alone through an hour of editing above it is not at 400 any
    /// more, and one that still claims to be is worse than no mark at all —
    /// it takes you somewhere confidently wrong.
    fn shift_marks(&mut self, edit: &Edit) {
        if self.marks.is_empty() {
            return;
        }
        let removed = edit.end - edit.start;
        let inserted = edit.inserted_len();
        if removed == 0 && inserted == 0 {
            return;
        }

        for offset in self.marks.values_mut() {
            if *offset <= edit.start {
                // Before the edit: untouched. `<=` rather than `<` so text
                // inserted exactly at a mark pushes past it rather than
                // swallowing it.
                continue;
            }
            *offset = if *offset >= edit.end {
                (*offset + inserted).saturating_sub(removed)
            } else {
                // Inside what was deleted. There is no honest answer, so it
                // collapses to where that text used to begin.
                edit.start
            };
        }
    }

    /// Row and column at a character offset, with the column in **bytes** —
    /// which is what an incremental parser means by a point, and not what the
    /// cursor means by a column.
    fn point_of(&self, offset: usize) -> Point {
        let offset = offset.min(self.text.len_chars());
        let row = self.text.char_to_line(offset);
        let line_start = self.text.line_to_char(row);
        Point {
            row,
            column: self.text.char_to_byte(offset) - self.text.char_to_byte(line_start),
        }
    }

    /// Everything that has changed since `version`, in the order it was
    /// applied.
    ///
    /// `None` means the journal no longer reaches back that far and the
    /// caller has to start over. That fallback is why the journal can be
    /// bounded at all: being unable to answer is safe, and answering wrongly
    /// would leave a parse tree describing text that no longer exists.
    pub fn changes_since(&self, version: u64) -> Option<Vec<Change>> {
        if version == self.version {
            return Some(Vec::new());
        }
        if version > self.version {
            // The caller saw a version we have never reached — it is looking
            // at a different buffer, or one that was reloaded.
            return None;
        }
        let reaches_back = self.changes.front().is_some_and(|(v, _)| *v <= version + 1);
        if !reaches_back {
            return None;
        }
        Some(
            self.changes
                .iter()
                .filter(|(v, _)| *v > version)
                .map(|(_, change)| *change)
                .collect(),
        )
    }

    pub fn undo(&mut self) -> bool {
        self.end_group();
        let Some(step) = self.history.undo() else {
            return false;
        };
        self.replay(step.edits);
        self.cursor = self.clamp(step.cursor);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_group();
        let Some(step) = self.history.redo() else {
            return false;
        };
        self.replay(step.edits);
        self.cursor = self.clamp(step.cursor);
        true
    }

    /// More than one branch out of here means an undo went back past a fork,
    /// and the other branch is still reachable.
    pub fn undo_branches(&self) -> usize {
        self.history.branches()
    }

    /// History steps are replayed rather than re-committed — recording them
    /// would make undo itself undoable and the tree would never terminate.
    /// Applies the stored order exactly. A group's edits are sequential and
    /// dependent, so sorting them here would be wrong.
    fn replay(&mut self, edits: Vec<Edit>) {
        self.splice(&edits);
    }

    /// Keep the cursor inside the text, and off the newline — normal mode sits
    /// *on* a character, not past the end of the line.
    pub fn clamp(&self, cursor: Cursor) -> Cursor {
        let len = self.text.len_chars();
        let offset = cursor.offset.min(len);
        let limit = motion::line_end(self.text.slice(..), offset);
        let start = motion::line_start(self.text.slice(..), offset);
        let clamped = offset.min(limit.max(start));
        Cursor {
            offset: clamped,
            anchor: cursor.anchor.min(len),
            goal_column: cursor.goal_column,
        }
    }

    /* ── motions, bound to this buffer ────────────────────────────── */

    pub fn word_forward(&self, count: usize, big: bool) -> usize {
        let mut at = self.cursor.offset;
        for _ in 0..count.max(1) {
            at = motion::word_forward(self.text.slice(..), at, big);
        }
        at
    }

    pub fn word_backward(&self, count: usize, big: bool) -> usize {
        let mut at = self.cursor.offset;
        for _ in 0..count.max(1) {
            at = motion::word_backward(self.text.slice(..), at, big);
        }
        at
    }

    pub fn word_end(&self, count: usize, big: bool) -> usize {
        let mut at = self.cursor.offset;
        for _ in 0..count.max(1) {
            at = motion::word_end(self.text.slice(..), at, big);
        }
        at
    }

    pub fn line_start(&self) -> usize {
        motion::line_start(self.text.slice(..), self.cursor.offset)
    }

    pub fn line_end(&self) -> usize {
        motion::line_end(self.text.slice(..), self.cursor.offset)
    }

    pub fn first_non_blank(&self) -> usize {
        motion::first_non_blank(self.text.slice(..), self.cursor.offset)
    }

    pub fn line_range(&self, count: usize) -> Range {
        motion::line_range(self.text.slice(..), self.cursor.offset, count)
    }

    /// `count` whole lines starting at `line`, wherever the cursor happens to
    /// be — what a linewise operator between two points acts on.
    pub fn line_span(&self, line: usize, count: usize) -> Range {
        let at = self
            .text
            .line_to_char(line.min(self.text.len_lines().saturating_sub(1)));
        motion::line_range(self.text.slice(..), at, count)
    }

    pub fn column(&self) -> usize {
        motion::column_of(self.text.slice(..), self.cursor.offset)
    }

    /// The goal column, remembered across vertical moves.
    pub fn goal(&self) -> usize {
        self.cursor.goal_column.unwrap_or_else(|| self.column())
    }

    pub fn line_down(&self, count: usize) -> usize {
        motion::line_down(self.text.slice(..), self.cursor.offset, count, self.goal())
    }

    pub fn line_up(&self, count: usize) -> usize {
        motion::line_up(self.text.slice(..), self.cursor.offset, count, self.goal())
    }

    pub fn file_start(&self) -> usize {
        0
    }

    pub fn file_end(&self) -> usize {
        motion::line_start(self.text.slice(..), self.text.len_chars())
    }

    pub fn inner_word(&self, big: bool) -> Range {
        motion::inner_word(self.text.slice(..), self.cursor.offset, big)
    }

    pub fn a_word(&self, big: bool) -> Range {
        motion::a_word(self.text.slice(..), self.cursor.offset, big)
    }

    pub fn inside_pair(&self, open: char, close: char, around: bool) -> Option<Range> {
        motion::inside_pair(self.text.slice(..), self.cursor.offset, open, close, around)
    }

    pub fn slice(&self, range: Range) -> String {
        let len = self.text.len_chars();
        self.text
            .slice(range.start.min(len)..range.end.min(len))
            .to_string()
    }
}

/// How many changes the journal keeps.
///
/// Generous enough that a burst of typing between redraws is always covered,
/// small enough that it is never a memory concern. Overflowing costs a
/// re-parse, not a wrong answer.
const JOURNAL: usize = 512;

/// A row and a column, with the column counted in **bytes**.
///
/// Not the same thing as a [`Position`], whose column is characters. Both are
/// needed: one addresses the cursor, the other addresses a parse tree, and
/// conflating them puts every highlight on a line with an emoji in the wrong
/// place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub row: usize,
    pub column: usize,
}

/// One contiguous replacement, in the coordinates an incremental parser wants.
///
/// Deliberately not a tree-sitter type: this is a description of what happened
/// to some text, and `quark-text` has no business knowing what is going to
/// read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: Point,
    pub old_end_point: Point,
    pub new_end_point: Point,
}

/// The file's modification time, if the filesystem will say.
fn modified_at(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

impl Buffer {
    /* ── search ───────────────────────────────────────────────────────
    Literal, with vim's smartcase: a pattern containing a capital is
    case-sensitive, one without is not. No regular expressions yet —
    `/` earns its keep long before `\<foo\>` does, and a half-supported
    regex dialect is worse than an honest literal one. */

    /// Every offset where `pattern` occurs, in order.
    ///
    /// The whole list rather than the next one, because that is what makes
    /// wrapping fall out — and what `hlsearch` will want when it arrives.
    pub fn matches(&self, pattern: &str) -> Vec<usize> {
        let needle: Vec<char> = pattern.chars().collect();
        if needle.is_empty() || needle.len() > self.text.len_chars() {
            return Vec::new();
        }

        // The folding rule lives in `search` because the viewport's highlight
        // needs the same one. A search that jumps to a match it will not
        // highlight is worse than one that does neither.
        let sensitive = crate::search::is_sensitive(pattern);
        let same = |a: char, b: char| crate::search::same(a, b, sensitive);

        let mut found = Vec::new();
        let mut window: VecDeque<char> = VecDeque::with_capacity(needle.len());
        for (at, c) in self.text.chars().enumerate() {
            if window.len() == needle.len() {
                window.pop_front();
            }
            window.push_back(c);
            if window.len() == needle.len() && window.iter().zip(&needle).all(|(&a, &b)| same(a, b))
            {
                found.push(at + 1 - needle.len());
            }
        }
        found
    }

    /// The next match from `from`, wrapping round the end of the file.
    ///
    /// Strictly after (or before) `from`, so pressing `n` on a match goes to
    /// the next one rather than staying put.
    pub fn find(&self, pattern: &str, from: usize, forward: bool) -> Option<usize> {
        let all = self.matches(pattern);
        if forward {
            all.iter()
                .copied()
                .find(|&at| at > from)
                .or_else(|| all.first().copied())
        } else {
            all.iter()
                .copied()
                .rev()
                .find(|&at| at < from)
                .or_else(|| all.last().copied())
        }
    }

    /// The word under `offset`, for `*` and `#`.
    pub fn word_at(&self, offset: usize) -> Option<String> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let len = self.text.len_chars();
        if offset >= len || !is_word(self.text.char(offset)) {
            return None;
        }
        let mut start = offset;
        while start > 0 && is_word(self.text.char(start - 1)) {
            start -= 1;
        }
        let mut end = offset + 1;
        while end < len && is_word(self.text.char(end)) {
            end += 1;
        }
        Some(self.text.slice(start..end).to_string())
    }
}

/// Write a file whole or not at all: beside it, then over it by a rename, so
/// a write cut short — a full disk, a crash — leaves the file as it was
/// rather than truncated. Through a link it is the file the link leads to
/// that is replaced, and the link stays a link; a file with other hard links,
/// which a rename would part it from, is written in place.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let meta = std::fs::metadata(&target).ok();
    #[cfg(unix)]
    if meta
        .as_ref()
        .is_some_and(|m| std::os::unix::fs::MetadataExt::nlink(m) > 1)
    {
        return std::fs::write(&target, text);
    }
    let mut part = target.as_os_str().to_owned();
    part.push(".quark-saving");
    let part = PathBuf::from(part);
    if let Err(err) = std::fs::write(&part, text) {
        // A folder nothing new may be made in can still hold a file that may
        // be written: in place, then, as it always was.
        return match err.kind() {
            std::io::ErrorKind::PermissionDenied => std::fs::write(&target, text),
            _ => Err(err),
        };
    }
    // The file's own permissions, not the ones a new file gets.
    if let Some(meta) = &meta {
        let _ = std::fs::set_permissions(&part, meta.permissions());
    }
    std::fs::rename(&part, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}
