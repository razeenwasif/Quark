//! The application: window chrome, command dispatch, and the service loop.

use std::path::{Path, PathBuf};

use egui::Context;
use quark_core::annot::Annotation;
use quark_core::geom::Rot;
use quark_core::prefs::{Prefs, SidePanel};
use quark_core::tools::Tool;
use quark_pdf::service::{DocOp, PdfService, Request, Response};
use quark_ui::theme::{self, Palette, ThemeMode};

use crate::dialogs::Dialog;
use crate::shortcuts::{self, Binding};
use crate::tab::Tab;
use crate::textures::TextureCache;

/// A transient message, floated over the bottom-right of the work area.
pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) error: bool,
    /// Seconds remaining.
    pub(crate) ttl: f32,
}

/// What the right-hand dock is showing.
///
/// Comments and the assistant share a column because they are the same kind of
/// thing — a conversation about the document, as opposed to the structural
/// panels (thumbnails, outline, fields) which live in the left nav.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Dock {
    #[default]
    Comments,
    Assistant,
}

pub struct App {
    pub(crate) service: PdfService,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) prefs: Prefs,
    pub(crate) theme: ThemeMode,
    pub(crate) textures: TextureCache,
    pub(crate) tool: Tool,
    pub(crate) bindings: Vec<Binding>,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) toasts: Vec<Toast>,
    /// Set when the user asked to quit but a document has unsaved changes.
    pub(crate) quit_confirmed: bool,
    pub(crate) show_palette: bool,
    pub(crate) palette_query: String,
    /// Search token, so results from an abandoned search are ignored.
    pub(crate) search_token: u64,
    /// Files waiting to be opened once the engine reports ready.
    pub(crate) startup_files: Vec<PathBuf>,
    /// Files macOS asks Quark to open, as they come; see
    /// `quark_shell::open_requests`.
    pub(crate) opened: crossbeam_channel::Receiver<Vec<PathBuf>>,
    /// Which panel the right-hand dock shows, and whether it is open at all.
    pub(crate) dock: Dock,
    pub(crate) dock_open: bool,
    /// The assistant: conversation state, the worker, and the composer.
    pub(crate) ai: crate::assistant::Assistant,
    /// The home screen. Always first in the tab strip, whether or not it is
    /// the surface currently showing.
    pub(crate) home: crate::home::HomeState,
    /// The books in the person's folders, walked on a worker.
    pub(crate) library: quark_pdf::library::Library,
    /// The LaTeX project open, and whether it is the surface showing.
    pub(crate) tex: Option<crate::tex::TexProject>,
    pub(crate) tex_visible: bool,
    /// Quark's own TeX, in `~/.quark/tex`: looked at when a project is
    /// first opened, and made only with the person's say-so.
    pub(crate) tex_env: Option<quark_lab::tex::TexEnv>,
    pub(crate) tex_dock: crate::texdock::Dock,
    /// The LaTeX project's PDF, shown in its dock by the viewer: a document
    /// of its own, apart from the tabs, and the one being opened in its
    /// place when a build makes another.
    pub(crate) tex_pdf: Option<Tab>,
    pub(crate) tex_pdf_opening: Option<(quark_pdf::service::DocId, u64)>,
    /// Pages being read with OCR, behind the window.
    pub(crate) ocr: Option<crate::ocr::OcrRun>,
    /// A document as it was when OCR was asked for, while its pages and
    /// languages are chosen: its bytes and which layout of its pages.
    pub(crate) ocr_prep: Option<(quark_pdf::service::DocId, std::sync::Arc<Vec<u8>>, u64)>,
    /// The side panel to bring back when the pages island is reopened.
    pub(crate) last_side_panel: SidePanel,
    /// The highlighted row in the command palette.
    pub(crate) palette_index: usize,
    /// Whether the palette was showing last frame.
    pub(crate) palette_was_open: bool,
    /// The velvet grain tile, and which theme it was built for.
    pub(crate) grain: Option<(bool, egui::TextureHandle)>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<PathBuf>,
        opened: crossbeam_channel::Receiver<Vec<PathBuf>>,
    ) -> Self {
        let prefs = load_prefs();
        let theme = if prefs.theme_dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, theme);
        // A zoom on top of the display's own scale, not a replacement for it:
        // setting pixels-per-point directly to 1.0 drew the whole interface at
        // half size on a Retina display.
        cc.egui_ctx.set_zoom_factor(prefs.ui_scale.clamp(0.5, 3.0));

        // A file sent by macOS reaches the channel while Quark may be idle,
        // drawing nothing until something happens. Passed on through a thread
        // that asks for a frame, so it is opened at once.
        let opened = {
            let (tx, rx) = crossbeam_channel::unbounded();
            let ctx = cc.egui_ctx.clone();
            let _ = std::thread::Builder::new().name("open-requests".into()).spawn(move || {
                for paths in opened.iter() {
                    if tx.send(paths).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
            rx
        };

        let service = PdfService::start();
        let textures = TextureCache::new(prefs.raster_cache_mb);

        let mut app = Self {
            service,
            tabs: Vec::new(),
            active: 0,
            tool: Tool::Select,
            bindings: shortcuts::bindings(),
            textures,
            theme,
            prefs,
            dialog: None,
            toasts: Vec::new(),
            quit_confirmed: false,
            show_palette: false,
            palette_query: String::new(),
            search_token: 0,
            startup_files: files,
            opened,
            ai: crate::assistant::Assistant::new(),
            dock: Dock::default(),
            dock_open: true,
            home: crate::home::HomeState::default(),
            library: library_kept(),
            ocr: None,
            ocr_prep: None,
            tex: None,
            tex_visible: false,
            tex_env: None,
            tex_dock: crate::texdock::Dock::default(),
            tex_pdf: None,
            tex_pdf_opening: None,
            last_side_panel: SidePanel::Thumbnails,
            palette_index: 0,
            palette_was_open: false,
            grain: None,
        };

        // On screen at once as it was last walked, and walked again behind.
        app.library.set_folders(app.prefs.library_folders.clone());
        app.library.walk();
        app.sync_books();

        let pending = std::mem::take(&mut app.startup_files);
        for f in pending {
            app.open_path(&f, None);
        }
        app
    }

    /// The home screen's library, as the library now is.
    pub(crate) fn sync_books(&mut self) {
        let shelf = &mut self.home.books;
        shelf.books = self.library.books();
        shelf.folders = self.library.folders().to_vec();
        shelf.missing = self.library.missing();
        shelf.walking = self.library.walking();
    }

    /// Read pages with OCR: what this document is now, then its pages and
    /// languages chosen.
    pub(crate) fn recognise_text(&mut self) {
        if self.ocr.is_some() {
            self.toast("Pages are being read already", false);
            return;
        }
        if quark_lab::ocr::Tesseract::find().is_none() {
            self.dialog = Some(Dialog::Message {
                title: "Recognise Text".into(),
                body: "OCR reads pages with tesseract, which is not on this Mac. It is installed with Homebrew: brew install tesseract".into(),
                error: false,
            });
            return;
        }
        if let Some(t) = self.tab() {
            self.service.send(Request::OcrPrep { id: t.id });
        }
    }

    /// The pages and languages chosen: the reading started.
    pub(crate) fn start_ocr(&mut self, pages: Vec<usize>, langs: Vec<String>) {
        let Some((doc, bytes, layout)) = self.ocr_prep.take() else { return };
        let Some(tesseract) = quark_lab::ocr::Tesseract::find() else { return };
        if pages.is_empty() {
            return;
        }
        self.prefs.ocr_langs = langs.clone();
        save_prefs(&self.prefs);
        self.ocr = Some(crate::ocr::OcrRun::start(doc, layout, bytes, pages, langs, tesseract));
    }

    /// How the reading is getting on: once a frame. What it read is laid
    /// over its pages as one change, if its document is still open.
    pub(crate) fn tend_ocr(&mut self, ctx: &Context) {
        let Some(run) = self.ocr.as_mut() else { return };
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        let Some(end) = run.poll() else { return };
        let (doc, layout) = (run.doc, run.layout);
        self.ocr = None;
        match end {
            crate::ocr::OcrNews::Done(pages) => {
                if self.tabs.iter().any(|t| t.id == doc) {
                    let n = pages.len();
                    self.service.send(Request::Apply { id: doc, op: Box::new(DocOp::Ocr { layout, pages }) });
                    self.toast(
                        format!("Read {n} page{} — their words can be found and selected", if n == 1 { "" } else { "s" }),
                        false,
                    );
                }
            }
            crate::ocr::OcrNews::Stopped => self.toast("OCR stopped — nothing was changed", false),
            crate::ocr::OcrNews::Failed(why) => self.toast(format!("OCR: {why}"), true),
            crate::ocr::OcrNews::Read(_) => {}
        }
    }

    /// The LaTeX project's PDF, version `version`, opened in the dock in
    /// place of the one there.
    pub(crate) fn show_tex_pdf(&mut self, path: &Path, version: u64) {
        if self.tex_pdf.as_ref().is_some_and(|t| t.pdf_version == version) || self.tex_pdf_opening.is_some_and(|(_, v)| v == version) {
            return;
        }
        let id = self.service.new_doc_id();
        self.service.send(Request::Open { id, path: path.to_path_buf(), password: None });
        self.tex_pdf_opening = Some((id, version));
    }

    /// What the TeX dock asked for.
    pub(crate) fn env_action(&mut self, action: crate::texdock::EnvAction) {
        use crate::texdock::EnvAction;
        let Some(env) = self.tex_env.as_mut() else { return };
        let done = match action {
            EnvAction::Plan(bundle) => env.plan(bundle),
            EnvAction::Create => env.create(),
            EnvAction::Cancel => {
                env.cancel();
                Ok(())
            }
            EnvAction::Remove => env.remove(),
        };
        if let Err(why) = done {
            self.toast(why, true);
        }
    }

    /// TeX's planning or making, once a frame.
    pub(crate) fn tend_tex(&mut self, ctx: &Context) {
        if let Some(env) = self.tex_env.as_mut() {
            env.poll();
            if env.working() {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            // texlab from Quark's own TeX, once there is one.
            if let Some(tex) = self.tex.as_mut().filter(|t| t.own_texlab.is_none()) {
                if let Some(marker) = env.marker() {
                    tex.set_texlab(marker.texlab_bin);
                }
            }
        }
        if let Some(tex) = self.tex.as_mut() {
            if tex.poll() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        self.tend_builds(ctx);
    }

    /// A typesetting asked for, begun once TeX is there; one under way,
    /// watched; one ended, its PDF shown.
    fn tend_builds(&mut self, ctx: &Context) {
        let Some(tex) = self.tex.as_mut() else { return };
        if std::mem::take(&mut tex.wants_build) {
            let env = self.tex_env.as_mut();
            match env.and_then(|e| e.marker().map(|m| (m, e.home().to_path_buf(), e.person().to_path_buf(), e.sandbox()))) {
                Some((marker, tex_home, person, sandbox)) => {
                    let tools = crate::texbuild::Tools { marker, tex_home, person, sandbox };
                    let active = tex.session.editor().buffer().path().map(Path::to_path_buf);
                    let root = tex.root.clone();
                    if let Err(why) = tex.builds.compile(&root, active.as_deref(), &tools) {
                        self.toast(format!("TeX could not be started: {why}"), true);
                        return;
                    }
                }
                None => {
                    tex.view = crate::tex::DockView::Tex;
                    self.toast("Install TeX to typeset: it is in the panel on the right.", false);
                    return;
                }
            }
        }
        if tex.builds.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if tex.builds.poll() {
            let last = tex.builds.last.clone();
            // A build with errors and no PDF opens its log, where they are.
            if let Some(last) = &last {
                if last.state == crate::texbuild::Ended::Failed {
                    tex.view = crate::tex::DockView::Log;
                }
            }
            if let Some((pdf, version)) = last.and_then(|l| l.pdf.map(|p| (p, l.version))) {
                self.show_tex_pdf(&pdf, version);
            }
        }
    }

    /// What the library asked for.
    pub(crate) fn books_action(&mut self, action: crate::books::BooksAction) {
        use crate::books::BooksAction;
        match action {
            BooksAction::None => {}
            BooksAction::Open(path) => self.startup_files.push(path),
            BooksAction::Rename { path, title, author } => {
                if let Err(e) = self.library.rename(&path, &title, &author) {
                    self.toast(e, true);
                }
            }
            BooksAction::SetFolders(folders) => {
                self.prefs.library_folders = folders.clone();
                save_prefs(&self.prefs);
                self.library.set_folders(folders);
                self.library.walk();
            }
            BooksAction::LookAgain => self.library.walk(),
        }
        self.sync_books();
    }

    /// The library's walk, where the book on screen was left, and the covers
    /// come on screen: once a frame.
    pub(crate) fn tend_library(&mut self, ctx: &Context) {
        if self.library.poll() {
            self.sync_books();
        }
        if self.library.walking() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        if let Some(path) = self.tab().and_then(|t| t.path().cloned()) {
            let page = self.tab().map_or(0, |t| t.current_page);
            self.library.left_at(&path, page);
        }
        for path in std::mem::take(&mut self.home.books.wanted) {
            match self.library.cover_file(&path) {
                Some(keep) => self.service.send(Request::Cover {
                    path,
                    max_edge: crate::books::COVER_EDGE,
                    keep: Some(keep),
                }),
                None => self.home.books.cover_drawn(ctx, path, None),
            }
        }
    }

    pub(crate) fn palette(&self) -> Palette {
        Palette::for_mode(self.theme)
    }

    /// The document on screen, if one is.
    ///
    /// Home deliberately reports `None` even when documents are open behind
    /// it. Every command handler already treats "no document" as "do nothing",
    /// so this is what stops Ctrl+S on the home screen from silently saving
    /// whichever file happens to be in the background — the guard lives here
    /// once instead of at sixty-odd call sites. `command_enabled` reads the
    /// same signal, so the menu and the command palette grey themselves out to
    /// match. The few handlers that reach for `self.active` directly rather
    /// than through here guard on `home.visible` themselves.
    pub(crate) fn tab(&self) -> Option<&Tab> {
        if !self.on_document() {
            return None;
        }
        self.tabs.get(self.active)
    }

    pub(crate) fn tab_mut(&mut self) -> Option<&mut Tab> {
        if !self.on_document() {
            return None;
        }
        self.tabs.get_mut(self.active)
    }

    /// Switches to home, keeping `active` where it was so leaving home again
    /// returns to the document the user was last reading.
    /// Whether a document is the surface showing: not home, not LaTeX.
    pub(crate) fn on_document(&self) -> bool {
        !self.home.visible && !self.tex_visible
    }

    /// The LaTeX project, if one is open.
    pub(crate) fn show_tex(&mut self) {
        if self.tex.is_some() {
            self.home.visible = false;
            self.tex_visible = true;
        }
    }

    /// Open the LaTeX project in `root`, in place of any open — which must
    /// have nothing unsaved.
    pub(crate) fn open_tex(&mut self, root: &std::path::Path) {
        if let Some(open) = &self.tex {
            if !open.unsaved().is_empty() {
                self.toast(format!("{} has unsaved files — save them first", open.name()), true);
                self.show_tex();
                return;
            }
        }
        if !root.is_dir() {
            self.toast(format!("{} is not a folder", root.display()), true);
            return;
        }
        self.tex = Some(crate::tex::TexProject::open(root));
        // Where TeX stands, looked at without starting anything or asking
        // the network.
        let env = self.tex_env.get_or_insert_with(quark_lab::tex::TexEnv::at_home);
        env.check();
        let typed = root.display().to_string();
        self.prefs.tex_projects.retain(|p| *p != typed);
        self.prefs.tex_projects.insert(0, typed);
        self.prefs.tex_projects.truncate(12);
        save_prefs(&self.prefs);
        self.show_tex();
    }

    /// Make a LaTeX project from template `id`, called `name`, in `parent`
    /// (`~` the home folder), and open it, typeset.
    pub(crate) fn new_tex(&mut self, id: &str, name: &str, parent: &str) {
        let at = quark_pdf::library::expand(parent);
        match quark_tex::templates::create(id, &at, name) {
            Ok(made) => {
                self.prefs.tex_parent = parent.to_string();
                self.open_tex(&made.folder);
                if let Some(tex) = &mut self.tex {
                    tex.open_at(&made.main.display().to_string(), 1);
                    tex.wants_build = true;
                }
            }
            Err(why) => self.toast(format!("The project could not be made: {why}"), true),
        }
    }

    /// Close the LaTeX project; unless forced, not with unsaved files.
    pub(crate) fn close_tex(&mut self, force: bool) {
        let Some(open) = &self.tex else { return };
        let unsaved = open.unsaved();
        if !force && !unsaved.is_empty() {
            self.dialog = Some(Dialog::Confirm {
                title: CLOSE_TEX.into(),
                body: format!("{} has changes not saved. Close it anyway, and lose them?", unsaved.join(", ")),
                confirm_label: "Close Without Saving".into(),
                danger: true,
            });
            return;
        }
        self.tex = None;
        // Its PDF goes with it, and any build under way stops as it is dropped.
        if let Some(pdf) = self.tex_pdf.take() {
            self.service.send(Request::Close { id: pdf.id });
        }
        self.tex_pdf_opening = None;
        if self.tex_visible {
            self.tex_visible = false;
            if self.tabs.is_empty() {
                self.show_home();
            }
        }
    }

    pub(crate) fn show_home(&mut self) {
        self.home.visible = true;
        self.tex_visible = false;
        // Where the books just read were left.
        self.sync_books();
    }

    /// Where the visible surface sits in the tab strip. Home is slot 0 and
    /// documents follow it, which is the order they are drawn in.
    pub(crate) fn strip_position(&self) -> usize {
        if self.home.visible {
            0
        } else {
            self.active + 1
        }
    }

    /// Selects a slot of the tab strip, counting home as slot 0.
    pub(crate) fn select_strip_position(&mut self, slot: usize) {
        match slot {
            0 => self.show_home(),
            n => self.show_tab(n - 1),
        }
        self.textures.clear();
    }

    /// Switches to a document, leaving home if it was showing.
    pub(crate) fn show_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.home.visible = false;
            self.tex_visible = false;
            self.active = index;
        }
    }

    pub(crate) fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            error,
            // Errors stay long enough to read; confirmations do not linger.
            ttl: if error { 8.0 } else { 3.5 },
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    // --- opening and closing ---

    pub(crate) fn open_path(&mut self, path: &Path, password: Option<String>) {
        // Re-opening a document already on screen just switches to its tab,
        // rather than showing the same file twice.
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.path().map(|p| p.as_path()) == Some(path))
        {
            self.show_tab(i);
            return;
        }
        let id = self.service.new_doc_id();
        self.service.send(Request::Open {
            id,
            path: path.to_path_buf(),
            password,
        });
    }

    pub(crate) fn open_dialog(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_title("Open PDF")
            .pick_files()
        {
            for f in files {
                self.open_path(&f, None);
            }
        }
    }

    pub(crate) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let unsaved = self.tabs[index].has_unsaved_changes();
        if unsaved && self.prefs.confirm_destructive {
            let name = self.tabs[index].title();
            self.dialog = Some(Dialog::Confirm {
                title: "Close Without Saving".into(),
                body: format!("“{name}” has unsaved changes. Close it anyway?"),
                confirm_label: "Discard Changes".into(),
                danger: true,
            });
            // The confirmation applies to the active tab, so make it active —
            // and visible, or the dialog would name a document that is not on
            // screen.
            self.show_tab(index);
            return;
        }
        self.force_close_tab(index);
    }

    pub(crate) fn force_close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let id = self.tabs[index].id;
        self.service.send(Request::Close { id });
        self.tabs.remove(index);
        self.textures.clear();
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        // Closing the last document lands on home rather than on a blank
        // window — that is the point of home being a tab.
        if self.tabs.is_empty() {
            self.show_home();
        }
    }

    // --- saving ---

    pub(crate) fn save(&mut self, as_new: bool) {
        let Some(tab) = self.tab() else { return };
        let id = tab.id;
        let has_path = tab.path().is_some();

        let path = if as_new || !has_path {
            let start = self
                .tab()
                .and_then(|t| t.path().cloned())
                .unwrap_or_else(|| PathBuf::from("Untitled.pdf"));
            rfd::FileDialog::new()
                .add_filter("PDF documents", &["pdf"])
                .set_file_name(
                    start
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Untitled.pdf".into()),
                )
                .save_file()
        } else {
            None
        };

        if (as_new || !has_path) && path.is_none() {
            return; // the user cancelled
        }
        self.service.send(Request::Save { id, path });
    }

    // --- edits ---

    /// Sends a document operation, recording it for undo.
    pub(crate) fn apply_op(&mut self, op: DocOp) {
        let Some(tab) = self.tab() else { return };
        let id = tab.id;

        if op.is_destructive() && self.prefs.confirm_destructive {
            // Handled by the caller, which puts up the confirmation first.
        }
        self.service.send(Request::Apply {
            id,
            op: Box::new(op),
        });
    }

    /// A new annotation: a change to the document at once — undone with
    /// ⌘Z, kept when it is saved — or, for a redaction mark, held here until
    /// the redactions are applied.
    pub(crate) fn commit_annotation(&mut self, ann: Annotation) {
        let Some(tab) = self.tab_mut() else { return };
        if quark_pdf::edits::mark_of(&ann).is_none() {
            tab.pending_annotations.push(ann);
            return;
        }
        let id = tab.id;
        self.service.send(Request::Apply {
            id,
            op: Box::new(DocOp::AddAnnotations(vec![ann])),
        });
    }

    pub(crate) fn delete_annotation(&mut self, id: quark_core::annot::AnnotId) {
        let Some(tab) = self.tab_mut() else { return };
        tab.selected_annotation = None;
        // A mark waiting to be applied is simply dropped; one in the
        // document is a change to it, undone like any.
        if let Some(i) = tab.pending_annotations.iter().position(|a| a.id == id) {
            tab.pending_annotations.remove(i);
            return;
        }
        if tab.annotations.iter().any(|a| a.id == id) {
            let doc = tab.id;
            self.service.send(Request::Apply {
                id: doc,
                op: Box::new(DocOp::RemoveAnnotations(vec![id])),
            });
        }
    }

    /// Recolour an annotation or change what it says. A mark waiting to be
    /// applied changes where it waits; one in the document is a change to
    /// it, undone like any.
    pub(crate) fn edit_annotation(
        &mut self,
        id: quark_core::annot::AnnotId,
        color: Option<quark_core::annot::Color>,
        contents: Option<String>,
    ) {
        let Some(tab) = self.tab_mut() else { return };
        if let Some(a) = tab.pending_annotations.iter_mut().find(|a| a.id == id) {
            if let Some(c) = color {
                a.color = c;
            }
            if let Some(text) = contents {
                a.contents = text;
            }
            return;
        }
        let doc = tab.id;
        self.service.send(Request::Apply {
            id: doc,
            op: Box::new(DocOp::EditAnnotation { id, color, contents }),
        });
    }

    /// Reply to an annotation, as the person using Quark.
    pub(crate) fn reply_to(&mut self, id: quark_core::annot::AnnotId, text: String) {
        let author = quark_core::tools::whoami();
        let Some(tab) = self.tab_mut() else { return };
        if let Some(a) = tab.pending_annotations.iter_mut().find(|a| a.id == id) {
            a.replies.push(quark_core::annot::Reply { author, contents: text, created: quark_core::annot::now_rfc3339() });
            return;
        }
        let doc = tab.id;
        self.service.send(Request::Apply { id: doc, op: Box::new(DocOp::ReplyTo { id, text, author }) });
    }

    // --- service responses ---

    pub(crate) fn drain_service(&mut self, ctx: &Context) {
        for response in self.service.poll() {
            match response {
                // The LaTeX PDF, made again: opened where the last was being
                // read, in its place, and never a tab or a recent file.
                Response::Opened { id, info } if self.tex_pdf_opening.is_some_and(|(o, _)| o == id) => {
                    let version = self.tex_pdf_opening.take().map_or(0, |(_, v)| v);
                    let mut tab = Tab::new(id, *info, quark_core::layout::PageMode::Continuous, quark_core::layout::ZoomMode::FitWidth);
                    tab.pdf_version = version;
                    if let Some(old) = self.tex_pdf.take() {
                        tab.zoom = old.zoom;
                        tab.current_page = old.current_page.min(tab.page_count().saturating_sub(1));
                        tab.scroll = old.scroll;
                        self.service.send(Request::Close { id: old.id });
                    }
                    self.service.send(Request::ReadLinks { id });
                    self.tex_pdf = Some(tab);
                }
                Response::Opened { id, info } => {
                    let mut tab = Tab::new(id, *info, self.prefs.page_mode, self.prefs.zoom);
                    if let Some(path) = tab.path().cloned() {
                        let key = path.to_string_lossy().into_owned();
                        // A book of the library is opened where it was left,
                        // however long ago that was.
                        let left = self.prefs.remembered_page(&key).or_else(|| {
                            self.prefs.remember_position.then(|| self.library.page_left(&path)).flatten()
                        });
                        if let Some(page) = left {
                            tab.current_page = page.min(tab.info.page_count.saturating_sub(1));
                        }
                        self.prefs.push_recent(&key, tab.current_page);
                    }
                    // Everything else about the document is fetched lazily.
                    self.service.send(Request::ReadOutline { id });
                    self.service.send(Request::ReadAnnotations { id });
                    self.service.send(Request::ReadFields { id });
                    self.service.send(Request::ReadAttachments { id });
                    self.service.send(Request::ReadLinks { id });

                    let go_to = tab.current_page;
                    self.tabs.push(tab);
                    // Before the `go_to` below: `tab_mut` reports `None` while
                    // home is showing, so restoring the reading position has
                    // to happen after the new document is the visible one.
                    self.show_tab(self.tabs.len() - 1);
                    if go_to > 0 {
                        if let Some(t) = self.tab_mut() {
                            t.go_to_page(go_to);
                        }
                    }
                    self.textures.clear();
                }

                Response::PasswordRequired { id, path, wrong } => {
                    let _ = id;
                    self.dialog = Some(Dialog::Password {
                        path,
                        wrong,
                        entry: String::new(),
                    });
                }

                Response::Failed { what, error, .. } => {
                    self.toast(format!("Could not {what}: {error}"), true);
                }

                Response::Rendered {
                    id,
                    page,
                    token,
                    raster,
                } => {
                    // A raster for a view the user has already left is thrown
                    // away rather than uploaded; that is what the token is for.
                    let Some(tab) = self.tabs.iter().chain(self.tex_pdf.iter()).find(|t| t.id == id) else {
                        continue;
                    };
                    if token != tab.render_token {
                        continue;
                    }
                    let key = crate::textures::PageKey::new(
                        page,
                        tab.raster_scale(),
                        tab.rotation,
                        self.prefs.tint,
                    ).of(id);
                    self.textures.insert(ctx, key, &raster);
                }

                Response::OcrPrep { id, bytes, blank, layout } => {
                    let Some(tesseract) = quark_lab::ocr::Tesseract::find() else { continue };
                    let Some(tab) = self.tabs.iter().chain(self.tex_pdf.iter()).find(|t| t.id == id) else { continue };
                    let chosen = if tab.selected_pages.is_empty() {
                        vec![tab.current_page]
                    } else {
                        let mut c = tab.selected_pages.clone();
                        c.sort_unstable();
                        c
                    };
                    let count = tab.page_count();
                    let langs = tesseract.languages();
                    let mut picked: Vec<String> =
                        self.prefs.ocr_langs.iter().filter(|l| langs.contains(l)).cloned().collect();
                    if picked.is_empty() {
                        picked = langs.iter().find(|l| *l == "eng").or(langs.first()).cloned().into_iter().collect();
                    }
                    let which = if blank.is_empty() { crate::ocr::Which::Chosen } else { crate::ocr::Which::Blank };
                    self.ocr_prep = Some((id, bytes, layout));
                    self.dialog = Some(crate::dialogs::Dialog::Ocr { blank, chosen, count, which, langs, picked });
                }

                Response::Cover { path, raster } => {
                    self.home.books.cover_drawn(ctx, path, raster.as_deref());
                }

                Response::Thumbnail {
                    id, page, raster, ..
                } => {
                    let Some(tab) = self.tabs.iter().chain(self.tex_pdf.iter()).find(|t| t.id == id) else {
                        continue;
                    };
                    let size = tab.info.page_size(page);
                    let scale = crate::panels::thumb_edge(ctx) as f32 / size.width.max(size.height).max(1.0);
                    let key = crate::textures::PageKey::new(page, scale, Rot::D0, self.prefs.tint).of(id);
                    self.textures.insert(ctx, key, &raster);
                }

                Response::PageText { id, page, text } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.page_text.insert(page, *text);
                        tab.text_pending.retain(|&p| p != page);
                    }
                }

                Response::SearchProgress {
                    id,
                    token,
                    page,
                    total,
                    hits,
                } => {
                    if token != self.search_token {
                        continue;
                    }
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.search.total_pages = total;
                        tab.search.scanned_pages = page + 1;
                        tab.search.hits.extend(hits);
                    }
                }

                Response::SearchFinished { id, token } => {
                    if token != self.search_token {
                        continue;
                    }
                    let mut jump = None;
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.search.running = false;
                        let current = tab.current_page;
                        tab.search.select_nearest(current);
                        jump = tab.search.current_hit().cloned();
                    }
                    if let (Some(hit), Some(tab)) = (jump, self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id))
                    {
                        reveal_hit(tab, &hit);
                    }
                }

                Response::Annotations { id, annotations } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        // Keep identifiers ahead of anything read from the file.
                        tab.annotations = annotations;
                    }
                }
                Response::Outline { id, bookmarks } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.outline = bookmarks;
                    }
                }
                Response::Fields { id, fields } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.fields = fields;
                    }
                }
                Response::Attachments { id, attachments } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.attachments = attachments;
                    }
                }
                Response::Links { id, links } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.links = links;
                    }
                }

                Response::Applied {
                    id,
                    label,
                    info,
                    redaction,
                    sanitize,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.info = *info;
                        tab.invalidate_layout();
                        tab.ensure_layout();
                        tab.bump_token();
                        tab.selected_pages.retain(|&p| p < tab.info.page_count);
                        if tab.current_page >= tab.info.page_count {
                            tab.current_page = tab.info.page_count.saturating_sub(1);
                        }
                    }
                    // The page images and everything derived from them are now
                    // stale, so they are re-fetched rather than patched.
                    self.textures.clear();
                    self.service.send(Request::ReadAnnotations { id });
                    self.service.send(Request::ReadOutline { id });
                    self.service.send(Request::ReadFields { id });
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.page_text.clear();
                        tab.text_pending.clear();
                    }

                    if let Some(r) = redaction {
                        self.toast(
                            format!(
                                "Redacted {} text run(s), {} image(s) and {} comment(s) across {} page(s)",
                                r.text_runs_removed,
                                r.images_removed,
                                r.annotations_removed,
                                r.pages_touched
                            ),
                            false,
                        );
                    } else if let Some(s) = sanitize {
                        self.toast(s.summary(), false);
                    } else {
                        self.toast(label, false);
                    }
                }

                Response::Saved { id, path } => {
                    if let Some(tab) = self.tabs.iter_mut().chain(self.tex_pdf.iter_mut()).find(|t| t.id == id) {
                        tab.info.dirty = false;
                        tab.info.path = Some(path.clone());
                    }
                    self.toast(
                        format!(
                            "Saved {}",
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ),
                        false,
                    );
                }

                Response::Exported { paths, .. } => {
                    self.toast(
                        match paths.len() {
                            0 => "Nothing was exported".to_string(),
                            1 => format!("Exported {}", paths[0].display()),
                            n => format!("Exported {n} files"),
                        },
                        false,
                    );
                }
            }
        }
    }
}

