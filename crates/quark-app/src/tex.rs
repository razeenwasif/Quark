//! The LaTeX workspace: a project's files on the left, its source in a Vim
//! editor beside them, a status line under it. Builds and the PDF come with
//! L3; this is the editor (L1).
//!
//! The editor is `quark_modal`'s session; this gives it keys, draws its
//! splits with `code`, and keeps a highlighter for each file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{CornerRadius, Rect, RichText, Sense, Ui, pos2, vec2};
use quark_lsp::client::Incoming;
use quark_lsp::servers::{self, Popup, Servers};
use quark_modal::key::Key;
use quark_modal::session::{Said, Session};
use quark_modal::vim::VimMode;
use quark_syntax::Syntax;
use quark_ui::theme::{self, Palette};

use crate::code::{self, Pointer, ViewDraw};
use crate::texvisual::Pane;

/// The most files a project's list shows.
const MOST_FILES: usize = 2_000;
/// Folders that are build output or tools', not the project's.
const PASSED: [&str; 6] = ["node_modules", "target", "build", "_minted", "__pycache__", "out"];
/// The dock's width.
const DOCK_W: f32 = 320.0;
/// The status line's height.
const STATUS_H: f32 = 28.0;
/// How narrow or wide each pane may be dragged.
const FILES_MIN: f32 = 150.0;
const FILES_MAX: f32 = 420.0;
const DOCK_MIN: f32 = 260.0;
const AGENT_MIN: f32 = 280.0;
const AGENT_MAX: f32 = 620.0;
/// The editor keeps at least this; the dock gives way first, then the agent.
const EDITOR_MIN: f32 = 360.0;
/// The source's share beside the page, at least and at most.
const SPLIT: (f32, f32) = (0.2, 0.8);
/// A seam's width: the gap between islands.
const SEAM: f32 = 10.0;

/// Where each island goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Places {
    pub files: Rect,
    pub editor: Rect,
    pub dock: Rect,
    pub agent: Option<Rect>,
}

/// The islands across `full`: the files on the left, then the editor, the
/// dock, and the agent when it is open. `auto_dock` is the dock's width when
/// it has not been dragged.
pub(crate) fn place(full: Rect, l: &quark_core::prefs::TexLayout, auto_dock: f32, agent_open: bool) -> Places {
    let gap = theme::GAP;
    let files_w = l.files.clamp(FILES_MIN, FILES_MAX);
    let mut agent_w = if agent_open { l.agent.clamp(AGENT_MIN, AGENT_MAX) } else { 0.0 };
    let mut dock_w = l.dock.unwrap_or(auto_dock).max(DOCK_MIN);
    let gaps = gap * if agent_open { 3.0 } else { 2.0 };
    let over = files_w + dock_w + agent_w + gaps + EDITOR_MIN - full.width();
    if over > 0.0 {
        let d = over.min(dock_w - DOCK_MIN);
        dock_w -= d;
        if agent_open {
            agent_w -= (over - d).clamp(0.0, agent_w - AGENT_MIN);
        }
    }
    let files = Rect::from_min_size(full.min, vec2(files_w, full.height()));
    let agent = agent_open.then(|| Rect::from_min_max(pos2(full.right() - agent_w, full.top()), full.max));
    let dock_right = agent.map_or(full.right(), |a| a.left() - gap);
    let dock = Rect::from_min_max(pos2(dock_right - dock_w, full.top()), pos2(dock_right, full.bottom()));
    let editor = Rect::from_min_max(pos2(files.right() + gap, full.top()), pos2(dock.left() - gap, full.bottom()));
    Places { files, editor, dock, agent }
}

/// A seam to drag: a grip, lit while held. Returns how far it moved and
/// whether it was double-clicked.
fn seam(ui: &mut Ui, p: &Palette, salt: &str, rect: Rect) -> (f32, bool) {
    let r = ui.interact(rect, egui::Id::new(("tex-seam", salt)), Sense::click_and_drag());
    let hot = r.hovered() || r.dragged();
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("tex-seams")));
    let c = rect.center();
    if r.dragged() {
        painter.vline(c.x, rect.y_range(), egui::Stroke::new(1.5, p.accent));
    }
    painter.rect_filled(Rect::from_center_size(c, vec2(4.0, 44.0)), CornerRadius::same(2), if hot { p.accent } else { p.wash(0x1a) });
    let r = r.on_hover_text("Drag to resize · double-click to reset");
    (r.drag_delta().x, r.double_clicked())
}

/// Text laid out on one line no wider than `width`, cut short with an
/// ellipsis where it would run on — into the next island, or under a control.
pub(crate) fn one_line(ui: &Ui, text: &str, font: egui::FontId, colour: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, colour);
    job.wrap = egui::text::TextWrapping { max_width: width.max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// A row of the file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRow {
    pub path: PathBuf,
    /// Its name, under its folder.
    pub name: String,
    pub depth: usize,
    pub dir: bool,
}

