//! One editor: the buffers of a project, the modal engine reading its keys,
//! the splits onto them — and what joins them, which in Proton is the
//! workspace's and here is this: what typing does (pairs, indentation),
//! search, the `:` line, page keys, `gv`, splits.
//!
//! A key goes in; what it did comes out as things to say. Nothing here
//! draws: the window asks for the text, the cursor and each split's lines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use quark_text::{Cursor, Edit, Range};

use crate::action::{Action, Object, Verb};
use crate::editor::Editor;
use crate::ex::Ex;
use crate::key::Key;
use crate::root::Root;
use crate::split::{Direction, Splits};
use crate::typing;
use crate::vim::{Outcome, Vim, VimMode};

/// How many lines are kept between the cursor and the top or bottom of the
/// view, so what follows it is always on screen.
const SCROLL_OFF: usize = 3;

/// What a key did, to say: a note, or a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    Info(String),
    Warn(String),
    /// A file was written.
    Wrote(PathBuf),
    /// A leader sequence the editor does not know, for the window to try.
    Leader(String),
    /// A `:` line the editor does not know, for the window to try.
    Ex(String),
}

/// A search remembered for `n` and `N`.
#[derive(Debug, Clone)]
struct Search {
    pattern: String,
    forward: bool,
}

pub struct Session {
    editor: Editor,
    vim: Vim,
    splits: Splits,
    root: Option<Root>,
    search: Option<Search>,
    /// Where the cursor was when a search prompt opened, so a search that
    /// moved it as it was typed can put it back.
    search_origin: Option<Cursor>,
    /// Whether matches of the last search are painted (`:noh` stops it).
    pub highlight: bool,
    /// The last selection, for `gv`: buffer, anchor, head, kind.
    last_visual: Option<(usize, usize, usize, VimMode)>,
    editorconfigs: HashMap<PathBuf, typing::Config>,
    /// How wide a tab stop is, when nothing else says.
    pub tab_size: usize,
}

impl Default for Session {
    fn default() -> Self {
        Session::new(None)
    }
}

impl Session {
    /// An editor for the files of a project in `root`, or of none.
    pub fn new(root: Option<&Path>) -> Session {
        let root = root.map(Root::new);
        Session {
            editor: root.clone().map_or_else(Editor::new, Editor::rooted),
            vim: Vim::new(),
            splits: Splits::default(),
            root,
            search: None,
            search_origin: None,
            highlight: true,
            last_visual: None,
            editorconfigs: HashMap::new(),
            tab_size: 4,
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The editor, to change: what a language server's completion puts in.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    pub fn mode(&self) -> VimMode {
        self.vim.mode()
    }

    pub fn vim(&self) -> &Vim {
        &self.vim
    }

    pub fn splits(&self) -> &Splits {
        &self.splits
    }

    /// The pattern painted, when matches are.
    pub fn pattern(&self) -> Option<&str> {
        let typing = matches!(self.vim.prompt(), '/' | '?') && self.vim.mode() == VimMode::Command;
        if typing {
            return Some(self.vim.cmdline()).filter(|p| !p.is_empty());
        }
        self.search.as_ref().filter(|_| self.highlight).map(|s| s.pattern.as_str())
    }

    /// A file of the project opened in the split in focus — or found, when
    /// it is open already.
    pub fn open(&mut self, path: &Path) -> std::io::Result<()> {
        self.park_cursor();
        let index = self.editor.open(path)?;
        self.editor.focus(index);
        self.splits.view_mut().buffer = index;
        let buf = self.editor.buffer_mut();
        buf.cursor = buf.clamp(buf.cursor);
        self.follow();
        Ok(())
    }

    /// Text put in at the cursor as ⌘V puts it: typed in, in insert mode;
    /// before the cursor in normal mode, on the line above when it is whole
    /// lines; in place of a selection. One undo step.
    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let before = self.vim.mode();
        let visual = matches!(before, VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock);
        if text.is_empty() || (!visual && !matches!(before, VimMode::Insert | VimMode::Replace | VimMode::Normal)) {
            return;
        }
        let buf = self.editor.buffer_mut();
        buf.end_group();
        buf.begin_group(false);
        if visual {
            let held = (self.editor.active(), self.editor.buffer().cursor);
            self.act(&Action::new(Verb::Delete, Object::Selection, 1), before, false);
            self.vim.keys(&Key::named("Escape"));
            self.last_visual = Some((held.0, held.1.anchor, held.1.offset, before));
        }
        let buf = self.editor.buffer_mut();
        let n = text.chars().count();
        let (at, cursor) = match before {
            VimMode::Insert | VimMode::Replace => (buf.cursor.offset, buf.cursor.offset + n),
            _ if text.ends_with('\n') && !visual => (buf.line_start(), buf.line_start()),
            _ => (buf.cursor.offset, buf.cursor.offset + n.saturating_sub(1)),
        };
        buf.apply(vec![Edit::insert(at, text)], Cursor::at(cursor));
        buf.end_group();
        self.follow();
    }

