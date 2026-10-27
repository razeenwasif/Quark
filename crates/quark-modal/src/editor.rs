//! Where an [`Action`] becomes an edit.
//!
//! The modal engine says *what* — `Delete · InnerWord · 2` — and this decides
//! what that means for a buffer. Keeping the two apart is what lets Video mode
//! answer the same action with a ripple delete without either side knowing
//! about the other.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use quark_text::edit::Range;
use quark_text::{motion, Buffer, Cursor, Edit, Position};

use crate::action::{Action, Object, Verb};
use crate::block::{Block, PendingInsert};
use crate::root::Root;
use crate::vim::VimMode;

/// How an object resolves against a buffer.
enum Resolved {
    /// A place to put the cursor.
    Motion(usize),
    /// A span of characters.
    Span(Range),
    /// A span of whole lines — deleted with their newline, and yanked so that
    /// putting them back makes new lines rather than splicing into one.
    Linewise(Range),
}

#[derive(Debug, Default)]
pub struct Editor {
    buffers: Vec<Buffer>,
    active: usize,
    registers: HashMap<String, Register>,
    /// What the last yank or delete put in the unnamed register (or `"+`,
    /// `"*`), not yet handed on for the system clipboard.
    clipboard: Option<String>,
    /// A block insert waiting for insert mode to end. See `block.rs`.
    block_insert: Option<PendingInsert>,
    /// One level of indentation, for `>` and `<`: a tab, or a run of spaces —
    /// whichever the file being edited uses. The workspace sets it, because
    /// the settings and the file are both its to read.
    indent: String,
    /// The project these files are in, under both of its names, so that a
    /// file is one buffer whichever name its path came through. Without one,
    /// paths match only as they are spelled.
    root: Root,
}

/// What shape the text in a register is, which decides how it comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shape {
    #[default]
    Charwise,
    /// Whole lines: put makes new lines rather than splicing into one.
    Linewise,
    /// A rectangle: put lays it out at the same column on successive lines.
    Blockwise,
}

