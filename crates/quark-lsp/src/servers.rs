//! Language servers, from the editor's side of the seam: which server speaks
//! which language, what each has been told, what each has said, and the
//! completion popup. From Proton's core (`lsp.rs`), for texlab.
//!
//! Everything asynchronous is on the other side of this module: `quark-lsp`
//! owns the processes and the threads, and hands back a mailbox. What lives
//! here is state — which server speaks which language, what each of them has
//! been told, and what they have said — plus the translation between LSP's
//! coordinates and Quark's.
//!
//! That translation is the part worth being careful about. LSP counts columns
//! in UTF-16 code units; a rope counts characters. They agree for ASCII and
//! diverge on the first emoji, which is why they are converted here, once,
//! against the buffer that knows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::client::Incoming;
use crate::{Client, Position};
use quark_text::{Buffer, Cursor, Edit};
use serde::{Deserialize, Serialize};

/* ── what the front end sees ──────────────────────────────────────── */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    pub fn from_lsp(code: Option<u8>) -> Self {
        match code {
            Some(1) => Severity::Error,
            Some(2) => Severity::Warning,
            Some(4) => Severity::Hint,
            // Including `None`. The spec leaves an unlabelled diagnostic to
            // the client, and treating it as an error would let a stray note
            // make a clean file look broken.
            _ => Severity::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    pub label: String,
    /// The dim trailing text: a signature, or the module it came from.
    pub hint: Option<String>,
    pub kind: CompletionKind,
}

/// LSP has twenty-five completion kinds. A reader scanning a popup can tell
/// about eight things apart, so this is the set the UI actually draws, and the
/// rest collapse into `Other` rather than each earning an icon nobody reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionKind {
    Function,
    Method,
    Field,
    Variable,
    Type,
    Module,
    Keyword,
    Constant,
    Snippet,
    Other,
}

impl CompletionKind {
    pub fn from_lsp(code: Option<u8>) -> Self {
        match code {
            Some(2) => CompletionKind::Method,
            Some(3) => CompletionKind::Function,
            Some(4) => CompletionKind::Function, // constructor
            Some(5) => CompletionKind::Field,
            Some(6) => CompletionKind::Variable,
            Some(7 | 8 | 22 | 25) => CompletionKind::Type, // class, interface, struct, param
            Some(9) => CompletionKind::Module,
            Some(14) => CompletionKind::Keyword,
            Some(15) => CompletionKind::Snippet,
            Some(21) => CompletionKind::Constant,
            _ => CompletionKind::Other,
        }
    }
}

/// How a server is doing, for the statusline. Not an error type: a language
/// with no server installed is a normal state, not a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerStatus {
    Starting,
    Ready,
    /// Indexing, or otherwise busy with work it has told us about.
    Busy,
    Unavailable,
    Stopped,
}

/* ── which server speaks what ─────────────────────────────────────── */

pub struct Language {
    pub id: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    pub extensions: &'static [&'static str],
}

/// The table is deliberately short and hard-coded for now. It becomes user
/// config in M4 along with the rest of the settings; hard-coding it until then
/// keeps a half-designed config format from being the thing that blocks
/// go-to-definition from working at all.
pub const LANGUAGES: &[Language] = &[
    // texlab, for LaTeX and BibTeX: Quark's own, in `~/.quark/tex`, once
    // TeX is installed (`Servers::set_program`); else whatever is on PATH.
    Language {
        id: "latex",
        program: "texlab",
        args: &[],
        extensions: &["tex", "sty", "cls", "ltx", "bib"],
    },
];

/// The language a file is announced to its server as: BibTeX for a `.bib`,
/// which texlab serves too.
fn announced_as(path: &Path, language: &'static str) -> &'static str {
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if language == "latex" && extension.as_deref() == Some("bib") {
        "bibtex"
    } else {
        language
    }
}

fn language_for(path: &Path) -> Option<&'static Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.extensions.contains(&extension.as_str()))
}

/* ── the servers we are talking to ────────────────────────────────── */

