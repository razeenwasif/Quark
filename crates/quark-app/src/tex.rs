//! The LaTeX workspace: a project's files on the left, its source in a Vim
//! editor beside them, a status line under it. Builds and the PDF come with
//! L3; this is the editor (L1).
//!
//! The editor is `quark_modal`'s session; this gives it keys, draws its
//! splits with `code`, and keeps a highlighter for each file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{CornerRadius, Rect, RichText, Sense, Ui, pos2, vec2};
use quark_modal::key::Key;
use quark_modal::session::{Said, Session};
use quark_modal::vim::VimMode;
use quark_syntax::Syntax;
use quark_ui::theme::{self, Palette};

use crate::code::{self, Pointer, ViewDraw};

/// The most files a project's list shows.
const MOST_FILES: usize = 2_000;
/// Folders that are build output or tools', not the project's.
const PASSED: [&str; 6] = ["node_modules", "target", "build", "_minted", "__pycache__", "out"];
/// The file list's width.
const FILES_W: f32 = 230.0;
/// The dock's width.
const DOCK_W: f32 = 320.0;
/// The status line's height.
const STATUS_H: f32 = 28.0;

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
        TexProject { root, session, files, syntaxes: HashMap::new(), said, lefts: Vec::new() }
    }

    pub(crate) fn name(&self) -> String {
        self.root.file_name().map_or_else(|| self.root.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    /// The files with changes not written.
    pub(crate) fn unsaved(&self) -> Vec<String> {
        self.session.unsaved()
    }

    /// ⌘S: the file in focus written.
    pub(crate) fn save(&mut self) {
        let said = self.session.save();
        self.tell(said);
    }

    /// Keys for the editor, as they came this frame; what it said kept for
    /// the status line, what it yanked put on the clipboard.
    pub(crate) fn input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let mut said = Vec::new();
        for event in &events {
            match event {
                egui::Event::Paste(text) => self.session.paste(text),
                // ⌘C and ⌘X with a selection: yanked and deleted, so the
                // clipboard gets it as `y` and `d` give it.
                egui::Event::Copy if self.session.vim().is_selecting() => said.extend(self.session.key(&Key::char('y'))),
                egui::Event::Cut if self.session.vim().is_selecting() => said.extend(self.session.key(&Key::char('d'))),
                _ => {
                    for key in key_of(event) {
                        said.extend(self.session.key(&key));
                    }
                }
            }
        }
        if let Some(text) = self.session.take_clipboard() {
            ctx.copy_text(text);
        }
        self.tell(said);
    }

    fn tell(&mut self, said: Vec<Said>) {
        for s in said {
            self.said = match s {
                Said::Info(text) => Some((text, false)),
                Said::Warn(text) => Some((text, true)),
                Said::Wrote(path) => {
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
    ) -> Option<crate::texdock::EnvAction> {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let files = Rect::from_min_size(full.min, vec2(FILES_W, full.height()));
        let side = Rect::from_min_max(pos2(full.right() - DOCK_W, full.top()), full.max);
        let editor = Rect::from_min_max(pos2(files.right() + theme::GAP, full.top()), pos2(side.left() - theme::GAP, full.bottom()));
        let painter = ui.ctx().layer_painter(ui.layer_id());
        theme::paint_island(&painter, files, p);
        theme::paint_island(&painter, editor, p);
        theme::paint_island(&painter, side, p);
        let mut asked = None;
        if let Some(state) = env {
            let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink2(vec2(16.0, 14.0))));
            egui::ScrollArea::vertical().id_salt("tex-dock").auto_shrink([false, false]).show(&mut side_ui, |ui| {
                asked = crate::texdock::show(ui, p, state, dock);
            });
        }

        let mut files_ui = ui.new_child(egui::UiBuilder::new().max_rect(files.shrink2(vec2(10.0, 12.0))));
        self.file_list(&mut files_ui, p);

        let inner = editor.shrink(1.0);
        let status = Rect::from_min_max(pos2(inner.left(), inner.bottom() - STATUS_H), inner.max);
        let panes = Rect::from_min_max(inner.min, pos2(inner.right(), status.top()));
        self.splits(ui, p, panes);
        self.status_line(ui, p, status);
        asked
    }

    fn file_list(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.name()).font(theme::strong(13.0)).color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("↻").on_hover_text("Look at the folder again").clicked() {
                    self.files = walk(&self.root);
                }
            });
        });
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
                ui.painter().text(pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, text, theme::body(12.5), colour);
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
            let draw = ViewDraw { view, focused: view == focused, active: true, syntax, tab, left: &mut self.lefts[view] };
            let got = code::show(ui, p, &self.session, draw, rect);
            if got != Pointer::None {
                pointer = got;
            }
            if count > 1 && view == focused {
                ui.painter().rect_stroke(rect, CornerRadius::ZERO, egui::Stroke::new(1.0, p.accent_alpha(0x60)), egui::StrokeKind::Inside);
            }
        }
        match pointer {
            Pointer::Click { view, line, column } => self.session.click(view, line, column),
            Pointer::Drag { line, column } => self.session.select_to(line, column),
            Pointer::Scroll { view, lines } => self.session.scroll(view, lines),
            Pointer::None => {}
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
        painter.text(pos2(pill_rect.right() + 10.0, mid), egui::Align2::LEFT_CENTER, file, theme::body(12.0), p.text_muted);
        if let Some((text, warn)) = &self.said {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::body(12.0), if *warn { p.warning } else { p.text_faint });
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
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
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
}
