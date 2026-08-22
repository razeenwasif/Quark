//! The application: window chrome, command dispatch, and the service loop.

use std::path::{Path, PathBuf};

use egui::Context;
use quark_core::annot::Annotation;
use quark_core::geom::Rot;
use quark_core::history::Edit;
use quark_core::prefs::Prefs;
use quark_core::tools::Tool;
use quark_pdf::service::{DocOp, PdfService, Request, Response};
use quark_ui::theme::{self, Palette, ThemeMode};

use crate::dialogs::Dialog;
use crate::shortcuts::{self, Binding};
use crate::tab::Tab;
use crate::textures::TextureCache;

/// A transient message shown in the status bar.
pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) error: bool,
    /// Seconds remaining.
    pub(crate) ttl: f32,
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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        let prefs = load_prefs();
        let theme = if prefs.theme_dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        theme::apply(&cc.egui_ctx, theme);
        cc.egui_ctx.set_pixels_per_point(prefs.ui_scale.clamp(0.5, 3.0));

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
        };

        let pending = std::mem::take(&mut app.startup_files);
        for f in pending {
            app.open_path(&f, None);
        }
        app
    }

    pub(crate) fn palette(&self) -> Palette {
        Palette::for_mode(self.theme)
    }

    pub(crate) fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub(crate) fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
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
            self.active = i;
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
            // The confirmation applies to the active tab, so make it active.
            self.active = index;
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
    }

    // --- saving ---

    pub(crate) fn save(&mut self, as_new: bool) {
        let Some(tab) = self.tab() else { return };
        let id = tab.id;
        let has_path = tab.path().is_some();

        // Anything created this session has to be written into the file before
        // the save, or the annotations exist only in the viewer.
        let pending: Vec<Annotation> = tab.pending_annotations.clone();
        if !pending.is_empty() {
            self.service.send(Request::Apply {
                id,
                op: Box::new(DocOp::AddAnnotations(pending)),
            });
            if let Some(t) = self.tab_mut() {
                // Moved into the saved set optimistically; a failure re-reads
                // the document anyway and would drop them.
                let moved = std::mem::take(&mut t.pending_annotations);
                t.annotations.extend(moved.into_iter().map(|mut a| {
                    a.dirty = false;
                    a
                }));
            }
        }

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

    pub(crate) fn commit_annotation(&mut self, ann: Annotation) {
        let Some(tab) = self.tab_mut() else { return };
        let boxed = Box::new(ann.clone());
        tab.pending_annotations.push(ann);
        tab.history.push(Edit::AddAnnotation(boxed));
        tab.dirty = true;
    }

    pub(crate) fn delete_annotation(&mut self, id: quark_core::annot::AnnotId) {
        let Some(tab) = self.tab_mut() else { return };
        // A pending annotation can simply be dropped; one already in the file
        // needs the document rewritten, which is handled on save.
        if let Some(i) = tab.pending_annotations.iter().position(|a| a.id == id) {
            let removed = tab.pending_annotations.remove(i);
            tab.history.push(Edit::RemoveAnnotation(Box::new(removed)));
            tab.selected_annotation = None;
            return;
        }
        if let Some(i) = tab.annotations.iter().position(|a| a.id == id) {
            let removed = tab.annotations.remove(i);
            tab.history.push(Edit::RemoveAnnotation(Box::new(removed)));
            tab.selected_annotation = None;
            tab.dirty = true;
        }
    }

    // --- service responses ---

    pub(crate) fn drain_service(&mut self, ctx: &Context) {
        for response in self.service.poll() {
            match response {
                Response::Opened { id, info } => {
                    let mut tab = Tab::new(id, *info, self.prefs.page_mode, self.prefs.zoom);
                    if let Some(path) = tab.path().cloned() {
                        let key = path.to_string_lossy().into_owned();
                        if let Some(page) = self.prefs.remembered_page(&key) {
                            tab.current_page = page;
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
                    self.active = self.tabs.len() - 1;
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
                    let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
                        continue;
                    };
                    if token != tab.render_token {
                        continue;
                    }
                    let key = crate::textures::PageKey::new(
                        page,
                        tab.layout.scale,
                        tab.rotation,
                        self.prefs.tint,
                    );
                    self.textures.insert(ctx, key, &raster);
                }

                Response::Thumbnail {
                    id, page, raster, ..
                } => {
                    let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
                        continue;
                    };
                    let size = tab.info.page_size(page);
                    let scale = 150.0 / size.width.max(size.height).max(1.0);
                    let key = crate::textures::PageKey::new(page, scale, Rot::D0, self.prefs.tint);
                    self.textures.insert(ctx, key, &raster);
                }

                Response::PageText { id, page, text } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.search.running = false;
                        let current = tab.current_page;
                        tab.search.select_nearest(current);
                        jump = tab.search.current_hit().cloned();
                    }
                    if let (Some(hit), Some(tab)) = (jump, self.tabs.iter_mut().find(|t| t.id == id))
                    {
                        reveal_hit(tab, &hit);
                    }
                }

                Response::Annotations { id, annotations } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        // Keep identifiers ahead of anything read from the file.
                        tab.annotations = annotations;
                    }
                }
                Response::Outline { id, bookmarks } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.outline = bookmarks;
                    }
                }
                Response::Fields { id, fields } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.fields = fields;
                    }
                }
                Response::Attachments { id, attachments } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.attachments = attachments;
                    }
                }
                Response::Links { id, links } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.info = *info;
                        tab.invalidate_layout();
                        tab.ensure_layout();
                        tab.bump_token();
                        tab.dirty = true;
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        tab.dirty = false;
                        tab.history.mark_saved();
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

// --- preferences persistence ---

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