/// Scrolls a search hit into view.
pub(crate) fn reveal_hit(tab: &mut Tab, hit: &quark_core::search::SearchHit) {
    let quads = tab
        .page_text
        .get(&hit.page)
        .map(|t| t.quads_for_range(hit.char_index, hit.char_index + hit.char_len))
        .unwrap_or_default();
    match quads.first() {
        Some(q) => tab.reveal(hit.page, *q),
        // The page's text has not arrived yet, so the best that can be done is
        // to land on the right page.
        None => tab.go_to_page(hit.page),
    }
}

/// The confirmation closing a LaTeX project with unsaved files puts up.
pub(crate) const CLOSE_TEX: &str = "Close Project";

// --- preferences persistence ---

/// The library as kept beside the settings; in memory when there is nowhere
/// to keep it.
fn library_kept() -> quark_pdf::library::Library {
    match dirs::data_dir() {
        Some(dir) => quark_pdf::library::Library::kept_in(&dir.join("Quark")),
        None => quark_pdf::library::Library::default(),
    }
}

fn prefs_path() -> Option<PathBuf> {
    let mut d = dirs::config_dir()?;
    d.push("Quark");
    Some(d.join("settings.toml"))
}

fn load_prefs() -> Prefs {
    let Some(p) = prefs_path() else {
        return Prefs::default();
    };
    match std::fs::read_to_string(&p) {
        // A settings file from an older version is missing fields; serde's
        // defaults fill them, which is why every field carries one.
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!("settings.toml could not be parsed ({e}); using defaults");
            Prefs::default()
        }),
        Err(_) => Prefs::default(),
    }
}