    /// What `y` and `d` left for the system clipboard, if anything.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.editor.take_clipboard()
    }

    /// Files with changes not written.
    pub fn unsaved(&self) -> Vec<String> {
        self.editor.unsaved().iter().map(|b| b.name()).collect()
    }

    /// Write every file with changes; what could not be written, said.
    pub fn save_all(&mut self) -> Vec<Said> {
        self.editor
            .save_unsaved()
            .into_iter()
            .map(|(name, why)| Said::Warn(format!("{name}: {why}")))
            .collect()
    }

    /// How many lines a split can show, as the window has it, and the cursor
    /// kept in it.
    pub fn set_lines(&mut self, view: usize, lines: usize) {
        if let Some(v) = self.splits.at_mut(view) {
            v.lines = lines.max(1);
        }
        if view == self.splits.focused() {
            self.follow();
        }
    }

    /// Scroll a split by `by` lines, as the wheel does, the cursor kept on
    /// screen.
    pub fn scroll(&mut self, view: usize, by: i64) {
        if by == 0 {
            return;
        }
        let total = self.buffer_lines(view);
        let focused = view == self.splits.focused();
        let Some(v) = self.splits.at_mut(view) else { return };
        let max = total.saturating_sub(1);
        v.first_line = (v.first_line as i64 + by).clamp(0, max as i64) as usize;
        if focused {
            // The cursor follows the view, as vim's does.
            let (first, lines) = (v.first_line, v.lines);
            let buf = self.editor.buffer_mut();
            let line = buf.position(buf.cursor.offset).line;
            // The margin is kept from an edge of the view, not of the file.
            let off = SCROLL_OFF.min(lines / 2);
            let low = if first == 0 { 0 } else { first + off };
            let high = if first + lines >= total { max } else { (first + lines).saturating_sub(1 + off) };
            let want = line.clamp(low.min(high), high.max(low)).min(max);
            if want != line {
                let at = buf.offset_of(quark_text::Position { line: want, column: 0 });
                buf.cursor = buf.clamp(Cursor::at(at));
            }
        }
    }

    /// Put the cursor where a click put it, in the split clicked. The mode
    /// is left alone: a click in insert mode is somewhere to type.
    pub fn click(&mut self, view: usize, line: usize, column: usize) {
        if view != self.splits.focused() {
            self.focus_split(view);
        }
        // Typing after a click is another change from the typing before it.
        if matches!(self.vim.mode(), VimMode::Insert | VimMode::Replace) {
            self.editor.buffer_mut().end_group();
        }
        let buf = self.editor.buffer_mut();
        // Clamped twice over: the line against the document, then the offset
        // against the line — a click past the end of a short line is usual.
        let line = line.min(buf.len_lines().saturating_sub(1));
        let at = buf.offset_of(quark_text::Position { line, column });
        buf.cursor = buf.clamp(buf.cursor.moved_to(at));
        self.follow();
    }

    /// A drag from where the cursor is to here: a visual selection. A drag
    /// that has gone nowhere is a click.
    pub fn select_to(&mut self, line: usize, column: usize) {
        let was = self.vim.mode();
        let buf = self.editor.buffer_mut();
        let line = line.min(buf.len_lines().saturating_sub(1));
        let at = buf.offset_of(quark_text::Position { line, column });
        let visual = matches!(was, VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock);
        if !visual && buf.clamp(buf.cursor.moved_to(at)).offset == buf.cursor.offset {
            return;
        }
        if matches!(was, VimMode::Insert | VimMode::Replace) {
            self.editor.buffer_mut().end_group();
        }
        self.vim.enter_visual();
        let buf = self.editor.buffer_mut();
        buf.cursor = buf.clamp(buf.cursor.extended_to(at));
        self.follow();
    }

    /// An action, as if its keys were typed in normal mode.
    pub fn perform(&mut self, action: &Action) {
        let was = self.vim.mode();
        self.act(action, was, false);
        self.follow();
    }

    /// Close buffer `index`; unless forced, not with unsaved changes.
    pub fn close_buffer(&mut self, index: usize, force: bool) -> Vec<Said> {
        if index >= self.editor.buffers().len() {
            return vec![Said::Warn(format!("no buffer {index}"))];
        }
        self.park_cursor();
        self.editor.focus(index);
        let said = self.ex_quit(&Ex { name: "bdelete".into(), bang: force, arg: None });
        self.restore_cursor();
        said
    }

    /// Focus split `index`.
    pub fn focus_split(&mut self, index: usize) {
        if index == self.splits.focused() {
            return;
        }
        self.park_cursor();
        if !self.splits.focus(index) {
            self.restore_cursor();
            return;
        }
        self.restore_cursor();
    }

    /// The buffer a split shows, and its cursor: the live one for the split
    /// in focus.
    pub fn view_of(&self, view: usize) -> Option<(usize, Cursor, usize)> {
        let v = self.splits.at(view)?;
        if view == self.splits.focused() {
            Some((self.editor.active(), self.editor.buffer().cursor, v.first_line))
        } else {
            Some((v.buffer, v.cursor, v.first_line))
        }
    }

    fn buffer_lines(&self, view: usize) -> usize {
        let index = self.view_of(view).map_or(0, |v| v.0);
        self.editor.buffers().get(index).map_or(1, |b| b.len_lines())
    }

    /// One key, and what it did to say.
    pub fn key(&mut self, key: &Key) -> Vec<Said> {
        let before = self.vim.mode();
        let held = matches!(before, VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock)
            .then(|| (self.editor.active(), self.editor.buffer().cursor));
        let mut said = Vec::new();

        // Insert-mode keys that are text entry rather than commands.
        if matches!(before, VimMode::Insert | VimMode::Replace) && !key.ctrl && !key.meta {
            match key.key.as_str() {
                "Enter" => {
                    self.editor.buffer_mut().ensure_group();
                    self.enter();
                }
                "Backspace" => {
                    self.editor.buffer_mut().ensure_group();
                    self.backspace();
                }
                _ => {}
            }
        }

        let outcomes = self.vim.keys(key);
        // Only dot and macro replay fan out from one key: one undo step.
        let replay = outcomes.len() > 1;
        if replay {
            self.editor.buffer_mut().begin_group(false);
        }
        for outcome in outcomes {
            said.extend(self.outcome(outcome, before, replay));
        }
        if replay {
            self.editor.buffer_mut().end_group();
        }

        if self.vim.mode() != before {
            let visual = |m: VimMode| matches!(m, VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock);
            // Leaving visual mode lets go of the selection, kept for `gv`.
            if visual(before) && !visual(self.vim.mode()) {
                if let Some((buffer, cursor)) = held {
                    self.last_visual = Some((buffer, cursor.anchor, cursor.offset, before));
                }
                let buf = self.editor.buffer_mut();
                let at = buf.cursor.offset;
                buf.cursor = buf.cursor.moved_to(at);
            }
            // Leaving insert mode steps back one, onto the character typed.
            if matches!(before, VimMode::Insert | VimMode::Replace) && self.vim.mode() == VimMode::Normal {
                self.editor.finish_block_insert();
                let buf = self.editor.buffer_mut();
                buf.end_group();
                let at = buf.cursor.offset;
                let floor = buf.line_start();
                buf.cursor = buf.clamp(buf.cursor.moved_to(at.saturating_sub(1).max(floor)));
            }
        }
        self.follow();
        said
    }

    fn outcome(&mut self, outcome: Outcome, was: VimMode, replay: bool) -> Vec<Said> {
        match outcome {
            Outcome::Action(action) => {
                self.act(&action, was, replay);
                Vec::new()
            }
            Outcome::Page { down, half, count } => {
                let lines = self.splits.view().lines.max(1);
                let step = if half { (lines / 2).max(1) } else { lines.saturating_sub(2).max(1) } * count.max(1) as usize;
                let object = if down { Object::LineDown } else { Object::LineUp };
                self.act(&Action::new(Verb::Move, object, step as u32), was, replay);
                Vec::new()
            }
            Outcome::Reselect => {
                if let Some((buffer, anchor, offset, kind)) = self.last_visual {
                    if buffer == self.editor.active() {
                        let buf = self.editor.buffer_mut();
                        let len = buf.len_chars();
                        buf.cursor = buf.clamp(Cursor { offset: offset.min(len), anchor: anchor.min(len), goal_column: None });
                        self.vim.reselect(kind);
                    }
                }
                Vec::new()
            }
            Outcome::Indent => {
                let indent = self.indent_here();
                self.editor.buffer_mut().ensure_group();
                for c in indent.chars() {
                    self.editor.insert_char(c);
                }
                Vec::new()
            }
            Outcome::Input(c) => {
                self.editor.buffer_mut().ensure_group();
                match was {
                    VimMode::Insert => self.type_char(c),
                    _ => self.editor.insert_char(c),
                }
                Vec::new()
            }
            Outcome::Pending(_) => Vec::new(),
            Outcome::Leader(prefix) => match window_step(&prefix) {
                Some(step) => {
                    self.vim.cancel_leader();
                    self.window(step)
                }
                None if prefix.starts_with('w') && prefix.len() == 2 => {
                    self.vim.cancel_leader();
                    Vec::new()
                }
                None => vec![Said::Leader(prefix)],
            },
            Outcome::Search { pattern, forward } => self.search(&pattern, forward),
            Outcome::Repeat { forward } => self.search_again(forward),
            Outcome::SearchWord { forward } => {
                let buffer = self.editor.buffer();
                match buffer.word_at(buffer.cursor.offset) {
                    Some(word) => self.search(&word, forward),
                    None => vec![Said::Warn("no word under the cursor".into())],
                }
            }
            Outcome::Cmdline(text) => {
                let prompt = self.vim.prompt();
                match (&text, prompt) {
                    (None, _) => self.abandon_search(),
                    (Some(pattern), '/' | '?') => self.preview_search(pattern, prompt == '/'),
                    _ => {}
                }
                Vec::new()
            }
            Outcome::Execute(text) => self.run_ex(text),
            Outcome::Ignored => Vec::new(),
        }
    }

    fn act(&mut self, action: &Action, was: VimMode, replay: bool) {
        if !replay && action.verb == Verb::Move && matches!(was, VimMode::Insert | VimMode::Replace) {
            self.editor.buffer_mut().end_group();
        }
        // `>` and `<` shift by the file's own indent, and `o` and `O` open a
        // line a level in where the code calls for it.
        if matches!(action.verb, Verb::Indent | Verb::Outdent)
            || (action.verb == Verb::Insert && matches!(action.object, Object::LineBelow | Object::LineAbove))
        {
            let unit = self.indent().unit();
            self.editor.set_indent(&unit);
        }
        let changed = self.editor.execute(action, was);
        // An action that opens insert mode starts an undo group, so the typing
        // that follows is part of the same change.
        if !replay && matches!(action.verb, Verb::Change | Verb::Insert) && self.vim.mode() == VimMode::Insert {
            self.editor.buffer_mut().begin_group(changed);
        }
    }

    /* ── typing ── */

    fn indent(&mut self) -> typing::Indent {
        let fallback = self.tab_size;
        let buffer = self.editor.buffer();
        let (path, name, lines) = (buffer.path().map(Path::to_path_buf), buffer.name(), buffer.lines(0, 1000));
        let config = match path {
            Some(path) => *self.editorconfigs.entry(path.clone()).or_insert_with(|| typing::editorconfig(&path)),
            None => typing::Config::default(),
        };
        typing::indent_of(config, &name, &lines, fallback)
    }

    /// What Tab inserts: a tab where the file is indented with tabs, else
    /// spaces to the next stop of the file's own indent.
    fn indent_here(&mut self) -> String {
        let indent = self.indent();
        if indent.tabs {
            return "\t".into();
        }
        let buffer = self.editor.buffer();
        let at = buffer.position(buffer.cursor.offset);
        let before: String = buffer.line(at.line).chars().take(at.column).collect();
        let column = typing::columns(&before, indent.tab);
        " ".repeat(indent.width - column % indent.width)
    }

    /// Enter in insert mode: a new line indented as what it starts belongs.
    fn enter(&mut self) {
        let indent = self.indent();
        let buf = self.editor.buffer();
        let at = buf.cursor.offset;
        let here = buf.position(at);
        let line = buf.line(here.line);
        let before: String = line.chars().take(here.column).collect();
        let after: String = line.chars().skip(here.column).collect();
        let eol = typing::eol(buf, here.line);
        let extension = extension_of(&buf.name());
        let (take_before, take_after, text, cursor) = typing::newline(&before, &after, &indent, &extension, eol);
        let start = at - take_before;
        self.editor
            .buffer_mut()
            .apply(vec![Edit::replace(Range::new(start, at + take_after), text)], Cursor::at(start + cursor));
    }

    /// Backspace in insert mode: both halves of an empty pair, spaces back
    /// to the last indent stop, a line ending whole, or one character.
    fn backspace(&mut self) -> bool {
        let buf = self.editor.buffer();
        let at = buf.cursor.offset;
        if at == 0 {
            return false;
        }
        let here = buf.position(at);
        let line = buf.line(here.line);
        let before: String = line.chars().take(here.column).collect();
        let (prev, next) = (before.chars().last(), line.chars().nth(here.column));
        let crlf = here.column == 0 && at >= 2 && buf.slice(Range::new(at - 2, at)) == "\r\n";
        let (from, to) = if here.column > 0 && typing::empty_pair(prev, next) {
            (at - 1, at + 1)
        } else if here.column > 0 && before.chars().all(|c| c == ' ') {
            let indent = self.indent();
            (at - typing::back_to_stop(&before, &indent), at)
        } else if crlf {
            (at - 2, at)
        } else {
            (at - 1, at)
        };
        self.editor.buffer_mut().apply(vec![Edit::delete(Range::new(from, to))], Cursor::at(from));
        true
    }

    /// A character typed in insert mode: with its partner, or over the one
    /// already there; a closing bracket after only indentation, back at its
    /// block's.
    fn type_char(&mut self, c: char) {
        let buf = self.editor.buffer();
        let at = buf.cursor.offset;
        let here = buf.position(at);
        let line = buf.line(here.line);
        let before: String = line.chars().take(here.column).collect();
        let (prev, next) = (before.chars().last(), line.chars().nth(here.column));
        let extension = extension_of(&buf.name());
        let text = match typing::typed(c, prev, next, &extension) {
            typing::Typed::Over => {
                self.editor.buffer_mut().cursor = Cursor::at(at + 1);
                return;
            }
            typing::Typed::Insert(text) => text,
        };
        if matches!(c, ')' | ']' | '}') && !before.is_empty() && before.trim().is_empty() {
            let previous = (0..here.line).rev().map(|l| buf.line(l)).find(|l| !l.trim().is_empty());
            if let Some(previous) = previous {
                let indent = self.indent();
                if let Some(target) = typing::outdented(&before, &previous, &indent, &extension) {
                    let start = at - here.column;
                    let cursor = start + target.chars().count() + 1;
                    self.editor.buffer_mut().apply(
                        vec![Edit::replace(Range::new(start, at), format!("{target}{text}"))],
                        Cursor::at(cursor),
                    );
                    return;
                }
            }
        }
        self.editor.buffer_mut().apply(vec![Edit::insert(at, text)], Cursor::at(at + 1));
    }

    /* ── search ── */

    fn preview_search(&mut self, pattern: &str, forward: bool) {
        let origin = *self.search_origin.get_or_insert(self.editor.buffer().cursor);
        let landing = if pattern.is_empty() { None } else { self.editor.buffer().find(pattern, origin.offset, forward) };
        let selecting = self.vim.is_selecting();
        let buffer = self.editor.buffer_mut();
        buffer.cursor = match landing {
            Some(at) if selecting => buffer.clamp(origin.extended_to(at)),
            Some(at) => buffer.clamp(origin.moved_to(at)),
            None => buffer.clamp(origin),
        };
    }

    fn abandon_search(&mut self) {
        if let Some(origin) = self.search_origin.take() {
            let buffer = self.editor.buffer_mut();
            buffer.cursor = buffer.clamp(origin);
        }
    }

    fn search(&mut self, pattern: &str, forward: bool) -> Vec<Said> {
        if let Some(origin) = self.search_origin.take() {
            let buffer = self.editor.buffer_mut();
            buffer.cursor = buffer.clamp(origin);
        }
        if pattern.is_empty() {
            return self.search_again(forward);
        }
        self.search = Some(Search { pattern: pattern.to_string(), forward });
        self.jump_to_match(pattern, forward)
    }

    fn search_again(&mut self, forward: bool) -> Vec<Said> {
        let Some(search) = self.search.clone() else {
            return vec![Said::Warn("no previous search".into())];
        };
        // `N` is the opposite of the search's own direction.
        let forward = if forward { search.forward } else { !search.forward };
        self.jump_to_match(&search.pattern, forward)
    }

    fn jump_to_match(&mut self, pattern: &str, forward: bool) -> Vec<Said> {
        let buffer = self.editor.buffer();
        let Some(at) = buffer.find(pattern, buffer.cursor.offset, forward) else {
            return vec![Said::Warn(format!("{pattern} not found"))];
        };
        let wrapped = if forward { at <= buffer.cursor.offset } else { at >= buffer.cursor.offset };
        let visual = self.vim.is_selecting();
        let buffer = self.editor.buffer_mut();
        buffer.cursor = buffer.clamp(if visual { buffer.cursor.extended_to(at) } else { buffer.cursor.moved_to(at) });
        self.highlight = true;
        if wrapped {
            return vec![Said::Info(format!(
                "search hit {} and continued from the {}",
                if forward { "the bottom" } else { "the top" },
                if forward { "top" } else { "bottom" }
            ))];
        }
        Vec::new()
    }

    /* ── the `:` line ── */

    fn run_ex(&mut self, line: String) -> Vec<Said> {
        // `:42` goes to line 42.
        if let Ok(n) = line.trim().trim_start_matches(':').trim().parse::<usize>() {
            let buf = self.editor.buffer_mut();
            let target = n.saturating_sub(1).min(buf.len_lines().saturating_sub(1));
            let at = buf.offset_of(quark_text::Position { line: target, column: 0 });
            buf.cursor = buf.clamp(Cursor::at(at));
            return Vec::new();
        }
        let Some(ex) = Ex::parse(&line) else {
            return vec![Said::Ex(line)];
        };
        if ex.name == "wa" || ex.name == "wall" {
            let failed = self.save_all();
            return if failed.is_empty() { vec![Said::Info("wrote every file".into())] } else { failed };
        }
        if ex.is("write", 1) {
            return self.ex_write(&ex);
        }
        if ex.is("saveas", 3) {
            let Some(arg) = &ex.arg else {
                return vec![Said::Warn(":saveas needs a file name".into())];
            };
            let path = self.resolve(arg);
            self.editor.buffer_mut().rename(path);
            // Forced: the guard is about the file it came from, and it no
            // longer came from there.
            return self.write_buffer(true);
        }
        if ex.is("edit", 1) {
            return self.ex_edit(&ex);
        }
        if ex.is("vsplit", 2) || ex.is("vnew", 2) {
            return self.window(Window::Split(true));
        }
        if ex.is("split", 2) || ex.is("new", 3) {
            return self.window(Window::Split(false));
        }
        if ex.is("only", 2) {
            return self.window(Window::Only);
        }
        if ex.is("close", 3) {
            return self.window(Window::Close);
        }
        if ex.is("nohlsearch", 3) {
            self.highlight = false;
            return Vec::new();
        }
        if ex.is("quit", 1) || ex.is("bdelete", 2) {
            return self.ex_quit(&ex);
        }
        if ex.is("bnext", 2) {
            return self.ex_buffer(1);
        }
        if ex.is("bprevious", 2) || ex.is("bNext", 2) {
            return self.ex_buffer(-1);
        }
        if ex.name == "wq" || ex.is("xit", 1) {
            let write = ex.name == "wq" || self.editor.buffer().is_modified();
            let mut said = if write { self.ex_write(&ex) } else { Vec::new() };
            if !said.iter().any(|s| matches!(s, Said::Warn(_))) {
                said.extend(self.ex_quit(&Ex { name: "q".into(), bang: true, arg: None }));
            }
            return said;
        }
        vec![Said::Ex(line)]
    }

    /// A path typed on the `:` line, against the project's folder.
    fn resolve(&self, typed: &str) -> PathBuf {
        let path = PathBuf::from(typed);
        match &self.root {
            Some(root) if path.is_relative() => root.path().join(path),
            _ => path,
        }
    }

    fn ex_write(&mut self, ex: &Ex) -> Vec<Said> {
        if let Some(arg) = &ex.arg {
            let path = self.resolve(arg);
            if self.editor.buffer().path().is_none() {
                self.editor.buffer_mut().rename(path);
                return self.write_buffer(true);
            }
            // A copy, not a rename: the buffer keeps its own name and stays
            // modified. `:saveas` is the one that moves house.
            return match self.editor.buffer().write_to(&path) {
                Ok(()) => vec![Said::Info(format!("wrote a copy to {}", path.display()))],
                Err(e) => vec![Said::Warn(format!("{}: {e}", path.display()))],
            };
        }
        self.write_buffer(ex.bang)
    }

    /// Write the buffer to its own file. Without `force` a write refuses
    /// when the file changed underneath it.
    fn write_buffer(&mut self, force: bool) -> Vec<Said> {
        if !force && self.editor.buffer().changed_on_disk() {
            return vec![Said::Warn(format!(
                "{} changed on disk since it was read — :w! to overwrite, :e! to discard yours",
                self.editor.buffer().name()
            ))];
        }
        if self.editor.buffer().path().is_none() {
            return vec![Said::Warn("no path to write to — :w {file} names it".into())];
        }
        match self.editor.save() {
            Ok(()) => {
                if self.editor.buffer().name() == ".editorconfig" {
                    self.editorconfigs.clear();
                }
                let path = self.editor.buffer().path().map(Path::to_path_buf).unwrap_or_default();
                vec![Said::Wrote(path)]
            }
            Err(e) => vec![Said::Warn(e.to_string())],
        }
    }

    fn ex_edit(&mut self, ex: &Ex) -> Vec<Said> {
        if let Some(arg) = &ex.arg {
            let path = self.resolve(arg);
            if path.is_dir() {
                return vec![Said::Warn(format!(
                    "{} is a folder — File ▸ Open LaTeX Project… opens folders",
                    path.display()
                ))];
            }
            if !path.exists() {
                // A new file: a buffer named for it, written on `:w`.
                return match self.editor.open_or_create(&path, "") {
                    Ok(index) => {
                        self.park_cursor();
                        self.editor.focus(index);
                        self.splits.view_mut().buffer = index;
                        vec![Said::Info(format!("{} (new)", path.display()))]
                    }
                    Err(e) => vec![Said::Warn(e.to_string())],
                };
            }
            return match self.open(&path) {
                Ok(()) => Vec::new(),
                Err(e) => vec![Said::Warn(format!("{}: {e}", path.display()))],
            };
        }
        if self.editor.buffer().path().is_none() {
            return vec![Said::Warn("no file to re-read — :e {file} opens one".into())];
        }
        if !ex.bang && self.editor.buffer().is_modified() {
            return vec![Said::Warn(format!("{} has unsaved changes — :e! to discard them", self.editor.buffer().name()))];
        }
        match self.editor.buffer_mut().reload() {
            Ok(()) => vec![Said::Info(format!("re-read {}", self.editor.buffer().name()))],
            Err(e) => vec![Said::Warn(e.to_string())],
        }
    }

    fn ex_quit(&mut self, ex: &Ex) -> Vec<Said> {
        if self.splits.len() > 1 && !ex.is("bdelete", 2) {
            return self.window(Window::Close);
        }
        if !ex.bang && self.editor.buffer().is_modified() {
            return vec![Said::Warn(format!(
                "{} has unsaved changes — :wq to write, :q! to discard them",
                self.editor.buffer().name()
            ))];
        }
        let name = self.editor.buffer().name();
        let closed = self.editor.active();
        self.editor.close(closed);
        self.splits.buffer_closed(closed, self.editor.buffers().len());
        self.splits.view_mut().buffer = self.editor.active();
        vec![Said::Info(format!("closed {name}"))]
    }

    fn ex_buffer(&mut self, by: i64) -> Vec<Said> {
        let count = self.editor.buffers().len() as i64;
        if count < 2 {
            return vec![Said::Warn("only one buffer is open".into())];
        }
        self.park_cursor();
        let next = (self.editor.active() as i64 + by).rem_euclid(count) as usize;
        self.editor.focus(next);
        self.splits.view_mut().buffer = next;
        vec![Said::Info(self.editor.buffer().name())]
    }

    /* ── splits ── */

    /// Split the view in focus: side by side, or one above the other.
    pub fn split(&mut self, vertical: bool) -> Vec<Said> {
        self.window(Window::Split(vertical))
    }

    /// Close the split in focus.
    pub fn close_split(&mut self) -> Vec<Said> {
        self.window(Window::Close)
    }

    /// Move focus to the split that way.
    pub fn step_split(&mut self, direction: Direction) -> Vec<Said> {
        self.window(Window::Step(direction))
    }

    /// `:w`: the file in focus written, unless it changed on disk.
    pub fn save(&mut self) -> Vec<Said> {
        self.write_buffer(false)
    }

    fn window(&mut self, step: Window) -> Vec<Said> {
        self.park_cursor();
        let done = match step {
            Window::Split(vertical) => {
                if self.splits.split(vertical).is_none() {
                    self.restore_cursor();
                    return vec![Said::Warn("four splits is as many as are worth having".into())];
                }
                true
            }
            Window::Close => {
                if self.splits.close(self.splits.focused()).is_none() {
                    self.restore_cursor();
                    return vec![Said::Warn("that is the last split — :q closes the file".into())];
                }
                true
            }
            Window::Only => {
                self.splits.only();
                true
            }
            Window::Step(direction) => self.splits.step(direction).is_some(),
        };
        let _ = done;
        self.restore_cursor();
        self.follow();
        Vec::new()
    }

    /// The live cursor put away on the split in focus, before focus moves.
    fn park_cursor(&mut self) {
        let cursor = self.editor.buffer().cursor;
        let buffer = self.editor.active();
        let view = self.splits.view_mut();
        view.cursor = cursor;
        view.buffer = buffer;
    }

    /// The split now in focus: its cursor made the live one.
    fn restore_cursor(&mut self) {
        let view = *self.splits.view();
        if !self.editor.focus(view.buffer) {
            self.editor.focus(0);
            self.splits.view_mut().buffer = self.editor.active();
        }
        let buffer = self.editor.buffer_mut();
        buffer.cursor = buffer.clamp(view.cursor);
    }

    /// The split in focus scrolled so the cursor is on screen, with a few
    /// lines of room either side.
    fn follow(&mut self) {
        let line = {
            let buf = self.editor.buffer();
            buf.position(buf.cursor.offset).line
        };
        let total = self.editor.buffer().len_lines();
        let view = self.splits.view_mut();
        view.first_line = followed(view.first_line, view.lines, line, total);
    }
}