#[derive(Debug, Clone, Default)]
struct Register {
    text: String,
    shape: Shape,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            buffers: vec![Buffer::default()],
            active: 0,
            registers: HashMap::new(),
            clipboard: None,
            block_insert: None,
            indent: "    ".into(),
            root: Root::default(),
        }
    }

    /// An editor for the files of one project. See `root` for why it needs to
    /// know which.
    pub fn rooted(root: Root) -> Self {
        Self {
            root,
            ..Self::new()
        }
    }

    /// What `>` adds and `<` takes away.
    pub fn set_indent(&mut self, unit: &str) {
        self.indent = unit.to_string();
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    pub fn buffers(&self) -> &[Buffer] {
        &self.buffers
    }

    /// The buffers with unsaved changes, by name.
    pub fn unsaved(&self) -> Vec<&Buffer> {
        self.buffers.iter().filter(|buffer| buffer.is_modified()).collect()
    }

    /// Write every buffer with unsaved changes, and answer the ones that were
    /// not, with why: one with no file, or whose file changed on disk since it
    /// was read — written over, those changes would be lost without a word.
    pub fn save_unsaved(&mut self) -> Vec<(String, String)> {
        let mut failed = Vec::new();
        for buffer in self.buffers.iter_mut().filter(|buffer| buffer.is_modified()) {
            let why = match (buffer.path().is_some(), buffer.changed_on_disk()) {
                (false, _) => Some("it has no file to be saved to".to_string()),
                (true, true) => Some("its file changed on disk since it was read".to_string()),
                (true, false) => buffer.save().err().map(|err| err.to_string()),
            };
            if let Some(why) = why {
                failed.push((buffer.name(), why));
            }
        }
        failed
    }

    /// Re-read every buffer whose file changed on disk and that has nothing
    /// unsaved in it, and say which. A buffer with unsaved work is never
    /// overwritten: it keeps its text, and its next `:w` is the one that
    /// notices the disk moved.
    pub fn reread_changed_on_disk(&mut self) -> Vec<String> {
        self.buffers
            .iter_mut()
            .filter(|buffer| !buffer.is_modified() && buffer.changed_on_disk())
            .filter_map(|buffer| buffer.reload().ok().map(|()| buffer.name()))
            .collect()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn focus(&mut self, index: usize) -> bool {
        if index < self.buffers.len() {
            self.active = index;
            return true;
        }
        false
    }

    /// Open a file, or focus it if it is already open — the same path twice
    /// must not become two buffers with diverging histories.
    pub fn open(&mut self, path: impl AsRef<Path>) -> std::io::Result<usize> {
        let path = path.as_ref();
        let canonical = canonical(path);
        if let Some(i) = self.holding(path, &canonical) {
            self.active = i;
            return Ok(i);
        }
        let buffer = Buffer::open(&canonical)?;
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        Ok(self.active)
    }

    /// The buffer holding a file, if one is open: the one `open` would focus
    /// rather than read the file again.
    ///
    /// For anything that has to see what the user is looking at. Missing the
    /// buffer means reading the disk, which quietly drops every unsaved edit.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.holding(path, &canonical(path))
    }

    /// Matched through both names of the project, and against both spellings
    /// of the path: as it was asked for, and canonical.
    ///
    /// A buffer `open` made is spelled canonically, but one named by
    /// `:w {file}`, `:saveas` or `open_or_create` keeps the spelling it was
    /// given — through the name the project was opened by, or through a link
    /// further in. Matching one way only is how one file becomes two buffers.
    fn holding(&self, path: &Path, canonical: &Path) -> Option<usize> {
        self.buffers.iter().position(|b| {
            b.path()
                .is_some_and(|p| self.root.same(p, path) || self.root.same(p, canonical))
        })
    }

    /// Open a file, or make a buffer for one that is not there yet.
    ///
    /// For a staged change that creates a file: the review has to be looked at
    /// before anything is written, so the buffer has to exist before the file
    /// does. `text` is what it starts with, which is empty for a new file and
    /// the current contents for one that failed to open for another reason.
    pub fn open_or_create(&mut self, path: &Path, text: &str) -> std::io::Result<usize> {
        if let Ok(index) = self.open(path) {
            return Ok(index);
        }
        let mut buffer = quark_text::Buffer::from_text(text);
        buffer.rename(path);
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        Ok(self.active)
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.buffer_mut().save()
    }

    /// Close a buffer, and say whether there was one to close.
    ///
    /// The editor always has at least one buffer — `buffer()` indexes into the
    /// list and every caller assumes it succeeds — so closing the last one
    /// leaves a fresh empty buffer rather than nothing to draw. That is also
    /// what a user means by it: an editor showing no editor is not a state
    /// anybody asked for.
    ///
    /// Which one you land on afterwards is the tab-bar convention rather than
    /// vim's: the neighbour to the left, or the new last one if you closed off
    /// the end.
    pub fn close(&mut self, index: usize) -> bool {
        if index >= self.buffers.len() {
            return false;
        }
        self.buffers.remove(index);

        if self.buffers.is_empty() {
            self.buffers.push(Buffer::default());
            self.active = 0;
        } else if index < self.active {
            // Everything after it shifted down; the same buffer is still open.
            self.active -= 1;
        } else if index == self.active {
            self.active = index.min(self.buffers.len() - 1);
        }

        // A block insert is a promise to type into a rectangle of the buffer
        // that was active when it started, and that buffer may have just gone.
        self.block_insert = None;
        true
    }

    /* ── executing actions ────────────────────────────────────────── */

    /// Run one action. Returns whether the buffer changed, so the caller knows
    /// whether to send a delta or just a cursor move.
    pub fn execute(&mut self, action: &Action, mode: VimMode) -> bool {
        let count = action.count.max(1) as usize;
        let visual = matches!(
            mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        );

        match action.verb {
            Verb::Move => {
                let buf = self.buffer_mut();
                let Some(resolved) = resolve(buf, action.object, count) else {
                    return false;
                };
                let target = match resolved {
                    Resolved::Motion(at) => at,
                    Resolved::Span(r) | Resolved::Linewise(r) => r.start,
                };
                // Vertical motions keep the goal column; everything else sets it.
                let vertical = matches!(action.object, Object::LineDown | Object::LineUp);
                let goal = if vertical { Some(buf.goal()) } else { None };
                let cursor = if visual {
                    buf.cursor.extended_to(target)
                } else {
                    buf.cursor.moved_to(target)
                };
                let cursor = buf.clamp(Cursor {
                    goal_column: goal,
                    ..cursor
                });
                // Normal mode never rests on the line break: `$` lands on the
                // last character, `l` stops at it, and `j` onto a shorter line
                // comes to rest on its end rather than past it — as in vim.
                // Insert mode can, which is where `A` types.
                buf.cursor = match mode {
                    VimMode::Normal => on_a_character(buf, cursor),
                    _ => cursor,
                };
                false
            }

            // A block is several ranges rather than one, so it turns off here
            // rather than being squeezed through machinery built for a run.
            Verb::Delete | Verb::Change | Verb::Yank
                if mode == VimMode::VisualBlock && action.object == Object::Selection =>
            {
                self.block_operator(action)
            }

            Verb::Delete | Verb::Change | Verb::Yank => {
                let Some((range, linewise)) = self.span_for(action, count, mode) else {
                    return false;
                };
                if range.is_empty() {
                    return false;
                }
                let text = self.buffer().slice(range);
                self.write_register(
                    action.register.as_deref(),
                    text,
                    if linewise {
                        Shape::Linewise
                    } else {
                        Shape::Charwise
                    },
                );

                if action.verb == Verb::Yank {
                    let buf = self.buffer_mut();
                    buf.cursor = buf.clamp(buf.cursor.moved_to(range.start));
                    return false;
                }

                // `cc`, `S` and `c` on a line-wise selection empty the lines
                // into one rather than removing them: what is typed replaces
                // them, on a line of its own, at the indentation they had.
                if action.verb == Verb::Change && linewise {
                    let buf = self.buffer_mut();
                    let first = buf.position(range.start).line;
                    let indent: String = buf
                        .line(first)
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .collect();
                    let end = match buf.slice(range).ends_with('\n') {
                        true => range.end - 1,
                        false => range.end,
                    };
                    let at = range.start + indent.chars().count();
                    buf.apply(
                        vec![Edit::replace(Range::new(range.start, end), indent)],
                        Cursor::at(at),
                    );
                    return true;
                }

                let buf = self.buffer_mut();
                buf.apply(vec![Edit::delete(range)], Cursor::at(range.start));
                // Not in insert mode, where `⌃w` deletes too and the cursor
                // belongs exactly where the deleted text began.
                if action.verb == Verb::Delete && !matches!(mode, VimMode::Insert | VimMode::Replace) {
                    // Whole lines gone: on to the first non-blank of the line
                    // that took their place. Otherwise on a character, as
                    // everywhere in normal mode — `x` on a line's last one
                    // steps back rather than onto the break.
                    let cursor = match linewise {
                        true => Cursor::at(buf.first_non_blank()),
                        false => buf.cursor,
                    };
                    buf.cursor = on_a_character(buf, cursor);
                }
                true
            }

            Verb::Indent | Verb::Outdent => self.shift(action, count, mode),

            Verb::Lowercase | Verb::Uppercase | Verb::ToggleCase => {
                let Some((range, _)) = self.span_for(action, count, mode) else {
                    return false;
                };
                let buf = self.buffer_mut();
                let before = buf.slice(range);
                let after: String = before
                    .chars()
                    .flat_map(|c| -> Vec<char> {
                        match action.verb {
                            Verb::Lowercase => c.to_lowercase().collect(),
                            Verb::Uppercase => c.to_uppercase().collect(),
                            _ if c.is_lowercase() => c.to_uppercase().collect(),
                            _ if c.is_uppercase() => c.to_lowercase().collect(),
                            _ => vec![c],
                        }
                    })
                    .collect();
                // `~` walks on past what it changed; an operator stays at the
                // start of it, as vim's do.
                let at = match action.object {
                    Object::CharRight => range.start + after.chars().count(),
                    _ => range.start,
                };
                let changed = before != after;
                if changed {
                    buf.apply(vec![Edit::replace(range, after)], Cursor::at(at));
                } else {
                    buf.cursor = Cursor::at(at);
                }
                buf.cursor = on_a_character(buf, buf.cursor);
                changed
            }

            Verb::Join => self.join(action, count, mode),

            Verb::Replace => {
                let Object::Char { ch } = action.object else {
                    return false;
                };
                if visual {
                    // The object is the character to write, so the range is
                    // asked for as the selection's.
                    let selection = Action::new(Verb::Replace, Object::Selection, 1);
                    let Some((range, _)) = self.span_for(&selection, 1, mode) else {
                        return false;
                    };
                    let buf = self.buffer_mut();
                    let text: String = buf
                        .slice(range)
                        .chars()
                        .map(|c| if c == '\n' { c } else { ch })
                        .collect();
                    buf.apply(vec![Edit::replace(range, text)], Cursor::at(range.start));
                    return true;
                }
                let buf = self.buffer_mut();
                let at = buf.cursor.offset;
                // As in vim, `5rx` with fewer than five characters left on the
                // line does nothing rather than reaching into the next one.
                if at + count > buf.line_end() {
                    return false;
                }
                buf.apply(
                    vec![Edit::replace(
                        Range::new(at, at + count),
                        ch.to_string().repeat(count),
                    )],
                    Cursor::at(at + count - 1),
                );
                true
            }

            Verb::Insert => {
                let unit = self.indent.clone();
                let buf = self.buffer_mut();
                match action.object {
                    Object::After => {
                        let at = (buf.cursor.offset + 1).min(buf.line_end());
                        buf.cursor = Cursor::at(at);
                    }
                    Object::LineStart | Object::FirstNonBlank => {
                        let at = buf.first_non_blank();
                        buf.cursor = Cursor::at(at);
                    }
                    Object::LineEnd => {
                        let at = buf.line_end();
                        buf.cursor = Cursor::at(at);
                    }
                    // At the indentation of the line it opens beside — a level
                    // in below a line that opens a block, and above one that
                    // closes it — with that line's own line ending.
                    Object::LineBelow => {
                        let here = buf.position(buf.cursor.offset).line;
                        let line = buf.line(here);
                        let extension = extension(&buf.name());
                        let mut lead = crate::typing::lead(&line).to_string();
                        if crate::typing::opens(&line, &extension) {
                            lead.push_str(&unit);
                        }
                        let text = format!("{}{lead}", crate::typing::eol(buf, here));
                        let at = buf.line_end();
                        let cursor = at + text.chars().count();
                        buf.apply(vec![Edit::insert(at, text)], Cursor::at(cursor));
                        return true;
                    }
                    Object::LineAbove => {
                        let here = buf.position(buf.cursor.offset).line;
                        let line = buf.line(here);
                        let mut lead = crate::typing::lead(&line).to_string();
                        if matches!(line.trim_start().chars().next(), Some('}' | ']' | ')')) {
                            lead.push_str(&unit);
                        }
                        let text = format!("{lead}{}", crate::typing::eol(buf, here));
                        let at = buf.line_start();
                        let cursor = at + lead.chars().count();
                        buf.apply(vec![Edit::insert(at, text)], Cursor::at(cursor));
                        return true;
                    }
                    Object::BlockStart | Object::BlockEnd => {
                        self.begin_block_insert(action.object == Object::BlockEnd);
                        return false;
                    }
                    _ => {}
                }
                false
            }

            Verb::Paste => {
                let reg = self.read_register(action.register.as_deref());
                if reg.text.is_empty() {
                    return false;
                }
                if reg.shape == Shape::Blockwise {
                    return self.paste_block(&reg.text, action.object == Object::Here, count);
                }
                let buf = self.buffer_mut();
                let (at, cursor) = if reg.shape == Shape::Linewise {
                    // Linewise put goes to a new line, above for `P`.
                    let at = if action.object == Object::Here {
                        buf.line_start()
                    } else {
                        let end = buf.line_end();
                        (end + 1).min(buf.len_chars())
                    };
                    (at, at)
                } else {
                    let at = if action.object == Object::Here {
                        buf.cursor.offset
                    } else {
                        (buf.cursor.offset + 1).min(buf.line_end().max(buf.cursor.offset))
                    };
                    (at, at + reg.text.chars().count().saturating_sub(1))
                };
                let text = reg.text.repeat(count);
                buf.apply(vec![Edit::insert(at, text)], Cursor::at(cursor));
                true
            }

            Verb::Undo => {
                let mut changed = false;
                for _ in 0..count {
                    changed |= self.buffer_mut().undo();
                }
                changed
            }

            Verb::Redo => {
                let mut changed = false;
                for _ in 0..count {
                    changed |= self.buffer_mut().redo();
                }
                changed
            }

            // `.` never reaches here: the engine expands it into the steps it
            // stands for, so the editor only ever sees real verbs.
            Verb::Repeat => false,

            Verb::SetMark => {
                if let Object::Mark { name } = action.object {
                    let buf = self.buffer_mut();
                    let at = buf.cursor.offset;
                    buf.set_mark(name, at);
                }
                false
            }

            // `gd` and `gr` are answered by a language server and land in a
            // file this editor may not have open yet; `K` is answered beside
            // the cursor and touches nothing at all. All three belong to the
            // workspace, so reaching here means nobody had an answer — a
            // no-op rather than a failure.
            Verb::Goto | Verb::Explain => false,
        }
    }

    /// A character typed in insert mode.
    pub fn insert_char(&mut self, c: char) {
        let buf = self.buffer_mut();
        let at = buf.cursor.offset;
        buf.apply(vec![Edit::insert(at, c.to_string())], Cursor::at(at + 1));
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) -> bool {
        let buf = self.buffer_mut();
        let at = buf.cursor.offset;
        if at == 0 {
            return false;
        }
        buf.apply(
            vec![Edit::delete(Range::new(at - 1, at))],
            Cursor::at(at - 1),
        );
        true
    }

    /* ── helpers ──────────────────────────────────────────────────── */

    /// The range an operator applies to, and whether it is linewise.
    fn span_for(&mut self, action: &Action, count: usize, mode: VimMode) -> Option<(Range, bool)> {
        let visual = matches!(
            mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        );
        let buf = self.buffer_mut();
        if visual && action.object == Object::Selection {
            let sel = buf.cursor.selection();
            // `V` selects whole lines whatever columns the ends are in: `Vd`
            // takes the lines, and `Vy` yanks them as lines, so `p` puts them
            // back as lines rather than splicing them into one.
            if mode == VimMode::VisualLine {
                let first = buf.position(sel.start).line;
                let last = buf.position(sel.end).line;
                return Some((buf.line_span(first, last - first + 1), true));
            }
            // Visual selections are inclusive of the character under the cursor.
            return Some((
                Range::new(sel.start, (sel.end + 1).min(buf.len_chars())),
                false,
            ));
        }
        // `cw` on a word changes the word and not the space after it — vim's
        // own exception, because that is what anybody typing it means.
        let on_word = buf
            .text()
            .get_char(buf.cursor.offset)
            .is_some_and(|c| !c.is_whitespace());
        let object = match (action.verb, action.object) {
            (Verb::Change, Object::WordForward) if on_word => Object::WordEnd,
            (Verb::Change, Object::BigWordForward) if on_word => Object::BigWordEnd,
            (_, object) => object,
        };
        match resolve(buf, object, count)? {
            Resolved::Linewise(r) => Some((r, true)),
            Resolved::Span(r) => Some((r, false)),
            // Some motions are linewise even though they resolve to a point:
            // `dj` takes two whole lines, not a rectangle of characters, and
            // `d'a` takes every line between here and the mark. Expanding the
            // span here rather than at each object keeps `resolve` answering
            // one question — where — and this one answering the other.
            Resolved::Motion(at) if linewise(object) => {
                let from = buf.cursor.offset.min(at);
                let to = buf.cursor.offset.max(at);
                let lines = buf.position(to).line - buf.position(from).line + 1;
                let start = buf.position(from).line;
                Some((buf.line_span(start, lines), true))
            }
            // An inclusive motion takes the character it lands on: `de`
            // deletes to the end of the word, `df)` the bracket as well.
            Resolved::Motion(at) if inclusive(object) && at >= buf.cursor.offset => Some((
                Range::new(buf.cursor.offset, (at + 1).min(buf.len_chars())),
                false,
            )),
            Resolved::Motion(at) => Some((Range::new(buf.cursor.offset, at), false)),
        }
    }

    /// The first and last line an action covers: a selection's, a count of
    /// lines from the cursor's, or every line a motion crosses.
    fn lines_for(&self, action: &Action, count: usize, mode: VimMode) -> Option<(usize, usize)> {
        let buf = self.buffer();
        let here = buf.position(buf.cursor.offset).line;
        let visual = matches!(
            mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        );
        if visual && action.object == Object::Selection {
            let sel = buf.cursor.selection();
            return Some((buf.position(sel.start).line, buf.position(sel.end).line));
        }
        if action.object == Object::Line {
            return Some((here, (here + count - 1).min(last_line(buf))));
        }
        let target = match resolve(buf, action.object, count)? {
            Resolved::Motion(at) => at,
            Resolved::Span(r) | Resolved::Linewise(r) => r.end.saturating_sub(1).max(r.start),
        };
        let there = buf.position(target).line;
        Some((here.min(there), here.max(there)))
    }

    /// `>` and `<`: every line covered gains or loses one level. Blank lines
    /// are left alone, as vim leaves them, rather than given trailing spaces.
    fn shift(&mut self, action: &Action, count: usize, mode: VimMode) -> bool {
        let Some((first, last)) = self.lines_for(action, count, mode) else {
            return false;
        };
        let unit = match self.indent.is_empty() {
            true => "    ".to_string(),
            false => self.indent.clone(),
        };
        let width = match unit.as_str() {
            "\t" => 4,
            spaces => spaces.chars().count(),
        };
        let buf = self.buffer_mut();
        let mut edits = Vec::new();
        for line in first..=last {
            let text = buf.line(line);
            if text.trim().is_empty() {
                continue;
            }
            let at = buf.offset_of(Position { line, column: 0 });
            if action.verb == Verb::Indent {
                edits.push(Edit::insert(at, unit.clone()));
                continue;
            }
            let take = match text.chars().next() {
                Some('\t') => 1,
                _ => text.chars().take(width).take_while(|c| *c == ' ').count(),
            };
            if take > 0 {
                edits.push(Edit::delete(Range::new(at, at + take)));
            }
        }
        if edits.is_empty() {
            return false;
        }
        // On the first non-blank of the first line, once it has moved.
        let start = buf.offset_of(Position {
            line: first,
            column: 0,
        });
        let blank = buf
            .line(first)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        let shifted = match edits.first() {
            Some(edit) if edit.start == start && action.verb == Verb::Indent => blank + unit.chars().count(),
            Some(edit) if edit.start == start => blank - (edit.end - edit.start),
            _ => blank,
        };
        buf.apply(edits, Cursor::at(start + shifted));
        true
    }

    /// `J`: the lines become one, the break and the next line's indentation
    /// replaced by a single space — none before a closing bracket, or where
    /// the next line is blank.
    fn join(&mut self, action: &Action, count: usize, mode: VimMode) -> bool {
        let visual = matches!(
            mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        );
        let buf = self.buffer();
        let here = buf.position(buf.cursor.offset).line;
        let (first, last) = match visual {
            true => self.lines_for(action, count, mode).unwrap_or((here, here)),
            // `J` joins two lines; `3J` joins three.
            false => (here, here + count.max(2) - 1),
        };
        let last = last.max(first + 1).min(last_line(buf));
        if first >= last {
            return false;
        }
        let mut edits = Vec::new();
        let mut joined_at = None;
        for line in first..last {
            let end = buf.offset_of(Position { line, column: 0 }) + buf.line(line).chars().count();
            let next = buf.line(line + 1);
            let lead = next.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            let rest = &next[next.char_indices().nth(lead).map_or(next.len(), |(i, _)| i)..];
            let space = match (buf.line(line).ends_with(' '), rest.is_empty(), rest.starts_with(')')) {
                (false, false, false) => " ",
                _ => "",
            };
            joined_at.get_or_insert(end);
            edits.push(Edit::replace(Range::new(end, end + 1 + lead), space.to_string()));
        }
        let at = joined_at.unwrap_or(0);
        let buf = self.buffer_mut();
        buf.apply(edits, Cursor::at(at));
        true
    }

    fn write_register(&mut self, name: Option<&str>, text: String, shape: Shape) {
        // The system clipboard gets what the unnamed register does, unless a
        // register of Quark's own (`"a`…) was named — `unnamedplus`.
        if matches!(name, None | Some("+") | Some("*")) && !text.is_empty() {
            self.clipboard = Some(text.clone());
        }
        let reg = Register { text, shape };
        // The unnamed register always gets a copy, exactly as in vim.
        self.registers.insert("\"".into(), reg.clone());
        if let Some(name) = name {
            self.registers.insert(name.to_string(), reg);
        }
    }

    /// What the last yank or delete left for the system clipboard, once.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    fn read_register(&self, name: Option<&str>) -> Register {
        self.registers
            .get(name.unwrap_or("\""))
            .cloned()
            .unwrap_or_default()
    }
}