/// What a server was last told about a file.
#[derive(Debug)]
struct Announced {
    language: &'static str,
    /// The version we last sent. LSP only requires this to increase, so it is
    /// ours to count rather than the buffer's — an undo moves a buffer's
    /// version in ways a server must not have to reason about.
    sent: u64,
    /// The buffer version behind that send, so an unchanged buffer is not
    /// re-sent on every keystroke that turned out to be a motion.
    buffer: u64,
}

#[derive(Debug, Default)]
pub struct Servers {
    root: PathBuf,
    /// Off unless asked for. A core test must not spawn rust-analyzer as a
    /// side effect of opening a `.rs` file, and every core test opens one.
    enabled: bool,
    clients: HashMap<&'static str, Client>,
    /// Languages whose server would not start. Remembered so that opening a
    /// second Python file is not a second failed spawn, and a fourth, and a
    /// tenth.
    unavailable: HashSet<&'static str>,
    announced: HashMap<PathBuf, Announced>,
    diagnostics: HashMap<PathBuf, Vec<crate::Diagnostic>>,
    status: HashMap<&'static str, ServerStatus>,
    /// What each server last said it was working on. Kept rather than emitted
    /// per message: progress reports arrive several times a second, and a
    /// statusline does not need to see every one.
    notes: HashMap<&'static str, String>,
    awaiting: HashMap<Ask, i64>,
    /// What each server said it would do, from its `initialize` reply.
    /// Recorded rather than learned from a failed request: a save should not
    /// produce an error notice on every language with no formatter behind it.
    can: HashMap<&'static str, crate::Capable>,
    /// A server's program where it is not the table's name on PATH: texlab
    /// in Quark's own TeX.
    programs: HashMap<&'static str, PathBuf>,
}

/// A question the editor is waiting on an answer to.
///
/// One per kind rather than a set of ids, because asking again replaces the
/// question: press `gd` twice and only the second answer is wanted, and the
/// first is a jump to wherever the cursor used to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ask {
    Definition,
    Hover,
    References,
    Symbols,
    Format,
    Rename,
    CodeActions,
    ResolveAction,
}

