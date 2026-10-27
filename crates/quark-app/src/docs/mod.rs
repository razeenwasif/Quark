//! The docs editor: documents written in Quark, as Markdown files.
//!
//! Each open document is a [`DocTab`] in the tab strip, after Home and any
//! LaTeX project. While one shows it draws its own three islands — the
//! outline, the page, and the dock of comments and Claude — in the space
//! Home also draws into, so the PDF chrome around it stays out of the way.
//!
//! # Saving
//!
//! Documents save themselves, a moment after the typing stops, the way a
//! Google Doc does: there is no unsaved state to lose and no question to
//! answer on closing. A new document has no file until something is written
//! in it, and until it is given a name it takes one from its first line.
//! ⌘S still saves at once, for the habit.
//!
//! A file changed by another program while open is never written over
//! silently: with nothing typed here since, it is read again; otherwise
//! saving stops for that document and says so, until ⌘S says to overwrite.

pub(crate) mod claude;
mod comments;
pub(crate) mod export;
pub(crate) mod home;
mod icons;
mod input;
pub(crate) mod layout;
#[cfg(test)]
mod shots;
mod view;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use quark_doc::{Editor, markdown};

use crate::app::App;

/// How long after the last change a document saves itself.
const AUTOSAVE: Duration = Duration::from_millis(900);
/// How often open files are looked at for changes made elsewhere.
const DISK_CHECK: Duration = Duration::from_secs(2);
/// The extensions opened as documents rather than PDFs.
pub(crate) const EXTENSIONS: [&str; 4] = ["md", "markdown", "mdown", "txt"];

pub(crate) fn is_document(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Which panel the document's dock shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DockTab {
    #[default]
    Comments,
    Claude,
}

/// Names a tab for as long as it is open, whatever its file is called.
static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// One open document.
pub(crate) struct DocTab {
    pub(crate) uid: u64,
    pub(crate) editor: Editor,
    pub(crate) path: PathBuf,
    /// Whether the file exists yet. A new document is written the first time
    /// there is something in it.
    pub(crate) exists: bool,
    /// Named after its first line until the person names it.
    pub(crate) auto_name: bool,
    /// The file's modification time as last read or written.
    disk: Option<SystemTime>,
    /// Set when the file changed elsewhere with edits here unsaved: saving
    /// waits for ⌘S.
    pub(crate) conflict: bool,
    /// The editor state last seen, and when it was first seen.
    seen: (u64, Instant),
    checked: Instant,
    pub(crate) last_error: Option<String>,

    // --- view ---
    pub(crate) zoom: f32,
    pub(crate) cache: layout::Cache,
    /// The sheet as last drawn, for input that needs geometry.
    pub(crate) sheet: layout::Sheet,
    /// Where the sheet was on screen last frame.
    pub(crate) sheet_origin: egui::Pos2,
    /// The column a run of ↑ and ↓ keeps to.
    pub(crate) goal_x: Option<f32>,
    /// Scroll the caret into view on the next frame.
    pub(crate) reveal: bool,
    /// Scroll to put this block at the top: a click in the outline.
    pub(crate) jump: Option<usize>,
    /// When the caret last moved, so it shows solid while in use.
    pub(crate) blink: Instant,
    /// Text being composed by an input method, shown at the caret.
    pub(crate) preedit: String,
    /// A drag selecting: where it began, and whether by words or blocks.
    pub(crate) drag: Option<view::Drag>,

    // --- the dock ---
    pub(crate) dock: DockTab,
    pub(crate) active_comment: Option<u32>,
    pub(crate) show_resolved: bool,
    pub(crate) draft: String,
    pub(crate) link_draft: Option<String>,
    pub(crate) find: Option<view::Find>,
    pub(crate) rename: Option<String>,
    pub(crate) new_comment: Option<comments::NewComment>,