/// The files of a project, folders first at each level, by name; hidden
/// ones and build output passed over. `MOST_FILES` at most.
pub(crate) fn walk(root: &Path) -> Vec<FileRow> {
    fn visit(dir: &Path, depth: usize, out: &mut Vec<FileRow>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<(bool, String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let meta = std::fs::symlink_metadata(e.path()).ok()?;
                if name.starts_with('.') || meta.file_type().is_symlink() {
                    return None;
                }
                Some((!meta.is_dir(), name, e.path()))
            })
            .collect();
        entries.sort_by(|a, b| (a.0, a.1.to_lowercase()).cmp(&(b.0, b.1.to_lowercase())));
        for (file, name, path) in entries {
            if out.len() >= MOST_FILES {
                return;
            }
            if !file && PASSED.contains(&name.as_str()) {
                continue;
            }
            out.push(FileRow { path: path.clone(), name, depth, dir: !file });
            if !file {
                visit(&path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(root, 0, &mut out);
    out
}

/// The file a project is opened at: `main.tex`, or else the first `.tex`
/// that begins a document, or else the first `.tex`.
pub(crate) fn main_file(files: &[FileRow]) -> Option<PathBuf> {
    let tex: Vec<&FileRow> = files.iter().filter(|f| !f.dir && f.name.ends_with(".tex")).collect();
    if let Some(main) = tex.iter().find(|f| f.depth == 0 && f.name == "main.tex") {
        return Some(main.path.clone());
    }
    let begins = |f: &&&FileRow| std::fs::read_to_string(&f.path).is_ok_and(|t| t.contains("\\documentclass"));
    tex.iter().find(begins).or(tex.first()).map(|f| f.path.clone())
}

/// A key or text event as the editor takes it; `None` for what is not the
/// editor's — ⌘ chords, which are the application's, and key releases.
pub(crate) fn key_of(event: &egui::Event) -> Vec<Key> {
    match event {
        egui::Event::Text(text) => text
            .chars()
            .map(|c| {
                let mut k = Key::char(c);
                k.shift = c.is_uppercase();
                k
            })
            .collect(),
        egui::Event::Key { key, pressed: true, modifiers, .. } => {
            if modifiers.mac_cmd || (modifiers.command && !cfg!(target_os = "macos") && !modifiers.ctrl) {
                return Vec::new();
            }
            use egui::Key as E;
            let named = match key {
                E::Escape => Some("Escape"),
                E::Enter => Some("Enter"),
                E::Backspace => Some("Backspace"),
                E::Tab => Some("Tab"),
                E::Delete => Some("Delete"),
                E::ArrowUp => Some("ArrowUp"),
                E::ArrowDown => Some("ArrowDown"),
                E::ArrowLeft => Some("ArrowLeft"),
                E::ArrowRight => Some("ArrowRight"),
                E::Home => Some("Home"),
                E::End => Some("End"),
                E::PageUp => Some("PageUp"),
                E::PageDown => Some("PageDown"),
                _ => None,
            };
            if modifiers.ctrl {
                // ⌃[ is Escape, as in every terminal.
                if *key == E::OpenBracket {
                    return vec![Key::named("Escape")];
                }
                let name = key.symbol_or_name();
                let mut c = name.chars();
                if let (Some(c), None) = (c.next(), c.next()) {
                    return vec![Key::ctrl(c.to_ascii_lowercase())];
                }
            }
            match named {
                Some(name) => {
                    let mut k = Key::named(name);
                    k.shift = modifiers.shift;
                    k.alt = modifiers.alt;
                    k.ctrl = modifiers.ctrl;
                    vec![k]
                }
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// An open project.
pub(crate) struct TexProject {
    pub root: PathBuf,
    pub session: Session,
    files: Vec<FileRow>,
    /// A highlighter for each file open, by its path.
    syntaxes: HashMap<PathBuf, Option<Syntax>>,
    /// What the editor last said, and whether it was a warning.
    said: Option<(String, bool)>,
    /// How far each split is scrolled sideways, in cells.
    lefts: Vec<usize>,
    /// texlab: told what each file says, asked for completions, and what
    /// it says of the files kept.
    pub servers: Servers,
    pub(crate) popup: Popup,
    /// When a server was last told something, so its answer is watched for.
    told: Option<std::time::Instant>,
    /// Where the cursor was drawn last, for the popup to sit by.
    cursor_at: Option<Rect>,
    /// Quark's own texlab, once it has been given it.
    pub own_texlab: Option<PathBuf>,
    /// Its builds, and what the last came to.
    pub builds: crate::texbuild::Builder,
    /// A typesetting asked for — by a save, ⌘B or the dock — not yet begun.
    pub wants_build: bool,
    /// ⌘↩: the line being edited, shown in the PDF.
    forward: bool,
    /// Which of the dock's views shows.
    pub view: DockView,
    /// The document's headings and words, as last read, and of which file.
    outline: (Option<PathBuf>, quark_tex::outline::Outline),
    /// The cite picker, while it is open.
    pub(crate) picker: Option<Picker>,
    /// The visual editor, and whether it shows in place of the source.
    pub visual: crate::texvisual::Visual,
    /// The agent: Claude Code or Antigravity, working on the files.
    pub agent: crate::texagent::Agent,
    /// How wide each pane was dragged.
    pub layout: quark_core::prefs::TexLayout,
}

/// The cite picker: what is typed, what was found, which is chosen.
#[derive(Default)]
pub(crate) struct Picker {
    pub cite: crate::cite::Cite,
    pub field: String,
    pub chosen: usize,
    focused: bool,
}

impl Picker {
    /// Everything found, the document's own first.
    pub(crate) fn all(&self) -> Vec<quark_tex::bib::Citation> {
        self.cite.local.iter().chain(self.cite.online_citations.iter()).cloned().collect()
    }
}

/// The dock's views, once TeX is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DockView {
    #[default]
    Pdf,
    Log,
    Tex,
}

/// What the dock asks of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TexAsk {
    Env(crate::texdock::EnvAction),
    Typeset,
}

/// The PDF and what draws it, lent to the dock for a frame.
pub(crate) struct PdfView<'a> {
    pub tab: Option<&'a mut crate::tab::Tab>,
    pub service: &'a quark_pdf::service::PdfService,
    pub textures: &'a mut crate::textures::TextureCache,
    pub prefs: &'a quark_core::prefs::Prefs,
}

/// A SyncTeX box, top-left points as SyncTeX has it, as page space — PDF
/// points from the bottom left — on a page `height` points tall.
pub(crate) fn synctex_rect(r: &quark_tex::synctex::Rect, height: f32) -> quark_core::geom::Rect {
    use quark_core::geom::{Rect as R, Vec2 as V};
    R::new(V::new(r.x, height - r.y - r.h), V::new(r.x + r.w, height - r.y))
}

/// What a key does while the completion list is showing: moves in it,
/// takes the chosen one, closes it — or is the editor's, as typing that
/// narrows the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKey {
    Move(i32),
    Accept,
    Close,
    /// Closes it, and is the editor's too.
    CloseAndPass,
    Pass,
}

pub(crate) fn list_key(key: &Key) -> ListKey {
    match key.key.as_str() {
        "Tab" | "ArrowDown" => ListKey::Move(1),
        "ArrowUp" => ListKey::Move(-1),
        "n" if key.ctrl => ListKey::Move(1),
        "p" if key.ctrl => ListKey::Move(-1),
        "Enter" => ListKey::Accept,
        // Escape closes the list and stays in insert mode: leaving insert
        // too would punish anyone who wanted to keep typing.
        "Escape" => ListKey::Close,
        // The list no longer describes what is there.
        "Backspace" => ListKey::CloseAndPass,
        _ => ListKey::Pass,
    }
}

/// Whether a character typed in insert mode asks the server again: one it
/// names (texlab's `\`, `{`, `,`), or a letter while the list is showing,
/// which narrows it.
pub(crate) fn asks_again(c: char, triggers: bool, open: bool) -> bool {
    triggers || (open && (c.is_alphanumeric() || c == '_' || c == ':'))
}

impl TexProject {
    /// The project in `root`, at its main file.
    pub(crate) fn open(root: &Path) -> TexProject {
        let root = root.to_path_buf();
        let files = walk(&root);
        let mut session = Session::new(Some(&root));
        let mut said = None;
        if let Some(main) = main_file(&files) {
            if let Err(e) = session.open(&main) {
                said = Some((format!("{}: {e}", main.display()), true));
            }
        }
        let mut servers = Servers::new(&root);
        servers.enable();
        let mut project = TexProject {
            root,
            session,
            files,
            syntaxes: HashMap::new(),
            said,
            lefts: Vec::new(),
            servers,
            popup: Popup::default(),
            told: None,
            cursor_at: None,
            own_texlab: None,
            builds: Default::default(),
            wants_build: false,
            forward: false,
            view: DockView::Pdf,
            outline: (None, Default::default()),
            picker: None,
            visual: Default::default(),
            agent: Default::default(),
            layout: Default::default(),
        };
        project.read_outline();
        project.sync();
        project
    }

    /// The main file citing is for: the document the file being edited is
    /// part of.
    fn main(&self) -> Option<PathBuf> {
        let active = self.session.editor().buffer().path().map(Path::to_path_buf);
        crate::texbuild::main_of(&self.root, active.as_deref())
    }

    /// Open the cite picker, at what was last searched.
    pub(crate) fn open_picker(&mut self) {
        let mut picker = self.picker.take().unwrap_or_default();
        picker.focused = false;
        let main = self.main();
        let query = picker.field.clone();
        picker.cite.search(main.as_deref(), &query);
        self.picker = Some(picker);
    }

    /// Cite what is chosen: its key put at the cursor — inside a `\cite{`
    /// already open, or as a `\cite{…}` of its own — and an online paper
    /// added to the document's `.bib` first.
    fn cite_chosen(&mut self) {
        let Some(picker) = &self.picker else { return };
        let Some(chosen) = picker.all().get(picker.chosen).cloned() else { return };
        let key = if chosen.source == "online" {
            let (Some(main), Some(record)) = (self.main(), picker.cite.online_record(&chosen.id)) else { return };
            match crate::cite::cite_record(&main, &record) {
                Ok(cited) => {
                    if let Some(bib) = &cited.added {
                        let name = bib.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let extra = if cited.unnamed {
                            format!(" — the document names no .bib: add \\bibliography{{{}}}", name.trim_end_matches(".bib"))
                        } else {
                            String::new()
                        };
                        self.said = Some((format!("added {} to {name}{extra}", cited.key), cited.unnamed));
                        // The .bib, if it is open, read again: it changed on disk.
                        self.session.editor_mut().reread_changed_on_disk();
                        self.files = walk(&self.root);
                    }
                    cited.key
                }
                Err(why) => {
                    self.said = Some((why, true));
                    return;
                }
            }
        } else {
            chosen.key.clone()
        };
        let buffer = self.session.editor_mut().buffer_mut();
        let at = buffer.cursor.offset;
        let here = buffer.position(at);
        let before: String = buffer.line(here.line).chars().take(here.column).collect();
        let text = crate::cite::cite_text(&before, &key);
        let n = text.chars().count();
        buffer.apply(vec![quark_text::Edit::insert(at, text)], quark_text::Cursor::at(at + n));
        self.picker = None;
        self.sync();
    }

    /// The cite picker, over the editor.
    fn picker_sheet(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(picker) = &mut self.picker else { return };
        let main = {
            let active = self.session.editor().buffer().path().map(Path::to_path_buf);
            crate::texbuild::main_of(&self.root, active.as_deref())
        };
        let mut close = false;
        let mut cite = false;
        let mut online = false;
        let modal = egui::Modal::new(egui::Id::new("quark-cite")).show(ui.ctx(), |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Cite").font(theme::strong(15.0)).color(p.text));
            ui.add_space(8.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut picker.field)
                    .hint_text("Authors, a title, a year, a key")
                    .desired_width(f32::INFINITY),
            );
            if !picker.focused {
                field.request_focus();
                picker.focused = true;
            }
            if field.changed() {
                picker.cite.search(main.as_deref(), &picker.field.clone());
                picker.chosen = 0;
            }
            let all = picker.all();
            let (down, up, enter) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                )
            });
            if down && picker.chosen + 1 < all.len() {
                picker.chosen += 1;
            }
            if up {
                picker.chosen = picker.chosen.saturating_sub(1);
            }
            if enter && !all.is_empty() {
                cite = true;
            }
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                let local = picker.cite.local.len();
                if all.is_empty() {
                    let none = if picker.field.trim().is_empty() { "Type to search the document's .bib." } else { "Nothing in the document's .bib." };
                    ui.label(RichText::new(none).font(theme::body(12.5)).color(p.text_faint));
                }
                for (i, c) in all.iter().enumerate() {
                    if i == local && local > 0 || (i == 0 && local == 0) && c.source == "online" {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Online").font(theme::strong(11.5)).color(p.text_faint));
                    }
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
                    if i == picker.chosen {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), p.accent_soft);
                    }
                    let byline = [Some(c.authors.clone()), c.year.clone(), c.venue.clone()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                    // The key on the right; the title cut to what is left of it.
                    let key = ui.painter().text(rect.right_top() + vec2(-10.0, 6.0), egui::Align2::RIGHT_TOP, &c.key, code::font(), if c.source == "online" { p.text_faint } else { p.accent_text });
                    let one_line = |text: &str, font: egui::FontId, colour: egui::Color32, width: f32| {
                        let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(font, colour));
                        job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                        job
                    };
                    let room = (key.left() - rect.left() - 26.0).max(40.0);
                    let title = ui.painter().layout_job(one_line(&c.title, theme::strong(12.5), p.text, room));
                    ui.painter().galley(rect.left_top() + vec2(10.0, 6.0), title, p.text);
                    let by = ui.painter().layout_job(one_line(&byline, theme::body(11.5), p.text_muted, rect.width() - 20.0));
                    ui.painter().galley(rect.left_top() + vec2(10.0, 25.0), by, p.text_muted);
                    if response.clicked() {
                        picker.chosen = i;
                        cite = true;
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if picker.cite.searching() {
                    ui.add(egui::Spinner::new().size(13.0));
                    ui.label(RichText::new("Asking arXiv, Semantic Scholar and OpenAlex…").font(theme::body(12.0)).color(p.text_faint));
                } else if quark_ui::widgets::ghost_button(ui, p, "Search online")
                    .on_hover_text("arXiv, Semantic Scholar and OpenAlex: what is typed is sent to them")
                    .clicked()
                {
                    online = true;
                }
                if let Some(note) = &picker.cite.note {
                    ui.label(RichText::new(note).font(theme::body(11.5)).color(p.text_faint));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if quark_ui::widgets::ghost_button(ui, p, "Close").clicked() {
                        close = true;
                    }
                });
            });
        });
        if online {
            if let Some(job) = picker.cite.job() {
                let fetch = quark_literature::Fetch::start(&crate::cite::context(), job);
                picker.cite.online(fetch);
            }
        }
        if close || modal.should_close() {
            self.picker = None;
            return;
        }
        if cite {
            self.cite_chosen();
        }
    }

    /// The headings, read again from the files as saved.
    fn read_outline(&mut self) {
        let active = self.session.editor().buffer().path().map(Path::to_path_buf);
        self.outline = crate::texbuild::outline_of(&self.root, active.as_deref());
    }

    /// Open `file` — a path in the project, or whole — at `line`, one-based.
    pub(crate) fn open_at(&mut self, file: &str, line: u32) {
        let path = if Path::new(file).is_absolute() { PathBuf::from(file) } else { self.root.join(file) };
        if let Err(e) = self.session.open(&path) {
            self.said = Some((format!("{file}: {e}"), true));
            return;
        }
        let view = self.session.splits().focused();
        self.session.click(view, line.saturating_sub(1) as usize, 0);
        self.popup.close();
        self.sync();
    }

    /// texlab is Quark's own now: started from there.
    pub(crate) fn set_texlab(&mut self, program: PathBuf) {
        self.servers.set_program("latex", program.clone());
        self.own_texlab = Some(program);
        self.sync();
    }

    /// The file in focus, as the server should have it.
    fn sync(&mut self) {
        self.servers.sync(self.session.editor().buffer());
        // Its diagnostics come back a moment later, on a thread.
        self.told = Some(std::time::Instant::now());
    }

    /// What the servers have said since; whether to look again soon.
    pub(crate) fn poll(&mut self) -> bool {
        let main = self.main();
        let searching = match &mut self.picker {
            Some(picker) => {
                picker.cite.poll(main.as_deref());
                picker.cite.searching()
            }
            None => false,
        };
        for (_, message) in self.servers.poll() {
            match message {
                Incoming::Completion { id, items } => {
                    self.popup.fill(id, items);
                }
                Incoming::Failed { text } => self.said = Some((text, true)),
                _ => {}
            }
        }
        // Watched for a few seconds after it was told something, and while a
        // list is wanted: answers come on a thread, and nothing else wakes
        // the window for them.
        self.told.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(4)) || self.popup.is_open() || searching
    }

    /// Ask for completions where the cursor is.
    fn ask_completion(&mut self) {
        let buffer = self.session.editor().buffer();
        let Some(path) = buffer.path().map(Path::to_path_buf) else { return };
        let at = servers::position_of(buffer, buffer.cursor.offset);
        if let Some(id) = self.servers.completion(&path, at) {
            self.popup.expect(id);
            self.told = Some(std::time::Instant::now());
        }
    }

    /// One key: the list's first, when it is showing, then the editor's.
    fn key(&mut self, key: &Key) -> Vec<Said> {
        if self.popup.is_open() {
            match list_key(key) {
                ListKey::Move(by) => {
                    self.popup.move_by(by);
                    return Vec::new();
                }
                ListKey::Accept => {
                    if let Some(item) = self.popup.chosen().cloned() {
                        self.popup.close();
                        servers::apply_completion(self.session.editor_mut().buffer_mut(), &item);
                    }
                    return Vec::new();
                }
                ListKey::Close => {
                    self.popup.close();
                    return Vec::new();
                }
                ListKey::CloseAndPass => self.popup.close(),
                ListKey::Pass => {}
            }
        }
        let was = self.session.mode();
        let said = self.session.key(key);
        let typed = key.as_char().filter(|_| key.is_bare_char());
        if was == VimMode::Insert && self.session.mode() == VimMode::Insert {
            if let Some(c) = typed {
                let path = self.session.editor().buffer().path().map(Path::to_path_buf);
                let triggers = path.as_deref().is_some_and(|p| self.servers.can(p).triggers_on(c));
                if asks_again(c, triggers, self.popup.is_open()) {
                    self.sync();
                    self.ask_completion();
                } else if self.popup.is_open() {
                    self.popup.close();
                }
            }
        } else if self.popup.is_open() {
            self.popup.close();
        }
        said
    }

    pub(crate) fn name(&self) -> String {
        self.root.file_name().map_or_else(|| self.root.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    /// The files with changes not written — typing in the visual editor
    /// not yet in its file's source among them.
    pub(crate) fn unsaved(&self) -> Vec<String> {
        let mut unsaved = self.session.unsaved();
        let buffer = self.session.editor().buffer();
        if self.visual.pending() && !buffer.is_modified() {
            unsaved.push(buffer.name());
        }
        unsaved
    }

    /// ⌘S: the file in focus written, with what is typed in it.
    pub(crate) fn save(&mut self) {
        self.visual.flush(&mut self.session);
        let said = self.session.save();
        self.tell(said);
    }

    /// What the editor's area shows — the source, the page or both — and
    /// the source's cursor put at character `at`, if one is given.
    /// Where the writer is, for the agent: the file in focus, the cursor's
    /// line, and what the last typesetting complained of.
    fn agent_context(&self) -> crate::texagent::Context {
        let buffer = self.session.editor().buffer();
        let line = buffer.position(buffer.cursor.offset).line;
        let log = self
            .builds
            .log
            .as_ref()
            .map(|l| {
                l.entries
                    .iter()
                    .filter(|e| e.kind != quark_tex::log::Kind::Box)
                    .map(|e| format!("{}{}: {}", e.file.as_deref().unwrap_or(""), e.line.map(|n| format!(":{n}")).unwrap_or_default(), e.message))
                    .collect()
            })
            .unwrap_or_default();
        crate::texagent::Context { root: self.root.clone(), file: buffer.path().map(Path::to_path_buf), line, log }
    }

    /// What the agent asked for: a change shown, or a typesetting.
    fn agent_asks(&mut self, asks: Vec<crate::texagent::AgentAsk>) {
        for ask in asks {
            match ask {
                crate::texagent::AgentAsk::Show(path, line) => {
                    if self.session.open(&path).is_ok() {
                        let buffer = self.session.editor().buffer();
                        let at = buffer.offset_of(quark_text::Position { line: line.min(buffer.len_lines().saturating_sub(1)), column: 0 });
                        self.session.put_cursor(at);
                        if self.visual.pane == Pane::Visual {
                            self.set_pane(Pane::Both, None);
                        }
                    }
                }
                crate::texagent::AgentAsk::Typeset => {
                    // A file made or removed belongs in the list.
                    self.files = walk(&self.root);
                    self.wants_build = true;
                }
            }
        }
    }

    fn review_press(&mut self, press: crate::texagent::BarPress) {
        use crate::texagent::BarPress;
        let asks = match press {
            BarPress::Step(back) => {
                let Some(review) = &mut self.agent.review else { return };
                review.step(back);
                review.here().map(|(path, line)| vec![crate::texagent::AgentAsk::Show(path, line)]).unwrap_or_default()
            }
            BarPress::Keep | BarPress::Undo => {
                let (err, asks) = self.agent.decide(matches!(press, BarPress::Keep), false, &mut self.session);
                if let Some(e) = err {
                    self.said = Some((format!("could not undo: {e}"), true));
                }
                asks
            }
        };
        self.agent_asks(asks);
    }

    pub(crate) fn set_pane(&mut self, pane: Pane, at: Option<usize>) {
        self.visual.flush(&mut self.session);
        if let Some(at) = at {
            self.session.put_cursor(at);
        }
        if pane == self.visual.pane {
            return;
        }
        self.popup.close();
        // Typing in the source ends when only the page is left, as one
        // step to undo.
        if pane == Pane::Visual && self.session.mode() != VimMode::Normal {
            let said = self.session.key(&Key::named("Escape"));
            self.tell(said);
        }
        self.visual.pane = pane;
    }

    /// What the visual editor asked for.
    fn visual_asked(&mut self, ctx: &egui::Context, asked: crate::texvisual::Asked) {
        use crate::texvisual::Asked;
        match asked {
            Asked::None => {}
            // Beside the source, to the source: the keys go to it.
            Asked::Source(at) if self.visual.pane == Pane::Both => {
                self.set_pane(Pane::Both, Some(at));
                ctx.memory_mut(|m| m.stop_text_input());
            }
            Asked::Source(at) => self.set_pane(Pane::Source, Some(at)),
            Asked::Step(at) => {
                ctx.memory_mut(|m| m.stop_text_input());
                self.set_pane(self.visual.pane.next(), Some(at));
            }
            Asked::Forward(at) => {
                self.session.put_cursor(at);
                self.view = DockView::Pdf;
                self.forward = true;
            }
            Asked::Cite(at) => {
                self.session.put_cursor(at);
                self.open_picker();
            }
        }
        if let Some(said) = self.visual.said.take() {
            self.said = Some((said, true));
        }
    }

    /// Keys while only the page shows and nothing on it is typed in: its
    /// own, and the workspace's chords — never Vim's.
    fn visual_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in &events {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
            let command = modifiers.mac_cmd || modifiers.command;
            match key {
                egui::Key::E if command => self.set_pane(self.visual.pane.next(), None),
                egui::Key::Z if command => self.visual.undo(&mut self.session, modifiers.shift),
                egui::Key::B if command => self.wants_build = true,
                egui::Key::C if command && modifiers.shift => self.open_picker(),
                egui::Key::Enter if command => {
                    self.view = DockView::Pdf;
                    self.forward = true;
                }
                _ => {}
            }
        }
    }

    /// The server's word on the file in focus at the cursor's line, if any.
    fn diagnostic_here(&self) -> Option<(String, bool)> {
        let buffer = self.session.editor().buffer();
        let path = buffer.path()?;
        let line = buffer.position(buffer.cursor.offset).line;
        let raw = self.servers.diagnostics_for(path);
        let mut here: Vec<servers::Diagnostic> =
            raw.iter().map(|d| servers::diagnostic(buffer, d)).filter(|d| d.line <= line && line <= d.end_line).collect();
        here.sort_by_key(|d| d.severity as u8);
        let first = here.first()?;
        Some((first.message.clone(), first.severity == servers::Severity::Error))
    }

    /// Keys for the editor, as they came this frame; what it said kept for
    /// the status line, what it yanked put on the clipboard.
    pub(crate) fn input(&mut self, ctx: &egui::Context) {
        // ⌘⇧A: the agent, whichever pane has the keys.
        let agent = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::A));
        if agent {
            self.agent.open = !self.agent.open;
            self.agent.focus = self.agent.open;
        }
        if self.visual.pane == Pane::Visual {
            return self.visual_input(ctx);
        }
        let events = ctx.input(|i| i.events.clone());
        let mut said = Vec::new();
        for event in &events {
            match event {
                egui::Event::Paste(text) => self.session.paste(text),
                egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if (modifiers.mac_cmd || modifiers.command) && modifiers.shift => {
                    self.open_picker();
                }
                egui::Event::Key { key: egui::Key::B, pressed: true, modifiers, .. } if modifiers.mac_cmd || modifiers.command => {
                    self.wants_build = true;
                }
                egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.mac_cmd || modifiers.command => {
                    self.forward = true;
                }
                egui::Event::Key { key: egui::Key::E, pressed: true, modifiers, .. } if modifiers.mac_cmd || modifiers.command => {
                    self.set_pane(self.visual.pane.next(), None);
                }
                // ⌘C and ⌘X with a selection: yanked and deleted, so the
                // clipboard gets it as `y` and `d` give it.
                egui::Event::Copy if self.session.vim().is_selecting() => said.extend(self.session.key(&Key::char('y'))),
                egui::Event::Cut if self.session.vim().is_selecting() => said.extend(self.session.key(&Key::char('d'))),
                _ => {
                    for key in key_of(event) {
                        said.extend(self.key(&key));
                    }
                }
            }
        }
        if let Some(text) = self.session.take_clipboard() {
            ctx.copy_text(text);
        }
        self.tell(said);
        self.sync();
    }

    fn tell(&mut self, said: Vec<Said>) {
        for s in said {
            self.said = match s {
                Said::Info(text) => Some((text, false)),
                Said::Warn(text) => Some((text, true)),
                Said::Wrote(path) => {
                    self.servers.saved(self.session.editor().buffer());
                    if crate::texbuild::is_source(&path) {
                        self.wants_build = true;
                        self.read_outline();
                    }
                    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    // A file made by `:w` or `:e` belongs in the list.
                    if !self.files.iter().any(|f| f.path == path) {
                        self.files = walk(&self.root);
                    }
                    Some((format!("wrote {name}"), false))
                }
                Said::Ex(line) => Some((format!("not an editor command: :{}", line.trim_start_matches(':')), true)),
                Said::Leader(_) => self.said.take(),
            };
        }
    }

    /// The workspace: its files, the editor's splits and its status line,
    /// and the dock, where TeX stands; what the dock asked for.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        env: Option<&quark_lab::tex::TexState>,
        dock: &mut crate::texdock::Dock,
        pdf: Option<PdfView>,
    ) -> Option<TexAsk> {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        // What the agent did since the last frame.
        let asks = self.agent.tend(&mut self.session, &self.root, ui.ctx());
        self.agent_asks(asks);
        // Wide enough to read a page in, once there is TeX to make one.
        let ready = matches!(env, Some(quark_lab::tex::TexState::Ready { .. }));
        // Narrower when the source and the page share the middle.
        let share = match (self.agent.open, self.visual.pane == Pane::Both) {
            // The agent takes its own column; the page gives up some of it.
            (true, true) => 0.22,
            (true, false) => 0.28,
            (false, true) => 0.3,
            (false, false) => 0.4,
        };
        let auto_dock = if ready { (full.width() * share).clamp(DOCK_W, 760.0) } else { DOCK_W };
        let places = place(full, &self.layout, auto_dock, self.agent.open);
        let (files, editor, side) = (places.files, places.editor, places.dock);
        let painter = ui.ctx().layer_painter(ui.layer_id());
        theme::paint_island(&painter, files, p);
        theme::paint_island(&painter, editor, p);
        theme::paint_island(&painter, side, p);
        if let Some(a) = places.agent {
            theme::paint_island(&painter, a, p);
            let mut agent_ui = ui.new_child(egui::UiBuilder::new().max_rect(a.shrink2(vec2(10.0, 10.0))));
            agent_ui.set_clip_rect(a.shrink(2.0));
            let cx = self.agent_context();
            let asks = crate::texagent::panel(&mut agent_ui, p, &mut self.agent, &mut self.session, &cx);
            self.agent_asks(asks);
        }
        let mut asked = None;
        if let Some(state) = env {
            let ready = matches!(state, quark_lab::tex::TexState::Ready { .. });
            let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink2(vec2(14.0, 12.0))));
            side_ui.set_clip_rect(side.shrink(2.0));
            if ready {
                asked = self.dock(&mut side_ui, p, state, dock, pdf);
            } else {
                egui::ScrollArea::vertical().id_salt("tex-dock").auto_shrink([false, false]).show(&mut side_ui, |ui| {
                    asked = crate::texdock::show(ui, p, state, dock).map(TexAsk::Env);
                });
            }
        }

        // The files above, the outline below when there is one.
        let inner_files = files.shrink2(vec2(10.0, 12.0));
        let outline_h = if self.outline.1.headings.is_empty() { 0.0 } else { (inner_files.height() * 0.45).min(360.0) };
        let list = Rect::from_min_max(inner_files.min, pos2(inner_files.right(), inner_files.bottom() - outline_h));
        let mut files_ui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        files_ui.set_clip_rect(files.shrink(2.0));
        self.file_list(&mut files_ui, p);
        if outline_h > 0.0 {
            let area = Rect::from_min_max(pos2(inner_files.left(), list.bottom() + 6.0), inner_files.max);
            let mut outline_ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
            outline_ui.set_clip_rect(files.shrink(2.0));
            self.outline_list(&mut outline_ui, p);
        }

        let inner = editor.shrink(1.0);
        let status = Rect::from_min_max(pos2(inner.left(), inner.bottom() - STATUS_H), inner.max);
        let panes = Rect::from_min_max(inner.min, pos2(inner.right(), status.top()));
        match self.visual.pane {
            Pane::Source => self.splits(ui, p, panes),
            Pane::Visual => {
                let asked = self.visual.show(ui, p, &mut self.session, &self.root, panes);
                self.visual_asked(ui.ctx(), asked);
            }
            Pane::Both => {
                // The source on the left, the page on the right, a hairline
                // between.
                let mid = panes.left() + panes.width() * self.layout.split.clamp(SPLIT.0, SPLIT.1);
                let source = Rect::from_min_max(panes.min, pos2(mid - 0.5, panes.bottom()));
                let page = Rect::from_min_max(pos2(mid + 0.5, panes.top()), panes.max);
                self.splits(ui, p, source);
                ui.painter().vline(mid, panes.y_range(), egui::Stroke::new(1.0, p.wash(0x18)));
                let asked = self.visual.show(ui, p, &mut self.session, &self.root, page);
                self.visual_asked(ui.ctx(), asked);
            }
        }
        // The agent's changes, a bar to go through them over the source.
        if self.visual.pane != Pane::Visual {
            let source_right = if self.visual.pane == Pane::Both { panes.left() + panes.width() * self.layout.split.clamp(SPLIT.0, SPLIT.1) } else { panes.right() };
            if let Some(review) = &self.agent.review {
                if let Some(press) = crate::texagent::review_bar(ui, p, pos2(source_right - 10.0, panes.top() + 10.0), review) {
                    self.review_press(press);
                }
            }
        }

        // The seams, over everything, so they win the pointer at the edges.
        let gap = |left: Rect, right: Rect| Rect::from_min_max(pos2(left.right(), full.top()), pos2(right.left(), full.bottom()));
        let (dx, reset) = seam(ui, p, "files", gap(files, editor));
        if reset {
            self.layout.files = quark_core::prefs::TexLayout::default().files;
        } else if dx != 0.0 {
            self.layout.files = (files.width() + dx).clamp(FILES_MIN, FILES_MAX);
        }
        let (dx, reset) = seam(ui, p, "dock", gap(editor, side));
        if reset {
            self.layout.dock = None;
        } else if dx != 0.0 {
            self.layout.dock = Some((side.width() - dx).max(DOCK_MIN));
        }
        if let Some(a) = places.agent {
            let (dx, reset) = seam(ui, p, "agent", gap(side, a));
            if reset {
                self.layout.agent = quark_core::prefs::TexLayout::default().agent;
            } else if dx != 0.0 {
                // Between the dock and the agent: one grows as the other gives.
                self.layout.agent = (a.width() - dx).clamp(AGENT_MIN, AGENT_MAX);
                self.layout.dock = Some((side.width() + dx).max(DOCK_MIN));
            }
        }
        if self.visual.pane == Pane::Both {
            let mid = panes.left() + panes.width() * self.layout.split.clamp(SPLIT.0, SPLIT.1);
            let (dx, reset) = seam(ui, p, "split", Rect::from_min_max(pos2(mid - SEAM / 2.0, panes.top()), pos2(mid + SEAM / 2.0, panes.bottom())));
            if reset {
                self.layout.split = 0.5;
            } else if dx != 0.0 {
                self.layout.split = ((mid + dx - panes.left()) / panes.width()).clamp(SPLIT.0, SPLIT.1);
            }
        }

        self.status_line(ui, p, status);
        let switch = Rect::from_min_size(pos2(status.right() - 250.0, status.center().y - 10.0), vec2(168.0, 20.0));
        if let Some(pane) = crate::texvisual::switch(ui, p, switch, self.visual.pane) {
            self.set_pane(pane, None);
        }
        self.picker_sheet(ui, p);
        asked
    }

    fn file_list(&mut self, ui: &mut Ui, p: &Palette) {
        // The buttons first, from the right; the name in what they leave.
        let name = self.name();
        let mut refresh = false;
        let mut cite = false;
        let row = vec2(ui.available_width(), 24.0);
        ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
            refresh = ui.small_button("↻").on_hover_text("Look at the folder again").clicked();
            cite = ui.small_button("Cite").on_hover_text("Cite a paper  ⌘⇧C").clicked();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(&name).font(theme::strong(13.0)).color(p.text)).truncate())
                    .on_hover_text(self.root.display().to_string());
            });
        });
        if refresh {
            self.files = walk(&self.root);
        }
        if cite {
            self.open_picker();
        }
        ui.add_space(6.0);
        let active = self.session.editor().buffer().path().map(Path::to_path_buf);
        let unsaved: Vec<PathBuf> = self
            .session
            .editor()
            .unsaved()
            .iter()
            .filter_map(|b| b.path().map(Path::to_path_buf))
            .collect();
        let mut open = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for row in &self.files {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                let here = active.as_deref().is_some_and(|a| quark_modal::root::Root::new(&self.root).same(a, &row.path));
                if here {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), p.accent_soft);
                } else if response.hovered() && !row.dir {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), p.wash(0x08));
                }
                let x = rect.left() + 8.0 + row.depth as f32 * 12.0;
                let (text, colour) = if row.dir {
                    (format!("{}/", row.name), p.text_faint)
                } else if row.name.ends_with(".tex") || row.name.ends_with(".bib") {
                    (row.name.clone(), if here { p.accent_text } else { p.text })
                } else {
                    (row.name.clone(), p.text_muted)
                };
                // Room for the unsaved dot at the right.
                let galley = one_line(ui, &text, theme::body(12.5), colour, rect.right() - 20.0 - x);
                let full = galley.elided;
                ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, colour);
                if full {
                    response.clone().on_hover_text(&row.name);
                }
                if unsaved.iter().any(|u| quark_modal::root::Root::new(&self.root).same(u, &row.path)) {
                    ui.painter().circle_filled(pos2(rect.right() - 10.0, rect.center().y), 3.0, p.accent);
                }
                if !row.dir && response.clicked() {
                    open = Some(row.path.clone());
                }
            }
        });
        if let Some(path) = open {
            if let Err(e) = self.session.open(&path) {
                self.said = Some((format!("{}: {e}", path.display()), true));
            }
            self.popup.close();
            self.sync();
        }
    }

    /// The document's headings, each a click from its line, and its words.
    fn outline_list(&mut self, ui: &mut Ui, p: &Palette) {
        ui.painter().hline(ui.max_rect().x_range(), ui.max_rect().top() - 3.0, egui::Stroke::new(1.0, p.wash(0x10)));
        ui.horizontal(|ui| {
            ui.label(RichText::new("Outline").font(theme::strong(12.5)).color(p.text_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{} words", self.outline.1.words)).font(theme::body(11.0)).color(p.text_faint));
            });
        });
        let mut go = None;
        egui::ScrollArea::vertical().id_salt("tex-outline").auto_shrink([false, false]).show(ui, |ui| {
            for h in &self.outline.1.headings {
                let indent = (h.level.saturating_sub(1)) as f32 * 10.0;
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 21.0), Sense::click());
                if response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), p.wash(0x08));
                }
                let x = rect.left() + 8.0 + indent;
                let font = if h.level <= 2 { theme::strong(12.0) } else { theme::body(12.0) };
                let galley = one_line(ui, &h.title, font, p.text, rect.right() - 6.0 - x);
                ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, p.text);
                if response.on_hover_text(format!("{}:{}", h.file, h.line)).clicked() {
                    go = Some((h.file.clone(), h.line));
                }
            }
        });
        if let Some((file, line)) = go {
            self.open_at(&file, line);
        }
    }

    /// The dock once TeX is installed: the PDF, the build's log, and TeX.
    fn dock(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        state: &quark_lab::tex::TexState,
        dock: &mut crate::texdock::Dock,
        pdf: Option<PdfView>,
    ) -> Option<TexAsk> {
        use crate::texbuild::Ended;
        use quark_ui::widgets::Segment;
        let mut asked = None;
        let issues = self.builds.log.as_ref().map_or(0, |l| l.entries.len());
        ui.horizontal(|ui| {
            let views = [Segment::Text("PDF"), Segment::Counted("Log", issues), Segment::Text("TeX")];
            let at = match self.view {
                DockView::Pdf => 0,
                DockView::Log => 1,
                DockView::Tex => 2,
            };
            if let Some(i) = quark_ui::widgets::segmented(ui, p, &views, at, false, 26.0) {
                self.view = [DockView::Pdf, DockView::Log, DockView::Tex][i];
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.builds.running() {
                    if quark_ui::widgets::ghost_button(ui, p, "Stop").clicked() {
                        self.builds.stop();
                    }
                    ui.add(egui::Spinner::new().size(14.0));
                } else if quark_ui::widgets::primary_button(ui, p, "Typeset", true).on_hover_text("⌘B — and every save of a .tex").clicked() {
                    asked = Some(TexAsk::Typeset);
                }
            });
        });
        // How the last build came out, in a line.
        if let Some(last) = &self.builds.last {
            let line = match last.state {
                Ended::Running => format!("Typesetting {} with {}… {:.0} s", last.main.as_deref().unwrap_or(""), last.engine, last.seconds),
                Ended::NoMain => "No file begins a document with \\documentclass.".into(),
                Ended::Stopped => "Stopped.".into(),
                Ended::TimedOut => "It took too long, and was stopped.".into(),
                Ended::Failed => format!("No PDF was made{}", last.said.as_deref().map(|s| format!(": {s}")).unwrap_or_else(|| ". The log says why.".into())),
                Ended::Done => {
                    let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
                    let pages = last.pages.map(|p| format!(" · {}", n(p as usize, "page", "pages"))).unwrap_or_default();
                    format!(
                        "{} · {} · {} in {:.1} s{pages}",
                        n(last.errors, "error", "errors"),
                        n(last.warnings, "warning", "warnings"),
                        last.engine,
                        last.seconds
                    )
                }
            };
            let warn = matches!(last.state, Ended::Failed | Ended::NoMain | Ended::TimedOut) || last.errors > 0;
            ui.label(RichText::new(line).font(theme::body(11.5)).color(if warn { p.warning } else { p.text_faint }));
        }
        ui.add_space(6.0);
        match self.view {
            DockView::Pdf => self.pdf_view(ui, p, pdf),
            DockView::Log => self.log_view(ui, p),
            DockView::Tex => {
                egui::ScrollArea::vertical().id_salt("tex-env").auto_shrink([false, false]).show(ui, |ui| {
                    if let Some(a) = crate::texdock::show(ui, p, state, dock) {
                        asked = Some(TexAsk::Env(a));
                    }
                });
            }
        }
        asked
    }

    /// The PDF last made, in Quark's viewer: double-click a place to go to
    /// the line that put it there.
    fn pdf_view(&mut self, ui: &mut Ui, p: &Palette, pdf: Option<PdfView>) {
        let Some(PdfView { tab: Some(tab), service, textures, prefs }) = pdf else {
            let words = if self.builds.running() {
                "Typesetting…"
            } else {
                "Typeset to see the PDF: ⌘B, or save a .tex file."
            };
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(words).font(theme::body(12.5)).color(p.text_faint)));
            return;
        };
        // SyncTeX forward: ⌘↩ in the editor.
        if std::mem::take(&mut self.forward) {
            let buffer = self.session.editor().buffer();
            if let Some(path) = buffer.path().map(Path::to_path_buf) {
                let line = buffer.position(buffer.cursor.offset).line as u32 + 1;
                match self.builds.forward(&self.root, &path, line) {
                    Ok(found) => {
                        let page = (found.page as usize).saturating_sub(1);
                        let height = tab.info.page_size(page).height;
                        if let Some(first) = found.rects.first() {
                            let rect = synctex_rect(first, height);
                            tab.reveal(page, rect);
                            tab.flash = Some((page, rect, None));
                        } else {
                            tab.go_to_page(page);
                        }
                    }
                    Err(why) => self.said = Some((why, false)),
                }
            }
        }
        let canvas = ui.available_rect_before_wrap();
        // SyncTeX back: a double-click is where something was typeset from.
        let double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
        let at = ui.input(|i| i.pointer.interact_pos()).filter(|at| canvas.contains(*at));
        let mut back = None;
        if let (true, Some(at)) = (double, at) {
            if let Some((page, point, size)) = crate::viewer::page_point(tab, canvas, at) {
                back = self.builds.inverse(page as u32 + 1, point.x as f64, (size.height - point.y) as f64);
            }
        }
        egui::Frame::new().corner_radius(CornerRadius::same(8)).show(ui, |ui| {
            let action = crate::viewer::show(ui, p, tab, service, textures, prefs, quark_core::tools::Tool::Select);
            if let crate::viewer::ViewerAction::GoToPage(page) = action {
                tab.go_to_page(page);
            }
        });
        if let Some((file, line)) = back {
            tab.selection = None;
            self.open_at(&file, line);
        }
    }

    /// What the build said: errors first, each a click from its line; then
    /// warnings and bad boxes; and the packages it lacked.
    fn log_view(&mut self, ui: &mut Ui, p: &Palette) {
        use quark_tex::log::Kind;
        let Some(log) = self.builds.log.clone() else {
            ui.label(RichText::new("Nothing typeset yet.").font(theme::body(12.5)).color(p.text_faint));
            return;
        };
        let mut go = None;
        egui::ScrollArea::vertical().id_salt("tex-log").auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            if !log.missing.is_empty() {
                ui.label(
                    RichText::new(format!("TeX lacks {}: a package would bring them.", log.missing.join(", ")))
                        .font(theme::body(12.0))
                        .color(p.warning),
                );
                ui.add_space(6.0);
            }
            if log.entries.is_empty() {
                ui.label(RichText::new("No errors or warnings.").font(theme::body(12.5)).color(p.text_faint));
            }
            for kind in [Kind::Error, Kind::Warning, Kind::Box] {
                for e in log.entries.iter().filter(|e| e.kind == kind) {
                    let colour = match kind {
                        Kind::Error => code::severity_colour(quark_lsp::servers::Severity::Error, p),
                        Kind::Warning => code::severity_colour(quark_lsp::servers::Severity::Warning, p),
                        Kind::Box => p.text_faint,
                    };
                    // The project's files as it names them; TeX's own, from
                    // deep in TinyTeX, by name alone.
                    let shown = e.file.as_deref().map(|f| {
                        if Path::new(f).is_absolute() {
                            Path::new(f).file_name().map_or_else(|| f.to_string(), |n| n.to_string_lossy().into_owned())
                        } else {
                            f.to_string()
                        }
                    });
                    let place = match (&shown, e.line) {
                        (Some(f), Some(l)) => format!("{f}:{l}"),
                        (Some(f), None) => f.clone(),
                        _ => String::new(),
                    };
                    let response = egui::Frame::new()
                        .fill(p.wash(0x06))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 3.5, colour);
                                ui.add(egui::Label::new(RichText::new(&place).font(code::font()).size(11.5).color(p.text_muted)).truncate())
                                    .on_hover_text(&place);
                            });
                            ui.add(egui::Label::new(RichText::new(&e.message).font(theme::body(12.5)).color(p.text)).wrap());
                            if let Some(context) = &e.context {
                                ui.add(egui::Label::new(RichText::new(context).font(code::font()).size(11.5).color(p.text_faint)).truncate());
                            }
                        })
                        .response
                        .interact(Sense::click());
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        if let (Some(f), Some(l)) = (&e.file, e.line) {
                            go = Some((f.clone(), l));
                        }
                    }
                    ui.add_space(4.0);
                }
            }
        });
        if let Some((file, line)) = go {
            self.open_at(&file, line);
        }
    }

    /// Each split, side by side or one above another.
    fn splits(&mut self, ui: &mut Ui, p: &Palette, area: Rect) {
        let count = self.session.splits().len().max(1);
        let vertical = self.session.splits().is_vertical();
        let focused = self.session.splits().focused();
        let tab = self.session.tab_size;
        let mut pointer = Pointer::None;
        self.lefts.resize(count, 0);
        for view in 0..count {
            let rect = split_rect(area, view, count, vertical);
            // Each file's highlighter, made the first time it is drawn.
            let path = self
                .session
                .view_of(view)
                .and_then(|(b, ..)| self.session.editor().buffers().get(b))
                .and_then(|b| b.path().map(Path::to_path_buf));
            let syntax = match &path {
                Some(path) => self.syntaxes.entry(path.clone()).or_insert_with(|| Syntax::for_path(path)).as_mut(),
                None => None,
            };
            let char_w = ui.fonts_mut(|f| f.glyph_width(&code::font(), 'M'));
            let m = code::Metrics::new(char_w, 1);
            let lines = m.lines_in(rect.height());
            if self.session.splits().at(view).map(|v| v.lines) != Some(lines) {
                self.session.set_lines(view, lines);
            }
            // What the server says of the file, against it as it is now.
            let diagnostics: Vec<servers::Diagnostic> = match (&path, self.session.view_of(view)) {
                (Some(path), Some((b, ..))) => match self.session.editor().buffers().get(b) {
                    Some(buffer) => self.servers.diagnostics_for(path).iter().map(|d| servers::diagnostic(buffer, d)).collect(),
                    None => Vec::new(),
                },
                _ => Vec::new(),
            };
            let draw = ViewDraw {
                view,
                focused: view == focused,
                active: true,
                syntax,
                tab,
                left: &mut self.lefts[view],
                diagnostics: &diagnostics,
            };
            let (got, cursor) = code::show(ui, p, &self.session, draw, rect);
            // The agent's changes in this file, over its lines.
            if let (Some(review), Some(path), Some((_, _, first))) = (&self.agent.review, &path, self.session.view_of(view)) {
                let marks = review.marks_for(path);
                if !marks.is_empty() {
                    let text = Rect::from_min_max(pos2(rect.left() + m.gutter_w, rect.top()), rect.max);
                    crate::texagent::paint_marks(&ui.painter_at(rect), text, first, m.line_h, &marks, p);
                }
            }
            if view == focused {
                self.cursor_at = cursor;
            }
            if got != Pointer::None {
                pointer = got;
            }
            if count > 1 && view == focused {
                ui.painter().rect_stroke(rect, CornerRadius::ZERO, egui::Stroke::new(1.0, p.accent_alpha(0x60)), egui::StrokeKind::Inside);
            }
        }
        self.completion_list(ui, p);
        if pointer != Pointer::None {
            self.popup.close();
        }
        // A click in the source takes the keys from the page beside it.
        if matches!(pointer, Pointer::Click { .. }) {
            ui.memory_mut(|m| m.stop_text_input());
        }
        match pointer {
            Pointer::Click { view, line, column } => self.session.click(view, line, column),
            Pointer::Drag { line, column } => self.session.select_to(line, column),
            Pointer::Scroll { view, lines } => self.session.scroll(view, lines),
            Pointer::None => {}
        }
    }

    /// The completions, under the cursor: the chosen one lit, its hint dim.
    fn completion_list(&mut self, ui: &mut Ui, p: &Palette) {
        let (true, Some(at)) = (self.popup.is_open(), self.cursor_at) else { return };
        const ROWS: usize = 9;
        let items = self.popup.items();
        let chosen = self.popup.selected();
        // The chosen one kept in view.
        let first = chosen.saturating_sub(ROWS - 1);
        let mut clicked = None;
        egui::Area::new(ui.id().with("tex-completions"))
            .order(egui::Order::Foreground)
            .fixed_pos(at.left_bottom() + vec2(-4.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).inner_margin(4).show(ui, |ui| {
                    ui.set_width(340.0);
                    for (i, item) in items.iter().enumerate().skip(first).take(ROWS) {
                        let (rect, response) = ui.allocate_exact_size(vec2(340.0, 22.0), Sense::click());
                        if i == chosen {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), p.accent_soft);
                        }
                        ui.painter().text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, &item.label, code::font(), if i == chosen { p.accent_text } else { p.text });
                        if let Some(hint) = &item.hint {
                            let hint = quark_ui::widgets::elide(hint, 26, false);
                            ui.painter().text(rect.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, hint, theme::body(11.0), p.text_faint);
                        }
                        if response.clicked() {
                            clicked = Some(i);
                        }
                    }
                    if items.len() > ROWS {
                        ui.label(RichText::new(format!("{} of {}", chosen + 1, items.len())).font(theme::body(10.5)).color(p.text_faint));
                    }
                });
            });
        if let Some(i) = clicked {
            self.popup.select(i);
            if let Some(item) = self.popup.chosen().cloned() {
                self.popup.close();
                servers::apply_completion(self.session.editor_mut().buffer_mut(), &item);
            }
        }
    }

    /// The mode, the file, what was said, and where the cursor is — or, at
    /// the `:` and `/`, the line being typed.
    fn status_line(&self, ui: &mut Ui, p: &Palette, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, p.wash(0x06));
        let mid = rect.center().y;
        let vim = self.session.vim();
        if self.session.mode() == VimMode::Command {
            let line = format!("{}{}", vim.prompt(), vim.cmdline());
            let galley = painter.layout_no_wrap(line, code::font(), p.text);
            let x = rect.left() + 12.0;
            let w = galley.size().x;
            painter.galley(pos2(x, mid - galley.size().y / 2.0), galley, p.text);
            painter.rect_filled(Rect::from_min_size(pos2(x + w + 1.0, mid - 8.0), vec2(2.0, 16.0)), CornerRadius::ZERO, p.accent);
            return;
        }
        let mode = self.session.mode();
        let (label, fill) = match mode {
            _ if self.visual.pane == Pane::Visual || (self.visual.pane == Pane::Both && self.visual.focused) => ("DOCUMENT", p.accent_alpha(0x90)),
            VimMode::Insert => ("INSERT", p.accent),
            VimMode::Replace => ("REPLACE", p.warning),
            VimMode::Visual => ("VISUAL", p.accent_alpha(0xb0)),
            VimMode::VisualLine => ("V-LINE", p.accent_alpha(0xb0)),
            VimMode::VisualBlock => ("V-BLOCK", p.accent_alpha(0xb0)),
            _ => ("NORMAL", p.wash(0x18)),
        };
        let pill = painter.layout_no_wrap(label.into(), theme::strong(10.5), p.text);
        let pill_rect = Rect::from_min_size(pos2(rect.left() + 8.0, mid - 9.0), vec2(pill.size().x + 14.0, 18.0));
        painter.rect_filled(pill_rect, CornerRadius::same(5), fill);
        painter.galley(pos2(pill_rect.left() + 7.0, mid - pill.size().y / 2.0), pill, p.text);
        let buffer = self.session.editor().buffer();
        let name = match buffer.path() {
            Some(path) => quark_modal::root::Root::new(&self.root).shown(path),
            None => buffer.name(),
        };
        let file = format!("{name}{}", if buffer.is_modified() { "  •" } else { "" });
        // Clear of the pane switch, which sits 250 points in from the right.
        let x = pill_rect.right() + 10.0;
        let galley = one_line(ui, &file, theme::body(12.0), p.text_muted, rect.right() - 262.0 - x);
        let name_right = x + galley.size().x;
        painter.galley(pos2(x, mid - galley.size().y / 2.0), galley, p.text_muted);
        // What was said last, or else the server's word on this line.
        if let Some((text, warn)) = self.said.clone().or_else(|| self.diagnostic_here()) {
            // Between the file's name and the switch, when there is room.
            let room = rect.right() - 262.0 - name_right - 24.0;
            if room > 60.0 {
                let colour = if warn { p.warning } else { p.text_faint };
                let galley = one_line(ui, &text, theme::body(12.0), colour, room);
                let x = (name_right + 12.0).max(rect.center().x - galley.size().x / 2.0).min(rect.right() - 262.0 - 12.0 - galley.size().x);
                painter.galley(pos2(x, mid - galley.size().y / 2.0), galley, colour);
            }
        }
        let at = buffer.position(buffer.cursor.offset);
        let place = format!("{}:{}", at.line + 1, at.column + 1);
        painter.text(pos2(rect.right() - 12.0, mid), egui::Align2::RIGHT_CENTER, place, code::font(), p.text_faint);
    }
}