impl Servers {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            ..Default::default()
        }
    }

    /// Start talking to servers. Explicit, and off by default, because the
    /// alternative is a test suite that spawns four language servers.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Every server we have tried to start, and how it is doing. Sorted so the
    /// statusline does not reshuffle itself between polls.
    pub fn status(&self) -> Vec<(&'static str, ServerStatus, Option<String>)> {
        let mut all: Vec<_> = self
            .status
            .iter()
            .map(|(id, status)| (*id, *status, self.notes.get(id).cloned()))
            .collect();
        all.sort_by_key(|(id, ..)| *id);
        all
    }

    /// The server for this file, started if it is not already running.
    fn ensure(&mut self, path: &Path) -> Option<&'static str> {
        if !self.enabled {
            return None;
        }
        let language = language_for(path)?;
        if self.unavailable.contains(language.id) {
            return None;
        }
        if !self.clients.contains_key(language.id) {
            let args: Vec<String> = language.args.iter().map(|s| (*s).to_string()).collect();
            let program = self.programs.get(language.id).map(|p| p.display().to_string()).unwrap_or_else(|| language.program.to_string());
            match Client::spawn(language.id, &program, &args, &self.root) {
                Ok(client) => {
                    self.clients.insert(language.id, client);
                    self.status.insert(language.id, ServerStatus::Starting);
                }
                Err(_) => {
                    // Not an error worth interrupting anyone over: most people
                    // do not have gopls installed, and most of them are not
                    // writing Go today either.
                    self.unavailable.insert(language.id);
                    self.status.insert(language.id, ServerStatus::Unavailable);
                    return None;
                }
            }
        }
        Some(language.id)
    }

    /// Tell the server about a buffer — opening it the first time, and
    /// re-syncing it after that.
    pub fn sync(&mut self, buffer: &Buffer) {
        let Some(path) = buffer.path().map(crate::uri::plain) else {
            return;
        };
        let Some(language) = self.ensure(&path) else {
            return;
        };

        let buffer_version = buffer.version();
        let previous = self.announced.get(&path);
        if previous.is_some_and(|a| a.buffer == buffer_version) {
            return;
        }
        let sent = previous.map_or(1, |a| a.sent + 1);
        let first = previous.is_none();
        let text = buffer.contents();

        if let Some(client) = self.clients.get_mut(language) {
            if first {
                client.did_open(&path, announced_as(&path, language), sent, &text);
            } else {
                client.did_change(&path, sent, &text);
            }
        }
        self.announced.insert(
            path,
            Announced {
                language,
                sent,
                buffer: buffer_version,
            },
        );
    }

    pub fn saved(&mut self, buffer: &Buffer) {
        let Some(path) = buffer.path().map(crate::uri::plain) else {
            return;
        };
        let Some(language) = self.announced.get(&path).map(|a| a.language) else {
            return;
        };
        let text = buffer.contents();
        if let Some(client) = self.clients.get_mut(language) {
            client.did_save(&path, &text);
        }
    }

    /// Tell the server a buffer is gone, and forget we announced it.
    ///
    /// Both halves matter. Without the notification the server keeps its own
    /// copy of a file nobody is editing and keeps reporting diagnostics for
    /// it; without forgetting, reopening the same file would be announced as a
    /// change to a document the server no longer has.
    pub fn closed(&mut self, buffer: &Buffer) {
        let Some(path) = buffer.path().map(crate::uri::plain) else {
            return;
        };
        let Some(announced) = self.announced.remove(&path) else {
            return;
        };
        if let Some(client) = self.clients.get_mut(announced.language) {
            client.did_close(&path);
        }
        self.diagnostics.remove(&path);
    }

    /// The client responsible for a file, if it is one we have announced.
    /// Asking about a file no server has been told about is not a question
    /// worth answering.
    fn client_for(&mut self, path: &Path) -> Option<&mut Client> {
        let language = self.announced.get(path)?.language;
        self.clients.get_mut(language)
    }

    pub fn completion(&mut self, path: &Path, at: Position) -> Option<i64> {
        let path = crate::uri::plain(path);
        Some(self.client_for(&path)?.completion(&path, at))
    }

    pub fn definition(&mut self, path: &Path, at: Position) -> Option<i64> {
        let path = crate::uri::plain(path);
        let id = self.client_for(&path)?.definition(&path, at);
        self.awaiting.insert(Ask::Definition, id);
        Some(id)
    }

    pub fn hover(&mut self, path: &Path, at: Position) -> Option<i64> {
        let path = crate::uri::plain(path);
        let id = self.client_for(&path)?.hover(&path, at);
        self.awaiting.insert(Ask::Hover, id);
        Some(id)
    }

    pub fn references(&mut self, path: &Path, at: Position) -> Option<i64> {
        let path = crate::uri::plain(path);
        let id = self.client_for(&path)?.references(&path, at);
        self.awaiting.insert(Ask::References, id);
        Some(id)
    }

    /// Start `id`'s server as `program`, rather than the table's name on PATH;
    /// tried again if it could not be started before.
    pub fn set_program(&mut self, id: &'static str, program: PathBuf) {
        if self.programs.get(id) != Some(&program) {
            self.programs.insert(id, program);
            self.unavailable.remove(id);
            if self.status.get(id) == Some(&ServerStatus::Unavailable) {
                self.status.remove(id);
            }
        }
    }

    /// What this file's server said it would do. Everything false for a file
    /// no server has been told about, which is the honest answer.
    pub fn can(&self, path: &Path) -> crate::Capable {
        self.announced
            .get(&crate::uri::plain(path))
            .and_then(|a| self.can.get(a.language).copied())
            .unwrap_or_default()
    }

    pub fn format(&mut self, path: &Path) -> Option<i64> {
        let path = crate::uri::plain(path);
        if !self.can(&path).format {
            return None;
        }
        let id = self.client_for(&path)?.formatting(&path);
        self.awaiting.insert(Ask::Format, id);
        Some(id)
    }

    pub fn rename(&mut self, path: &Path, at: Position, to: &str) -> Option<i64> {
        let path = crate::uri::plain(path);
        if !self.can(&path).rename {
            return None;
        }
        let id = self.client_for(&path)?.rename(&path, at, to);
        self.awaiting.insert(Ask::Rename, id);
        Some(id)
    }

    pub fn code_actions(
        &mut self,
        path: &Path,
        range: crate::Range,
        diagnostics: &[serde_json::Value],
    ) -> Option<i64> {
        let path = crate::uri::plain(path);
        if !self.can(&path).code_action {
            return None;
        }
        let id = self
            .client_for(&path)?
            .code_actions(&path, range, diagnostics);
        self.awaiting.insert(Ask::CodeActions, id);
        Some(id)
    }

    pub fn resolve_action(&mut self, path: &Path, action: serde_json::Value) -> Option<i64> {
        let path = crate::uri::plain(path);
        let id = self.client_for(&path)?.resolve_action(action);
        self.awaiting.insert(Ask::ResolveAction, id);
        Some(id)
    }

    pub fn symbols(&mut self, path: &Path) -> Option<i64> {
        let path = crate::uri::plain(path);
        let id = self.client_for(&path)?.symbols(&path);
        self.awaiting.insert(Ask::Symbols, id);
        Some(id)
    }

    /// True if this is the answer we are still waiting for, rather than one to
    /// a question the user has since changed their mind about — a `gd` from
    /// two cursor positions ago, or a hover for a word already scrolled past.
    pub fn wanted(&mut self, ask: Ask, id: i64) -> bool {
        if self.awaiting.get(&ask) == Some(&id) {
            self.awaiting.remove(&ask);
            return true;
        }
        false
    }

    /// Whether a server's idea of a document is still the one we sent it.
    ///
    /// The version in a workspace edit is the server's count of *our*
    /// `didChange` notifications, not the buffer's own version — so this is
    /// the only place the two can be compared. `true` when there is nothing
    /// to check: a server that sent no version, or a file we never announced.
    pub fn is_current(&self, path: &Path, version: Option<i64>) -> bool {
        let (Some(version), Some(announced)) =
            (version, self.announced.get(&crate::uri::plain(path)))
        else {
            return true;
        };
        announced.sent as i64 == version
    }

    /// Stop wanting an answer, without one having arrived. Closing the hover
    /// panel by hand should not leave a reply in flight that reopens it.
    pub fn forget(&mut self, ask: Ask) {
        self.awaiting.remove(&ask);
    }

    /// Take everything the servers have said. Never blocks.
    pub fn poll(&mut self) -> Vec<(&'static str, Incoming)> {
        let mut all = Vec::new();
        for (language, client) in self.clients.iter_mut() {
            for message in client.poll() {
                all.push((*language, message));
            }
        }

        for (language, message) in &all {
            match message {
                Incoming::Ready { can, .. } => {
                    self.status.insert(language, ServerStatus::Ready);
                    self.can.insert(language, *can);
                }
                Incoming::Progress {
                    title,
                    message,
                    done,
                } => {
                    self.status.insert(
                        language,
                        if *done {
                            ServerStatus::Ready
                        } else {
                            ServerStatus::Busy
                        },
                    );
                    if *done {
                        self.notes.remove(language);
                    } else {
                        // "indexing 40%" reads better than either half alone.
                        self.notes.insert(
                            language,
                            match message {
                                Some(detail) => format!("{title} {detail}"),
                                None => title.clone(),
                            },
                        );
                    }
                }
                Incoming::Exited => {
                    self.status.insert(language, ServerStatus::Stopped);
                }
                Incoming::Diagnostics { path, items } => {
                    self.diagnostics.insert(path.clone(), items.clone());
                }
                _ => {}
            }
        }
        all
    }

    pub fn diagnostics_for(&self, path: &Path) -> &[crate::Diagnostic] {
        self.diagnostics
            .get(&crate::uri::plain(path))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/* ── coordinates ──────────────────────────────────────────────────── */

/// Where the cursor is, as a server counts it.
pub fn position_of(buffer: &Buffer, offset: usize) -> Position {
    Position {
        line: buffer.position(offset).line as u32,
        character: buffer.utf16_column(offset) as u32,
    }
}

/// A diagnostic in the buffer's own coordinates.
///
/// Converted against the buffer as it is *now*, not as it was when the server
/// spoke. Diagnostics always arrive slightly late; clamping them to something
/// real beats holding a stale copy of the text to be exactly right about a
/// squiggle.
pub fn diagnostic(buffer: &Buffer, raw: &crate::Diagnostic) -> Diagnostic {
    let start = buffer.position(buffer.offset_at_utf16(
        raw.range.start.line as usize,
        raw.range.start.character as usize,
    ));
    let end = buffer.position(buffer.offset_at_utf16(
        raw.range.end.line as usize,
        raw.range.end.character as usize,
    ));
    Diagnostic {
        line: start.line,
        column: start.column,
        end_line: end.line,
        end_column: end.column,
        severity: Severity::from_lsp(raw.severity),
        message: raw.message.clone(),
        source: raw.source.clone(),
        code: raw.code.as_ref().map(ToString::to_string),
    }
}

/// The char column a UTF-16 column names, against one line of text.
///
/// The buffer does this for text it holds. A reference in a file nobody has
/// opened has no buffer, and reading the whole file into one to find a column
/// would be a rope per row of the picker.
pub fn char_column(line: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (chars, c) in line.chars().enumerate() {
        if units >= utf16 {
            return chars;
        }
        units += c.len_utf16();
    }
    line.chars().count()
}

/// Apply a server's edits to a buffer, keeping the cursor where it was.
///
/// Three things make this more than a loop.
///
/// **Every offset is computed before anything is applied.** The ranges are
/// all against the document as the server saw it, so converting them one at a
/// time as the text moves underneath would leave every edit but the first
/// pointing at the wrong place. `Buffer::apply` then applies them back to
/// front and records the lot as one undoable change — which is what makes a
/// reformat one press of `u` rather than four hundred.
///
/// **The cursor is kept by line and column, not by offset.** A whole-document
/// format is usually one edit replacing everything, and an offset-preserving
/// cursor lands at the end of the file. Line and column survives the common
/// case — a few lines re-indented above you — which is the one that matters.
///
/// Returns whether anything actually changed.
pub fn apply_edits(buffer: &mut Buffer, edits: &[crate::TextEdit]) -> bool {
    let was = buffer.position(buffer.cursor.offset);

    let edits: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            start: buffer.offset_at_utf16(
                edit.range.start.line as usize,
                edit.range.start.character as usize,
            ),
            end: buffer.offset_at_utf16(
                edit.range.end.line as usize,
                edit.range.end.character as usize,
            ),
            text: edit.new_text.clone(),
        })
        // A server may send an edit that changes nothing — a range replaced
        // by what is already in it. Keeping those would make an already
        // formatted file report as reformatted, and mark it modified.
        .filter(|edit| buffer.text().slice(edit.start..edit.end) != edit.text.as_str())
        .collect();

    if edits.is_empty() {
        return false;
    }

    // Where the cursor lands is worked out after the edits are applied, so
    // this is a placeholder that `apply` will clamp; the real move follows.
    buffer.apply(edits, buffer.cursor);
    let at = buffer.offset_of(was);
    buffer.cursor = buffer.clamp(buffer.cursor.moved_to(at));
    true
}