    // --- Claude ---
    pub(crate) chat: Vec<claude::ChatTurn>,
    pub(crate) chat_draft: String,
    pub(crate) focus_chat: bool,
    /// The instruction being written under a selection.
    pub(crate) ask_box: Option<String>,
    /// A question held until the person has been told the text leaves the Mac.
    pub(crate) waiting_ask: Option<claude::Asked>,
    /// Whether Claude is answering for this document, and what it is doing.
    pub(crate) claude_busy: bool,
    pub(crate) claude_doing: Option<String>,
}

impl DocTab {
    fn new(editor: Editor, path: PathBuf, exists: bool, auto_name: bool) -> DocTab {
        let disk = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let state = editor.state();
        DocTab {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            editor,
            path,
            exists,
            auto_name,
            disk,
            conflict: false,
            seen: (state, Instant::now()),
            checked: Instant::now(),
            last_error: None,
            zoom: 1.0,
            cache: Default::default(),
            sheet: Default::default(),
            sheet_origin: egui::Pos2::ZERO,
            goal_x: None,
            reveal: false,
            jump: None,
            blink: Instant::now(),
            preedit: String::new(),
            drag: None,
            dock: DockTab::default(),
            active_comment: None,
            show_resolved: false,
            draft: String::new(),
            link_draft: None,
            find: None,
            rename: None,
            new_comment: None,
            chat: Vec::new(),
            chat_draft: String::new(),
            focus_chat: false,
            ask_box: None,
            waiting_ask: None,
            claude_busy: false,
            claude_doing: None,
        }
    }

    /// Reads a file.
    pub(crate) fn open(path: &Path) -> std::io::Result<DocTab> {
        let text = std::fs::read_to_string(path)?;
        let doc = markdown::read(&text);
        Ok(DocTab::new(Editor::new(doc), path.to_path_buf(), true, false))
    }

    /// A new, empty document, to be written in `folder`.
    pub(crate) fn blank(folder: &Path) -> DocTab {
        let path = unique(folder, "Untitled");
        DocTab::new(Editor::default(), path, false, true)
    }

    /// A new document holding Markdown, as Claude drafted it.
    pub(crate) fn drafted(folder: &Path, src: &str) -> DocTab {
        let mut tab = DocTab::blank(folder);
        tab.editor.insert_markdown(src);
        tab.editor.caret_to(tab.editor.doc.start());
        tab
    }

    pub(crate) fn title(&self) -> String {
        self.path.file_stem().map_or_else(|| "Untitled".into(), |s| s.to_string_lossy().into_owned())
    }

    /// Whether the document has changes no file holds yet.
    pub(crate) fn unsaved(&self) -> bool {
        self.editor.is_dirty() && (self.exists || !self.editor.doc.is_empty())
    }

    /// Writes the file, and before that, renames a document still called
    /// after its first line if that line has changed.
    pub(crate) fn save(&mut self, force: bool) -> Result<(), String> {
        if !force && self.conflict {
            return Err(format!("{} was changed by another app — ⌘S saves over it", self.title()));
        }
        if !self.exists && self.editor.doc.is_empty() {
            self.editor.mark_saved();
            return Ok(());
        }
        if self.auto_name {
            if let Some(name) = name_from(&self.editor.doc) {
                let folder = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
                if self.title() != name && !self.title().starts_with(&format!("{name} ")) {
                    let to = unique(&folder, &name);
                    if self.exists {
                        std::fs::rename(&self.path, &to).map_err(|e| format!("Could not rename to {}: {e}", to.display()))?;
                    }
                    self.path = to;
                }
            }
        }
        let text = markdown::write(&self.editor.doc);
        write_atomically(&self.path, &text).map_err(|e| format!("Could not save {}: {e}", self.path.display()))?;
        self.exists = true;
        self.conflict = false;
        self.disk = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        self.editor.mark_saved();
        self.last_error = None;
        Ok(())
    }