/// How `open` names a file: canonicalised so that two spellings of one file
/// cannot become two buffers, then un-prefixed, because on Windows
/// canonicalising hands back `\\?\C:\...` — which is the same file, but is
/// neither what a statusline should show nor a spelling any language server
/// accepts. The display path and the LSP path want the same normalisation, so
/// they get it in the same place. As given for a file that is not there yet.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .map(crate::root::plain)
        .unwrap_or_else(|_| PathBuf::from(path))
}

/// Turn an object into something a verb can act on.
fn resolve(buf: &Buffer, object: Object, count: usize) -> Option<Resolved> {
    use Object::*;
    Some(match object {
        Line => Resolved::Linewise(buf.line_range(count)),
        LineDown => Resolved::Motion(buf.line_down(count)),
        LineUp => Resolved::Motion(buf.line_up(count)),
        CharLeft => Resolved::Motion(buf.cursor.offset.saturating_sub(count)),
        CharRight => Resolved::Motion((buf.cursor.offset + count).min(buf.line_end())),
        WordForward => Resolved::Motion(buf.word_forward(count, false)),
        WordBackward => Resolved::Motion(buf.word_backward(count, false)),
        WordEnd => Resolved::Motion(buf.word_end(count, false)),
        BigWordForward => Resolved::Motion(buf.word_forward(count, true)),
        BigWordBackward => Resolved::Motion(buf.word_backward(count, true)),
        BigWordEnd => Resolved::Motion(buf.word_end(count, true)),
        LineStart => Resolved::Motion(buf.line_start()),
        FirstNonBlank => Resolved::Motion(buf.first_non_blank()),
        LineEnd => Resolved::Motion(buf.line_end()),
        FileStart => Resolved::Motion(buf.file_start()),
        FileEnd => Resolved::Motion(buf.file_end()),
        Selection => Resolved::Span(buf.cursor.selection()),
        InnerWord => Resolved::Span(buf.inner_word(false)),
        AroundWord => Resolved::Span(buf.a_word(false)),
        InnerBigWord => Resolved::Span(buf.inner_word(true)),
        AroundBigWord => Resolved::Span(buf.a_word(true)),
        FindChar { ch, forward, till } => Resolved::Motion(find_in_line(buf, ch, forward, till, count)?),
        MatchPair => Resolved::Motion(match_pair(buf)?),
        ParagraphForward => Resolved::Motion(paragraph(buf, true, count)),
        ParagraphBackward => Resolved::Motion(paragraph(buf, false, count)),
        LineNumber { line } => {
            let line = (line as usize).saturating_sub(1).min(last_line(buf));
            let start = buf.offset_of(Position { line, column: 0 });
            Resolved::Motion(motion::first_non_blank(buf.text().slice(..), start))
        }
        InnerPair { open } => Resolved::Span(buf.inside_pair(open, closing(open), false)?),
        AroundPair { open } => Resolved::Span(buf.inside_pair(open, closing(open), true)?),
        MarkExact { name } => Resolved::Motion(buf.mark(name)?),
        // `'a` lands on the first non-blank of the mark's line, the way `'`
        // always does — a mark remembers a place, and `'` asks for the line
        // it is on rather than the column.
        MarkLine { name } => {
            let at = buf.mark(name)?;
            Resolved::Motion(motion::first_non_blank(buf.text().slice(..), at))
        }
        // `Mark` is where to *put* one, which is the cursor — nothing to
        // resolve. `Definition` is a question for a language server, answered
        // in the workspace where one lives.
        // A block is not one range, so it never resolves — it goes down its
        // own path in `execute`. `Mark` is where to *put* a mark, which is
        // the cursor. `Definition` is a question for a language server,
        // answered in the workspace where one lives.
        BlockStart
        | BlockEnd
        | Char { .. }
        | Mark { .. }
        | Definition
        | References
        | None
        | Here
        | After
        | LineBelow
        | LineAbove => return Option::None,
    })
}