/// The first line a view of `lines` lines shows, moved as little as it can
/// be so `cursor` is on it with room either side.
pub fn followed(first: usize, lines: usize, cursor: usize, total: usize) -> usize {
    let lines = lines.max(1);
    let off = SCROLL_OFF.min(lines.saturating_sub(1) / 2);
    let mut first = first;
    if cursor < first + off {
        first = cursor.saturating_sub(off);
    } else if cursor + off >= first + lines {
        first = cursor + off + 1 - lines;
    }
    first.min(total.saturating_sub(1))
}

/// What a `⌃w` step does to the splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Window {
    Split(bool),
    Close,
    Only,
    Step(Direction),
}

/// `⌃w v`, `⌃w s` and the rest, as the engine reports them.
fn window_step(prefix: &str) -> Option<Window> {
    Some(match prefix {
        "wv" => Window::Split(true),
        "ws" => Window::Split(false),
        "wq" | "wc" => Window::Close,
        "wo" => Window::Only,
        "wh" => Window::Step(Direction::Left),
        "wl" => Window::Step(Direction::Right),
        "wk" => Window::Step(Direction::Up),
        "wj" => Window::Step(Direction::Down),
        "ww" => Window::Step(Direction::Next),
        "wW" => Window::Step(Direction::Previous),
        _ => return None,
    })
}