    /// Gives the document a name of the person's choosing.
    pub(crate) fn rename(&mut self, name: &str) -> Result<(), String> {
        let name = sanitise(name);
        if name.is_empty() {
            return Ok(());
        }
        let folder = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let ext = self.path.extension().map_or_else(|| "md".to_string(), |e| e.to_string_lossy().into_owned());
        let to = folder.join(format!("{name}.{ext}"));
        if to == self.path {
            self.auto_name = false;
            return Ok(());
        }
        if to.exists() {
            return Err(format!("There is already a file called {name}.{ext} there"));
        }
        if self.exists {
            std::fs::rename(&self.path, &to).map_err(|e| format!("Could not rename: {e}"))?;
        }
        self.path = to;
        self.auto_name = false;
        Ok(())
    }

    /// Saves if the typing has stopped; reads the file again if it changed
    /// elsewhere. Returns an error to show, once.
    fn tend(&mut self, now: Instant) -> Option<String> {
        let state = self.editor.state();
        if state != self.seen.0 {
            self.seen = (state, now);
        }
        if now.duration_since(self.checked) >= DISK_CHECK && self.exists {
            self.checked = now;
            let on_disk = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
            if on_disk.is_some() && on_disk != self.disk {
                if self.editor.is_dirty() {
                    if !self.conflict {
                        self.conflict = true;
                        return Some(format!("{} was changed by another app — your edits are kept here; ⌘S saves over it", self.title()));
                    }
                } else if let Ok(text) = std::fs::read_to_string(&self.path) {
                    let at = self.editor.selection();
                    self.editor = Editor::new(markdown::read(&text));
                    self.editor.set_selection(at);
                    self.disk = on_disk;
                }
            }
        }
        if self.editor.is_dirty() && !self.conflict && now.duration_since(self.seen.1) >= AUTOSAVE {
            if let Err(e) = self.save(false) {
                // Said once; tried again with the next change.
                self.seen.1 = now + Duration::from_secs(3600);
                if self.last_error.as_deref() != Some(e.as_str()) {
                    self.last_error = Some(e.clone());
                    return Some(e);
                }
            }
        }
        None
    }

    /// Whether a frame should come soon to save.
    fn waiting(&self) -> bool {
        self.editor.is_dirty() && !self.conflict && self.last_error.is_none()
    }
}