fn closing(open: char) -> char {
    match open {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        '<' => '>',
        other => other,
    }
}

/// Whether a motion takes whole lines when an operator is waiting on it.
///
/// `dj` deletes two lines rather than a rectangle of characters, and `d'a`
/// takes every line between here and the mark. The objects themselves resolve
/// to a point either way — this is only about what an operator does with it.
fn linewise(object: Object) -> bool {
    matches!(
        object,
        Object::LineDown
            | Object::LineUp
            | Object::MarkLine { .. }
            // `dG`, `dgg` and `d5G` take whole lines, as in vim.
            | Object::FileEnd
            | Object::FileStart
            | Object::LineNumber { .. }
    )
}

/// Whether a motion includes the character it lands on, when an operator is
/// waiting on it. `e`, `f` and `t` do and `w` does not — `dw` stops before the
/// next word, `de` takes the end of this one.
fn inclusive(object: Object) -> bool {
    matches!(
        object,
        Object::WordEnd
            | Object::BigWordEnd
            | Object::MatchPair
            | Object::FindChar { forward: true, .. }
    )
}

/// The last line with anything on it. A file ending in a line break has an
/// empty line after it as far as the rope is concerned, and nothing should
/// land on or join with that.
fn last_line(buf: &Buffer) -> usize {
    let lines = buf.len_lines().max(1);
    match buf.text().len_chars() > 0 && buf.text().char(buf.text().len_chars() - 1) == '\n' {
        true => lines.saturating_sub(2),
        false => lines - 1,
    }
}