/// LSP's symbol kinds, in the vocabulary the completion popup already draws.
///
/// The two numberings are unrelated — `6` is a method here and a variable
/// there — so this is a second table rather than a shared one. What is shared
/// is the *display*: a function is a function, and giving the symbol list its
/// own icons would be two legends for one idea.
pub fn symbol_kind(code: Option<u8>) -> CompletionKind {
    match code {
        Some(2..=4) => CompletionKind::Module, // module, namespace, package
        Some(5 | 10 | 11 | 23) => CompletionKind::Type, // class, enum, interface, struct
        Some(6) => CompletionKind::Method,
        Some(7) => CompletionKind::Field, // property
        Some(8) => CompletionKind::Field,
        Some(9 | 12) => CompletionKind::Function, // constructor, function
        Some(13) => CompletionKind::Variable,
        Some(14 | 22) => CompletionKind::Constant, // constant, enum member
        _ => CompletionKind::Other,
    }
}


/* ── the completion popup ─────────────────────────────────────────── */

/// A server can answer with several hundred candidates. Past the first screen
/// nobody is reading them, and every one costs a row in an event that has to
/// cross the bridge on a keystroke.
const MOST: usize = 200;

#[derive(Debug, Default)]
pub struct Popup {
    /// Asked for, though possibly not yet showing anything.
    wanted: bool,
    /// The request whose answer we still want. A reply to an older one is
    /// stale — the cursor has moved since we asked.
    awaiting: Option<i64>,
    items: Vec<Completion>,
    raw: Vec<crate::CompletionItem>,
    selected: usize,
}