/// Split `view` of `count`, side by side when `vertical`, a pixel between.
pub(crate) fn split_rect(area: Rect, view: usize, count: usize, vertical: bool) -> Rect {
    let count = count.max(1) as f32;
    let i = view as f32;
    if vertical {
        let w = area.width() / count;
        Rect::from_min_max(pos2(area.left() + w * i, area.top()), pos2(area.left() + w * (i + 1.0) - 1.0, area.bottom()))
    } else {
        let h = area.height() / count;
        Rect::from_min_max(pos2(area.left(), area.top() + h * i), pos2(area.right(), area.top() + h * (i + 1.0) - 1.0))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn project(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("quark-tex-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["chapters", ".git", "build", "figures"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("refs.bib"), "@book{k,}\n").unwrap();
        std::fs::write(root.join("notes.tex"), "% notes\n").unwrap();
        std::fs::write(root.join("paper.tex"), "\\documentclass{article}\n\\begin{document}\nHi\n\\end{document}\n").unwrap();
        std::fs::write(root.join("chapters/one.tex"), "\\section{One}\n").unwrap();
        std::fs::write(root.join("build/paper.aux"), "").unwrap();
        std::fs::write(root.join(".git/HEAD"), "").unwrap();
        root
    }

    #[test]
    fn a_projects_files_are_listed_folders_first_without_what_is_hidden_or_built() {
        let root = project("walk");
        let rows: Vec<(usize, String)> = walk(&root).into_iter().map(|r| (r.depth, r.name)).collect();
        assert_eq!(
            rows,
            [
                (0, "chapters".to_string()),
                (1, "one.tex".to_string()),
                (0, "figures".to_string()),
                (0, "notes.tex".to_string()),
                (0, "paper.tex".to_string()),
                (0, "refs.bib".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_project_opens_at_main_tex_or_the_file_that_begins_the_document() {
        let root = project("main");
        let files = walk(&root);
        assert_eq!(main_file(&files), Some(root.join("paper.tex")), "notes.tex comes first but begins nothing");
        std::fs::write(root.join("main.tex"), "\\input{paper}\n").unwrap();
        assert_eq!(main_file(&walk(&root)), Some(root.join("main.tex")));
        let opened = TexProject::open(&root);
        assert_eq!(opened.session.editor().buffer().name(), "main.tex");
        assert_eq!(opened.name(), root.file_name().unwrap().to_string_lossy());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn keys_reach_vim_as_it_names_them_and_command_chords_do_not() {
        let key = |key: egui::Key, modifiers: egui::Modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        assert_eq!(key_of(&egui::Event::Text("aB".into())).iter().map(|k| (k.key.clone(), k.shift)).collect::<Vec<_>>(), [("a".into(), false), ("B".into(), true)]);
        assert_eq!(key_of(&key(egui::Key::Escape, egui::Modifiers::NONE))[0].key, "Escape");
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
        let d = &key_of(&key(egui::Key::D, ctrl))[0];
        assert_eq!((d.key.as_str(), d.ctrl), ("d", true), "⌃d is Vim's");
        assert_eq!(key_of(&key(egui::Key::OpenBracket, ctrl))[0].key, "Escape", "⌃[ is Escape");
        let cmd = egui::Modifiers { mac_cmd: true, command: true, ..Default::default() };
        assert!(key_of(&key(egui::Key::S, cmd)).is_empty(), "⌘S is the application's");
        assert!(key_of(&key(egui::Key::A, egui::Modifiers::NONE)).is_empty(), "a letter comes as text, not twice");
        let shifted = &key_of(&key(egui::Key::Tab, egui::Modifiers::SHIFT))[0];
        assert!(shifted.shift && shifted.key == "Tab");
    }

    #[test]
    fn splits_share_the_area_side_by_side_or_stacked() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0));
        assert_eq!(split_rect(area, 0, 1, true), Rect::from_min_max(pos2(0.0, 0.0), pos2(899.0, 600.0)));
        assert_eq!(split_rect(area, 1, 3, true), Rect::from_min_max(pos2(300.0, 0.0), pos2(599.0, 600.0)));
        assert_eq!(split_rect(area, 1, 2, false), Rect::from_min_max(pos2(0.0, 300.0), pos2(900.0, 599.0)));
    }

    #[test]
    fn the_list_takes_the_keys_that_mean_something_to_it() {
        assert_eq!(list_key(&Key::named("Tab")), ListKey::Move(1));
        assert_eq!(list_key(&Key::ctrl('p')), ListKey::Move(-1));
        assert_eq!(list_key(&Key::named("Enter")), ListKey::Accept);
        assert_eq!(list_key(&Key::named("Escape")), ListKey::Close);
        assert_eq!(list_key(&Key::named("Backspace")), ListKey::CloseAndPass);
        assert_eq!(list_key(&Key::char('n')), ListKey::Pass, "a bare n is typing");
        assert!(asks_again('\\', true, false), "a trigger asks");
        assert!(asks_again('s', false, true), "a letter narrows the list showing");
        assert!(!asks_again('s', false, false), "a letter alone asks nothing");
        assert!(!asks_again(' ', false, true), "a space ends the word, and the list");
    }

    #[test]
    fn a_diagnostic_marks_what_it_names_on_each_line_it_spans() {
        let d = servers::Diagnostic {
            line: 1,
            column: 4,
            end_line: 3,
            end_column: 2,
            severity: servers::Severity::Error,
            message: String::new(),
            source: None,
            code: None,
        };
        assert_eq!(code::marked_on(&d, 0, 10), None);
        assert_eq!(code::marked_on(&d, 1, 10), Some((4, 10)));
        assert_eq!(code::marked_on(&d, 2, 6), Some((0, 6)));
        assert_eq!(code::marked_on(&d, 3, 9), Some((0, 2)));
        let point = servers::Diagnostic { line: 0, column: 3, end_line: 0, end_column: 3, ..d.clone() };
        assert_eq!(code::marked_on(&point, 0, 9), Some((3, 4)), "at least a character");
        assert_eq!(code::marked_on(&point, 0, 2), Some((2, 3)), "on a short line, at its end");
    }

    /// texlab, standing in: answers the handshake with `\` and `{` as
    /// triggers, says the first line of a file opened has an error, and
    /// offers `\section` for a completion.
    pub(crate) const STAND_IN: &str = r#"#!/usr/bin/perl
use strict; $| = 1;
sub send_json { my $b = shift; print "Content-Length: " . length($b) . "\r\n\r\n" . $b; }
while (1) {
  my $len = 0;
  while (my $line = <STDIN>) { last if $line eq "\r\n"; $len = $1 if $line =~ /Content-Length: (\d+)/i; }
  last unless $len;
  read(STDIN, my $body, $len);
  my ($id) = $body =~ /"id":(\d+)/;
  if ($body =~ /"method":"initialize"/) {
    send_json(qq({"jsonrpc":"2.0","id":$id,"result":{"capabilities":{"completionProvider":{"triggerCharacters":["\\\\","{"]}},"serverInfo":{"name":"stand-in"}}}));
  } elsif ($body =~ /"method":"textDocument\/didOpen"/) {
    my ($uri) = $body =~ /"uri":"([^"]+)"/;
    send_json(qq({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"$uri","diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}},"severity":1,"message":"Undefined control sequence."}]}}));
  } elsif ($body =~ /"method":"textDocument\/completion"/) {
    send_json(qq({"jsonrpc":"2.0","id":$id,"result":{"isIncomplete":false,"items":[{"label":"\\\\section","insertText":"section","kind":3,"detail":"a heading"}]}}));
  } elsif ($body =~ /"method":"shutdown"/) {
    send_json(qq({"jsonrpc":"2.0","id":$id,"result":null}));
  } elsif ($body =~ /"method":"exit"/) { last; }
}
"#;

    pub(crate) fn settle(project: &mut TexProject, done: impl Fn(&TexProject) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done(project) {
            assert!(std::time::Instant::now() < deadline, "the server never answered");
            project.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn texlab_is_told_the_file_says_what_is_wrong_and_completes_a_command() {
        use std::os::unix::fs::PermissionsExt;
        let root = project("lsp");
        std::fs::write(root.join("main.tex"), "\\badcommand\nText \n").unwrap();
        let server = root.join("stand-in-texlab");
        std::fs::write(&server, STAND_IN).unwrap();
        std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut project = TexProject::open(&root);
        project.set_texlab(server);
        // Told of the file it opened at, and its first line marked.
        settle(&mut project, |p| p.diagnostic_here().is_some());
        let (said, error) = project.diagnostic_here().unwrap();
        assert_eq!((said.as_str(), error), ("Undefined control sequence.", true));

        // `\` at the end of the second line, in insert mode: the list comes,
        // and Enter takes its first.
        for key in [Key::char('j'), Key::char('A')] {
            project.key(&key);
        }
        project.key(&Key::char('\\'));
        settle(&mut project, |p| p.popup.is_open());
        assert_eq!(project.popup.items()[0].label, "\\section");
        project.key(&Key::named("Enter"));
        assert!(!project.popup.is_open());
        let text = project.session.editor().buffer().contents();
        assert_eq!(text, "\\badcommand\nText \\section\n", "the command completed where it was typed");
        assert_eq!(project.session.mode(), VimMode::Insert, "and still typing");
        drop(project);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_save_of_a_source_asks_for_a_build_and_reads_the_outline_again() {
        let root = project("save");
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\n\\begin{document}\n\\section{One}\nWords here.\n\\end{document}\n").unwrap();
        let mut p = TexProject::open(&root);
        assert_eq!(p.outline.1.headings.len(), 1);
        assert!(!p.wants_build);
        for c in "4Go\\section{Two}".chars() {
            p.key(&Key::char(c));
        }
        p.key(&Key::named("Escape"));
        for c in ":w".chars() {
            p.key(&Key::char(c));
        }
        let said = p.key(&Key::named("Enter"));
        p.tell(said);
        assert!(p.wants_build, "a .tex written typesets the project");
        let titles: Vec<&str> = p.outline.1.headings.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two"], "and its outline is read again");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_place_in_the_log_or_the_outline_opens_its_file_at_its_line() {
        let root = project("open-at");
        let mut p = TexProject::open(&root);
        p.open_at("chapters/one.tex", 1);
        assert_eq!(p.session.editor().buffer().name(), "one.tex");
        p.open_at("paper.tex", 3);
        let b = p.session.editor().buffer();
        assert_eq!((b.name().as_str(), b.position(b.cursor.offset).line), ("paper.tex", 2));
        p.open_at("gone.tex", 1);
        assert!(p.said.as_ref().is_some_and(|(_, warn)| *warn), "a file not there is said");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_synctex_box_is_turned_the_right_way_up() {
        let r = quark_tex::synctex::Rect { x: 72.0, y: 100.0, w: 200.0, h: 12.0 };
        let page = synctex_rect(&r, 792.0);
        assert_eq!((page.min.x, page.min.y, page.max.x, page.max.y), (72.0, 680.0, 272.0, 692.0));
    }

    #[test]
    fn a_project_made_from_a_template_opens_at_its_main_file() {
        let parent = std::env::temp_dir().join(format!("quark-tex-{}-templates", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        for t in quark_tex::templates::templates() {
            let made = quark_tex::templates::create(&t.id, &parent, &format!("My {}", t.name)).unwrap();
            let p = TexProject::open(&made.folder);
            assert_eq!(p.session.editor().buffer().name(), "main.tex", "{}", t.id);
            assert!(p.session.editor().buffer().contents().contains("\\documentclass"), "{}", t.id);
        }
        assert!(quark_tex::templates::create("article", &parent, "My Article").is_err(), "a folder already made is not written over");
        assert!(crate::dialogs::new_tex_name_ok("My Paper_2-final"));
        assert!(!crate::dialogs::new_tex_name_ok("../escape") && !crate::dialogs::new_tex_name_ok(".hidden") && !crate::dialogs::new_tex_name_ok("  "));
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn a_paper_from_the_bib_is_cited_at_the_cursor() {
        let root = project("cite");
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\n\\begin{document}\nAs shown \n\\bibliography{refs}\n\\end{document}\n").unwrap();
        std::fs::write(root.join("refs.bib"), "@book{knuth1984,\n  title = {The TeXbook},\n  author = {Donald E. Knuth},\n  year = {1984}\n}\n").unwrap();
        let mut p = TexProject::open(&root);
        for c in "3G$".chars() {
            p.key(&Key::char(c));
        }
        p.key(&Key::char('a'));
        p.open_picker();
        let picker = p.picker.as_mut().unwrap();
        picker.field = "knuth".into();
        let main = root.join("main.tex");
        picker.cite.search(Some(&main), "knuth");
        assert_eq!(picker.all().len(), 1);
        p.cite_chosen();
        assert!(p.picker.is_none(), "closed once cited");
        assert!(p.session.editor().buffer().contents().contains("As shown \\cite{knuth1984}\n"), "{}", p.session.editor().buffer().contents());
        // Inside a \cite{ already open, the key alone.
        for c in " \\cite{".chars() {
            p.key(&Key::char(c));
        }
        p.open_picker();
        p.cite_chosen();
        assert!(p.session.editor().buffer().contents().contains("\\cite{knuth1984} \\cite{knuth1984}"), "{}", p.session.editor().buffer().contents());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_source_and_the_page_show_side_by_side_and_command_e_steps_through_the_panes() {
        let root = project("panes");
        let mut tex = TexProject::open(&root);
        assert_eq!(tex.visual.pane, Pane::Both, "side by side unless asked");
        let ctx = egui::Context::default();
        let press = |tex: &mut TexProject, events: Vec<egui::Event>| {
            let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| tex.input(ui.ctx()));
            out.textures_delta.clear();
        };
        let key = |key: egui::Key, modifiers: egui::Modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        // Beside the page, keys are Vim's.
        press(&mut tex, vec![egui::Event::Text("O".into()), egui::Event::Text("X".into()), key(egui::Key::Escape, egui::Modifiers::NONE)]);
        assert!(tex.session.editor().buffer().contents().starts_with("X\n"), "{}", tex.session.editor().buffer().contents());
        press(&mut tex, vec![key(egui::Key::E, egui::Modifiers::COMMAND)]);
        assert_eq!(tex.visual.pane, Pane::Visual);
        // The page alone: Vim hears nothing.
        press(&mut tex, vec![egui::Event::Text("d".into()), egui::Event::Text("d".into())]);
        assert!(tex.session.editor().buffer().contents().starts_with("X\n"));
        press(&mut tex, vec![key(egui::Key::E, egui::Modifiers::COMMAND)]);
        assert_eq!(tex.visual.pane, Pane::Source);
        press(&mut tex, vec![key(egui::Key::E, egui::Modifiers::COMMAND)]);
        assert_eq!(tex.visual.pane, Pane::Both);
        // Leaving for the page alone ends typing in the source.
        press(&mut tex, vec![egui::Event::Text("i".into())]);
        tex.set_pane(Pane::Visual, None);
        assert_eq!(tex.session.mode(), VimMode::Normal);
        let _ = std::fs::remove_dir_all(root);
    }
}