/// The cursor, moved back off a line break onto the line's last character —
/// where normal mode keeps it. An empty line has only the break to sit on.
fn on_a_character(buf: &Buffer, cursor: Cursor) -> Cursor {
    let text = buf.text().slice(..);
    let start = motion::line_start(text, cursor.offset);
    let end = motion::line_end(text, cursor.offset);
    if cursor.offset >= end && end > start {
        Cursor {
            offset: end - 1,
            anchor: end - 1,
            goal_column: cursor.goal_column,
        }
    } else {
        cursor
    }
}

/// `f`, `t`, `F`, `T`: the `count`th `ch` along the cursor's line.
fn find_in_line(buf: &Buffer, ch: char, forward: bool, till: bool, count: usize) -> Option<usize> {
    let text = buf.text();
    let (at, start, end) = (buf.cursor.offset, buf.line_start(), buf.line_end());
    let mut found = 0;
    if forward {
        for i in at + 1..end {
            if text.char(i) == ch {
                found += 1;
                if found == count {
                    return Some(if till { i - 1 } else { i });
                }
            }
        }
    } else {
        for i in (start..at).rev() {
            if text.char(i) == ch {
                found += 1;
                if found == count {
                    return Some(if till { i + 1 } else { i });
                }
            }
        }
    }
    None
}