/// A file name not taken in `folder`: `name.md`, then `name 2.md`, …
fn unique(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(format!("{name}.md"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| folder.join(format!("{name} {n}.md"))).find(|p| !p.exists()).unwrap()
}

/// What a file name can hold of a line of text.
fn sanitise(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut name = String::new();
    for w in words {
        if name.chars().count() + w.chars().count() > 60 {
            break;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(w);
    }
    name.trim_start_matches('.').to_string()
}

/// The name a document takes from its first line of text.
fn name_from(doc: &quark_doc::Document) -> Option<String> {
    let first = doc.blocks.iter().find(|b| !b.text().trim().is_empty())?;
    let name = sanitise(first.text().lines().next().unwrap_or(""));
    (!name.is_empty()).then_some(name)
}

/// Written beside the file and moved over it, so a crash mid-write leaves
/// the old file rather than half of the new one.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.quark-tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// `~` as the home folder.
pub(crate) fn expand(folder: &str) -> PathBuf {
    if let Some(rest) = folder.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if folder == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(folder)
}

/// Every open document, and which one shows.
#[derive(Default)]
pub(crate) struct Docs {
    pub(crate) tabs: Vec<DocTab>,
    pub(crate) shown: Option<usize>,
}

impl Docs {
    pub(crate) fn current(&self) -> Option<&DocTab> {
        self.shown.and_then(|i| self.tabs.get(i))
    }

    pub(crate) fn current_mut(&mut self) -> Option<&mut DocTab> {
        self.shown.and_then(|i| self.tabs.get_mut(i))
    }
}

/// What closing a document with nothing saved asks.
pub(crate) const CLOSE_DOC: &str = "Close Document";

impl App {
    /// The folder new documents go in.
    pub(crate) fn docs_folder(&self) -> PathBuf {
        expand(&self.prefs.docs_folder)
    }

    pub(crate) fn new_document(&mut self) {
        let tab = DocTab::blank(&self.docs_folder());
        self.add_document(tab);
    }

    /// A new document with Claude's draft in it.
    pub(crate) fn new_drafted_document(&mut self, markdown: &str) {
        let tab = DocTab::drafted(&self.docs_folder(), markdown);
        self.add_document(tab);
    }

    fn add_document(&mut self, tab: DocTab) {
        self.docs.tabs.push(tab);
        self.show_doc(self.docs.tabs.len() - 1);
    }

    pub(crate) fn open_document(&mut self, path: &Path) {
        if let Some(i) = self.docs.tabs.iter().position(|t| t.path == path) {
            self.show_doc(i);
            return;
        }
        match DocTab::open(path) {
            Ok(tab) => {
                self.remember_document(path);
                self.add_document(tab);
            }
            Err(e) => self.toast(format!("Could not open {}: {e}", path.display()), true),
        }
    }

    fn remember_document(&mut self, path: &Path) {
        let key = path.display().to_string();
        self.prefs.docs_recent.retain(|p| *p != key);
        self.prefs.docs_recent.insert(0, key);
        self.prefs.docs_recent.truncate(30);
    }

    pub(crate) fn show_doc(&mut self, i: usize) {
        if i < self.docs.tabs.len() {
            self.home.visible = false;
            self.tex_visible = false;
            self.docs.shown = Some(i);
            self.docs.tabs[i].reveal = true;
        }
    }

    pub(crate) fn save_document(&mut self) {
        let Some(tab) = self.docs.current_mut() else { return };
        let result = tab.save(true);
        let path = tab.path.clone();
        match result {
            Ok(()) => {
                self.remember_document(&path);
                self.toast("Saved", false);
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// Saves a copy somewhere else, and goes on editing that.
    pub(crate) fn save_document_as(&mut self) {
        let Some(tab) = self.docs.current() else { return };
        let folder = tab.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let Some(to) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md"])
            .set_directory(folder)
            .set_file_name(format!("{}.md", tab.title()))
            .save_file()
        else {
            return;
        };
        let Some(tab) = self.docs.current_mut() else { return };
        tab.path = to.clone();
        tab.exists = false;
        tab.auto_name = false;
        tab.conflict = false;
        match tab.save(true) {
            Ok(()) => self.remember_document(&to),
            Err(e) => self.toast(e, true),
        }
    }

    /// Closes a document, saving it first. Asks only when it cannot be saved.
    pub(crate) fn close_document(&mut self, i: usize, force: bool) {
        let Some(tab) = self.docs.tabs.get_mut(i) else { return };
        if !force && tab.unsaved() {
            if let Err(e) = tab.save(false) {
                self.show_doc(i);
                self.dialog = Some(crate::dialogs::Dialog::Confirm {
                    title: CLOSE_DOC.into(),
                    body: format!("{e}. Close it anyway, and lose the changes since it was last saved?"),
                    confirm_label: "Close Without Saving".into(),
                    danger: true,
                });
                return;
            }
        }
        if tab.exists {
            let path = tab.path.clone();
            self.remember_document(&path);
        }
        self.docs.tabs.remove(i);
        match self.docs.shown {
            Some(s) if s == i => {
                self.docs.shown = None;
                if self.docs.tabs.is_empty() {
                    if self.tabs.is_empty() {
                        self.show_home();
                    }
                } else {
                    self.show_doc(i.min(self.docs.tabs.len() - 1));
                }
            }
            Some(s) if s > i => self.docs.shown = Some(s - 1),
            _ => {}
        }
        if self.docs.shown.is_none() && !self.home.visible && !self.tex_visible && self.tabs.is_empty() {
            self.show_home();
        }
    }

    /// Autosaving, and files changed elsewhere, for every open document.
    pub(crate) fn tend_documents(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let mut said = Vec::new();
        let mut waiting = false;
        for tab in &mut self.docs.tabs {
            if let Some(e) = tab.tend(now) {
                said.push(e);
            }
            waiting |= tab.waiting();
        }
        for s in said {
            self.toast(s, true);
        }
        if waiting {
            ctx.request_repaint_after(AUTOSAVE);
        } else if !self.docs.tabs.is_empty() {
            ctx.request_repaint_after(DISK_CHECK);
        }
    }

    /// Saves everything on the way out.
    pub(crate) fn save_all_documents(&mut self) {
        for tab in &mut self.docs.tabs {
            if tab.unsaved() {
                if let Err(e) = tab.save(false) {
                    tracing::warn!("{e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quark-docs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_blank_document_has_no_file_until_it_has_words() {
        let dir = temp("blank");
        let mut tab = DocTab::blank(&dir);
        tab.save(false).unwrap();
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        tab.editor.type_text("Seminar plan");
        tab.save(false).unwrap();
        assert_eq!(tab.path, dir.join("Seminar plan.md"));
        assert_eq!(std::fs::read_to_string(&tab.path).unwrap(), "Seminar plan\n");
    }

    #[test]
    fn an_untitled_document_follows_its_first_line() {
        let dir = temp("follow");
        let mut tab = DocTab::blank(&dir);
        tab.editor.type_text("Draft");
        tab.save(false).unwrap();
        tab.editor.type_text(" two");
        tab.save(false).unwrap();
        assert_eq!(tab.path, dir.join("Draft two.md"));
        assert!(!dir.join("Draft.md").exists());
        // Named by the person, it stays named.
        tab.rename("Notes").unwrap();
        tab.editor.type_text("!");
        tab.save(false).unwrap();
        assert_eq!(tab.path, dir.join("Notes.md"));
    }

    #[test]
    fn names_never_take_another_files_place() {
        let dir = temp("unique");
        std::fs::write(dir.join("Plan.md"), "theirs").unwrap();
        let mut tab = DocTab::blank(&dir);
        tab.editor.type_text("Plan");
        tab.save(false).unwrap();
        assert_eq!(tab.path, dir.join("Plan 2.md"));
        assert_eq!(std::fs::read_to_string(dir.join("Plan.md")).unwrap(), "theirs");
    }

    #[test]
    fn file_names_are_cleaned_and_kept_short() {
        assert_eq!(sanitise("a/b: c?"), "a b c");
        assert_eq!(sanitise("..hidden"), "hidden");
        assert!(sanitise(&"word ".repeat(40)).chars().count() <= 60);
    }

    #[test]
    fn a_file_changed_elsewhere_is_not_written_over() {
        let dir = temp("conflict");
        let path = dir.join("Shared.md");
        std::fs::write(&path, "one\n").unwrap();
        let mut tab = DocTab::open(&path).unwrap();
        tab.editor.caret_to(tab.editor.doc.end());
        tab.editor.type_text(" mine");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "theirs\n").unwrap();
        // Pretend the last look was long ago and the typing stopped.
        let later = Instant::now() + DISK_CHECK + AUTOSAVE;
        tab.checked = Instant::now() - DISK_CHECK;
        tab.disk = Some(SystemTime::UNIX_EPOCH);
        assert!(tab.tend(later).is_some());
        assert!(tab.conflict);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs\n");
        tab.save(true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one mine\n");
    }

    #[test]
    fn markdown_and_text_open_as_documents() {
        assert!(is_document(Path::new("a/notes.md")));
        assert!(is_document(Path::new("README.MARKDOWN")));
        assert!(!is_document(Path::new("paper.pdf")));
    }
}