impl Popup {
    /// Whether the popup owns the keyboard. Deliberately false while the
    /// first reply is still in flight: stealing Enter from someone who is
    /// typing, on the chance that a list might arrive, is worse than being
    /// slow.
    pub fn is_open(&self) -> bool {
        self.wanted && !self.items.is_empty()
    }

    pub fn expect(&mut self, id: i64) {
        self.wanted = true;
        self.awaiting = Some(id);
    }

    /// Accept a reply, unless it answers a question we have stopped asking.
    pub fn fill(&mut self, id: i64, items: Vec<crate::CompletionItem>) -> bool {
        if self.awaiting != Some(id) {
            return false;
        }
        self.awaiting = None;
        self.raw = items;

        // Servers rank by `sortText`, not by label — that is the whole point
        // of the field. Ignoring it is the difference between the obvious
        // candidate being first and being fortieth.
        self.raw.sort_by(|a, b| {
            let key = |i: &crate::CompletionItem| {
                i.sort_text.clone().unwrap_or_else(|| i.label.clone())
            };
            key(a).cmp(&key(b))
        });
        self.raw.truncate(MOST);

        self.items = self
            .raw
            .iter()
            .map(|item| Completion {
                label: item.label.clone(),
                hint: item.hint(),
                kind: CompletionKind::from_lsp(item.kind),
            })
            .collect();
        self.selected = 0;
        true
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn move_by(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as i32;
        self.selected = (((self.selected as i32 + delta) % len + len) % len) as usize;
    }

    /// Pick a row outright, for a click. The keyboard moves by delta; a mouse
    /// already knows which row it is on.
    pub fn select(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = index;
        }
    }