fn extension_of(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &mut Session, keys: &str) -> Vec<Said> {
        keys.chars().flat_map(|c| s.key(&Key::char(c))).collect()
    }

    fn with(text: &str) -> Session {
        let mut s = Session::default();
        s.key(&Key::char('i'));
        s.paste(text);
        s.key(&Key::named("Escape"));
        typed(&mut s, "gg");
        s
    }

    fn line(s: &Session) -> usize {
        let b = s.editor().buffer();
        b.position(b.cursor.offset).line
    }

    #[test]
    fn the_view_moves_as_little_as_it_can_to_keep_the_cursor_in_it() {
        // Room either side: three lines.
        assert_eq!(followed(0, 20, 5, 100), 0);
        assert_eq!(followed(0, 20, 17, 100), 1);
        assert_eq!(followed(30, 20, 31, 100), 28);
        assert_eq!(followed(30, 20, 40, 100), 30, "already in view: left alone");
        // A view of two lines has no room to keep.
        assert_eq!(followed(0, 2, 5, 100), 4);
    }

    #[test]
    fn a_number_on_the_command_line_goes_to_that_line() {
        let mut s = with(&"x\n".repeat(50));
        s.set_lines(0, 10);
        for c in ":42".chars() {
            s.key(&Key::char(c));
        }
        s.key(&Key::named("Enter"));
        assert_eq!(line(&s), 41);
        let first = s.view_of(0).unwrap().2;
        assert!((32..=41).contains(&first), "the view followed: {first}");
        for c in ":999".chars() {
            s.key(&Key::char(c));
        }
        s.key(&Key::named("Enter"));
        assert_eq!(line(&s), 50, "past the end, the last line");
    }

    #[test]
    fn control_w_splits_and_moves_between_them() {
        let mut s = with("one\ntwo\n");
        s.key(&Key::ctrl('w'));
        s.key(&Key::char('v'));
        assert_eq!(s.splits().len(), 2);
        assert_eq!(s.splits().focused(), 1);
        s.key(&Key::ctrl('w'));
        s.key(&Key::char('h'));
        assert_eq!(s.splits().focused(), 0);
        let said = typed(&mut s, ":only");
        assert!(said.is_empty());
        s.key(&Key::named("Enter"));
        assert_eq!(s.splits().len(), 1);
    }

    #[test]
    fn a_drag_selects_and_a_click_does_not() {
        let mut s = with("alpha beta\ngamma\n");
        s.click(0, 0, 2);
        assert_eq!(s.mode(), VimMode::Normal);
        s.select_to(0, 2);
        assert_eq!(s.mode(), VimMode::Normal, "a drag that went nowhere is a click");
        s.select_to(1, 3);
        assert_eq!(s.mode(), VimMode::Visual);
        typed(&mut s, "d");
        assert_eq!(s.editor().buffer().contents(), "ala\n");
    }
}