/// `%`: the bracket matching the one under the cursor, or the first one after
/// it on the line, counting nested pairs on the way.
fn match_pair(buf: &Buffer) -> Option<usize> {
    let text = buf.text();
    let (at, end) = (buf.cursor.offset, buf.line_end());
    let (pos, c) = (at..end)
        .map(|i| (i, text.char(i)))
        .find(|(_, c)| "(){}[]".contains(*c))?;
    let (open, close, forward) = match c {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        '[' => ('[', ']', true),
        _ => ('[', ']', false),
    };
    let mut depth = 0usize;
    if forward {
        for (i, ch) in text.chars_at(pos).enumerate() {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(pos + i);
                }
            }
        }
    } else {
        let mut chars = text.chars_at(pos + 1);
        let mut i = pos + 1;
        while let Some(ch) = chars.prev() {
            i -= 1;
            if ch == close {
                depth += 1;
            } else if ch == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// `}` and `{`: past the paragraph the cursor is in to the blank line beyond
/// it, `count` times; the end or start of the file when there is none.
fn paragraph(buf: &Buffer, forward: bool, count: usize) -> usize {
    let last = last_line(buf);
    let blank = |line: usize| buf.line(line).trim().is_empty();
    let mut line = buf.position(buf.cursor.offset).line;
    for _ in 0..count {
        if forward {
            while line < last && blank(line) {
                line += 1;
            }
            while line < last && !blank(line) {
                line += 1;
            }
        } else {
            while line > 0 && blank(line) {
                line -= 1;
            }
            while line > 0 && !blank(line) {
                line -= 1;
            }
        }
    }
    let start = buf.offset_of(Position { line, column: 0 });
    match forward && line == last && !blank(line) {
        // No blank line ahead: the end of the last line.
        true => start + buf.line(line).chars().count().saturating_sub(1),
        false => start,
    }
}

/* ── visual block ─────────────────────────────────────────────────── */

impl Editor {
    /// Delete, change or yank a rectangle.
    ///
    /// Separate from `span_for` because a block is several ranges and the
    /// operators are built around one. They are applied as a single change,
    /// so one `u` takes the whole rectangle back.
    fn block_operator(&mut self, action: &Action) -> bool {
        let block = Block::of(self.buffer());
        let ranges = block.ranges(self.buffer());

        // Every row, including the ones too short to reach the block. Dropping
        // the empty ones would lose the shape, and a block pasted back without
        // its blank rows comes out crooked.
        let rows: Vec<String> = ranges
            .iter()
            .map(|range| self.buffer().slice(*range))
            .collect();
        self.write_register(
            action.register.as_deref(),
            rows.join("\n"),
            Shape::Blockwise,
        );

        if action.verb == Verb::Yank {
            let buf = self.buffer_mut();
            let at = block.corner(buf);
            buf.cursor = buf.clamp(buf.cursor.moved_to(at));
            return false;
        }

        let edits: Vec<Edit> = ranges
            .iter()
            .filter(|range| !range.is_empty())
            .map(|range| Edit::delete(*range))
            .collect();
        if edits.is_empty() {
            return false;
        }

        let buf = self.buffer_mut();
        let at = block.corner(buf);
        buf.apply(edits, Cursor::at(at));
        true
    }

    /// Start a block insert: put the cursor on the first line's edge and
    /// remember the rest for when insert mode ends.
    fn begin_block_insert(&mut self, at_end: bool) {
        let block = Block::of(self.buffer());
        let buf = self.buffer_mut();

        let column = if at_end { block.right - 1 } else { block.left };
        let lines: Vec<usize> = block
            .lines()
            .skip(1)
            .filter(|line| *line < buf.len_lines())
            .collect();

        let first = buf.offset_of(quark_text::Position {
            line: block.first_line,
            column,
        });
        // `A` on a line shorter than the block's right edge appends at its
        // end; the padding that gets it to the column happens on commit, when
        // it is known how much is needed.
        buf.cursor = Cursor::at(first);

        self.block_insert = Some(PendingInsert {
            lines,
            column,
            from: first,
            pad: at_end,
        });
    }

    /// Insert mode is ending — copy what was typed onto the rest of the block.
    ///
    /// Read back out of the buffer rather than accumulated as it was typed,
    /// because backspace, a completion and anything else that edits during
    /// insert all have to be reflected, and only the buffer knows all of them.
    pub fn finish_block_insert(&mut self) -> bool {
        let Some(pending) = self.block_insert.take() else {
            return false;
        };
        let buf = self.buffer_mut();
        let to = buf.cursor.offset.max(pending.from).min(buf.len_chars());
        let typed = buf.slice(Range::new(pending.from, to));

        // Nothing typed, or the typing left the line it started on: either way
        // there is nothing sensible to replicate.
        if typed.is_empty() || typed.contains('\n') {
            return false;
        }

        let mut edits = Vec::new();
        for line in pending.lines {
            let width = buf.line(line).chars().count();
            let start = buf.offset_of(quark_text::Position { line, column: 0 });

            if pending.column <= width {
                edits.push(Edit::insert(start + pending.column, &typed));
            } else if pending.pad {
                // `A` reaches past the end of a short line by padding it out.
                // `I` does not — a column that line never had is not a place
                // to insert, so it is skipped.
                let padded = format!("{}{typed}", " ".repeat(pending.column - width));
                edits.push(Edit::insert(start + width, &padded));
            }
        }

        if edits.is_empty() {
            return false;
        }
        let cursor = buf.cursor;
        buf.apply(edits, cursor);
        true
    }
}

impl Editor {
    /// Put a rectangle back: each row at the same column on successive lines.
    ///
    /// Short lines are padded to reach the column, because the point of a
    /// block paste is that the result is a column — a row that quietly landed
    /// at the end of a short line instead would make it not one.
    fn paste_block(&mut self, text: &str, before: bool, count: usize) -> bool {
        let buf = self.buffer_mut();
        let start = buf.position(buf.cursor.offset);
        let column = if before {
            start.column
        } else {
            start.column + 1
        };

        let mut edits = Vec::new();
        let mut tail = String::new();

        for (offset, row) in text.split('\n').enumerate() {
            let row = row.repeat(count);
            let line = start.line + offset;
            if line >= buf.len_lines() {
                // Past the end of the buffer: collect the rest into new lines
                // rather than dropping them.
                tail.push('\n');
                tail.push_str(&" ".repeat(column));
                tail.push_str(&row);
                continue;
            }
            let width = buf.line(line).chars().count();
            let at = buf.offset_of(quark_text::Position { line, column: 0 });
            if column <= width {
                edits.push(Edit::insert(at + column, row));
            } else {
                edits.push(Edit::insert(
                    at + width,
                    format!("{}{row}", " ".repeat(column - width)),
                ));
            }
        }

        if !tail.is_empty() {
            let end = buf.len_chars();
            // A file ending in a newline has an empty last line that starts
            // exactly at the end, so a row landing on it and the overflow are
            // two inserts at one offset — which is not an order, it is a race.
            // Merging is what that order would have meant anyway.
            match edits.last_mut().filter(|edit| edit.start == end) {
                Some(edit) => edit.text.push_str(&tail),
                None => edits.push(Edit::insert(end, tail)),
            }
        }
        if edits.is_empty() {
            return false;
        }

        let landing = buf.offset_of(quark_text::Position {
            line: start.line,
            column,
        });
        buf.apply(edits, Cursor::at(landing));
        true
    }
}

/// A file name's extension, as the typing rules know languages by.
fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default()
}