    pub fn items(&self) -> &[Completion] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn chosen(&self) -> Option<&crate::CompletionItem> {
        self.raw.get(self.selected)
    }
}

/// Put a chosen completion in the buffer.
///
/// Pure, and separate from the workspace, because the interesting part is not
/// the insertion but the clamping — and that is worth being able to test
/// without a language server in the room.
pub fn apply_completion(buffer: &mut Buffer, item: &crate::CompletionItem) {
    let (range, text) = item.edit();
    let cursor = buffer.cursor.offset;

    let (start, end) = match range {
        // A `textEdit` is how a server says "you typed `HashM`, replace all of
        // it". Appending instead is how you get `HashMHashMap`.
        Some(range) => (
            buffer.offset_at_utf16(range.start.line as usize, range.start.character as usize),
            buffer.offset_at_utf16(range.end.line as usize, range.end.character as usize),
        ),
        None => (cursor, cursor),
    };

    // The list can be a keystroke or two behind the buffer, so the span is
    // clamped to end at the cursor. Replacing what was typed is the point;
    // eating what comes after it never is.
    let start = start.min(cursor);
    let end = end.clamp(start, cursor);
    let landing = start + text.chars().count();
    buffer.apply(vec![Edit { start, end, text }], Cursor::at(landing));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latex_and_bibtex_go_to_texlab_and_quarks_own_is_tried_once_it_is_installed() {
        let tex = language_for(Path::new("main.tex")).unwrap();
        assert_eq!((tex.id, tex.program), ("latex", "texlab"));
        assert_eq!(announced_as(Path::new("refs.bib"), "latex"), "bibtex");
        assert_eq!(announced_as(Path::new("main.tex"), "latex"), "latex");
        let mut servers = Servers::new(".");
        servers.unavailable.insert("latex");
        servers.status.insert("latex", ServerStatus::Unavailable);
        servers.set_program("latex", PathBuf::from("/Users/you/.quark/tex/texlab/texlab"));
        assert!(!servers.unavailable.contains("latex") && !servers.status.contains_key("latex"), "tried again once its program is known");
        assert_eq!(servers.programs.get("latex").map(|p| p.display().to_string()).as_deref(), Some("/Users/you/.quark/tex/texlab/texlab"));
    }
}