pub(crate) fn save_prefs(prefs: &Prefs) {
    let Some(p) = prefs_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match toml::to_string_pretty(prefs) {
        Ok(s) => {
            if let Err(e) = std::fs::write(&p, s) {
                tracing::warn!("could not write settings: {e}");
            }
        }
        Err(e) => tracing::warn!("could not serialise settings: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_core::layout::{PageMode, ZoomMode};
    use quark_core::prefs::{PageTint, SidePanel};


    #[test]
    fn a_settings_file_written_before_the_dock_existed_still_loads() {
        // Every field carries a serde default precisely so an older file does
        // not reset the user's whole configuration. Adding `dock_width` is the
        // first new field since the redesign, so this checks the mechanism
        // rather than trusting it.
        let old = "side_panel_width = 300.0\nshow_toolbar = false\n";
        let p: Prefs = toml::from_str(old).expect("an older settings file must still parse");
        assert_eq!(p.side_panel_width, 300.0, "existing values are kept");
        assert!(!p.show_toolbar);
        assert_eq!(
            p.dock_width,
            Prefs::default().dock_width,
            "a missing dock width falls back to the default"
        );
    }

    #[test]
    fn the_dock_width_survives_a_round_trip() {
        let p = Prefs {
            dock_width: 341.0,
            ..Prefs::default()
        };
        let back: Prefs = toml::from_str(&toml::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.dock_width, 341.0);
    }
    #[test]
    fn preferences_round_trip_through_toml() {
        // This is what makes settings survive a restart.
        let mut p = Prefs::default();
        p.page_mode = PageMode::TwoUpCover;
        p.zoom = ZoomMode::Custom(1.75);
        p.tint = PageTint::Sepia;
        p.side_panel = SidePanel::Bookmarks;
        p.push_recent("/tmp/a.pdf", 4);

        let s = toml::to_string_pretty(&p).expect("serialise");
        let back: Prefs = toml::from_str(&s).expect("deserialise");
        assert_eq!(back.page_mode, PageMode::TwoUpCover);
        assert_eq!(back.zoom, ZoomMode::Custom(1.75));
        assert_eq!(back.tint, PageTint::Sepia);
        assert_eq!(back.recent[0].path, "/tmp/a.pdf");
        assert_eq!(back.recent[0].page, 4);
    }

    #[test]
    fn a_settings_file_missing_fields_still_loads() {
        let partial = "theme_dark = false\n";
        let p: Prefs = toml::from_str(partial).expect("partial settings should load");
        assert!(!p.theme_dark);
        assert_eq!(p.recent_limit, 20, "missing fields take their defaults");
    }

    #[test]
    fn a_corrupt_settings_file_does_not_stop_startup() {
        // `load_prefs` swallows the error; this asserts the parse itself fails
        // so that the fallback is actually exercised.
        assert!(toml::from_str::<Prefs>("this is not toml {{{").is_err());
    }
}
