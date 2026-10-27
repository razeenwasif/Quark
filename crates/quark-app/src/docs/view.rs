//! Drawing a document: the outline island, the well with its sheet, and the
//! dock; and the mouse on the sheet.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use quark_doc::{BlockKind, Marks, Pos, Selection, survey};
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

use super::icons::{self as doc_icons, DocIcon};
use super::input::{self, Ask};
use super::layout::{self, Marker};
use super::{DocTab, DockTab};
use crate::app::App;

const OUTLINE_W: f32 = 248.0;
pub(crate) const DOCK_W: f32 = 316.0;
const HEADER_H: f32 = 58.0;
/// Room above the sheet for the formatting bar, and below it for the count.
const TOP_ROOM: f32 = 58.0;
const BOTTOM_ROOM: f32 = 72.0;

/// A drag selecting text.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drag {
    anchor: Pos,
}

/// Find, and find and replace.
#[derive(Debug, Clone, Default)]
pub(crate) struct Find {
    pub(crate) query: String,
    pub(crate) with: String,
    pub(crate) match_case: bool,
    pub(crate) replace: bool,
    pub(crate) current: usize,
    /// Put the keyboard in the query field on the next frame.
    pub(crate) focus: bool,
}

/// What the document view asks the application to do, once it has given
/// back its borrow.
pub(crate) enum Action {
    Toast(String, bool),
    OpenLink(String),
    Rename(String),
    ToggleDock,
    Export(super::export::Format),
    Claude(super::claude::Asked),
    OpenDock,
    ClaudeAcknowledged,
    /// "Explain" under a selection: a question in the dock about it.
    ExplainSelection,
}

/// The id the sheet takes keyboard focus under.
pub(crate) fn paper_id() -> Id {
    Id::new("quark-doc-paper")
}

/// What the surface needs to know from outside the document.
pub(crate) struct Around {
    pub(crate) dock_open: bool,
    /// A dialog or the palette has the keyboard.
    pub(crate) modal: bool,
    pub(crate) claude: super::claude::Status,
}

impl App {
    /// The document showing, with its outline and dock, in `ui`'s space.
    pub(crate) fn docs_surface(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(index) = self.docs.shown.filter(|i| *i < self.docs.tabs.len()) else { return };
        let around = Around { dock_open: self.dock_open, modal: self.dialog.is_some() || self.show_palette, claude: self.docs_claude_status() };
        let busy = self.docs_claude.busy_with(self.docs.tabs[index].uid);
        let doing = self.docs_claude.activity().map(str::to_string);
        let tab = &mut self.docs.tabs[index];
        tab.claude_busy = busy;
        tab.claude_doing = doing;
        for a in surface(ui, p, tab, &around) {
            self.doc_action(index, a);
        }
    }

    fn doc_action(&mut self, index: usize, action: Action) {
        match action {
            Action::Toast(text, error) => self.toast(text, error),
            Action::OpenLink(url) => {
                if let Err(e) = crate::chrome::open_link(&url) {
                    self.toast(e, true);
                }
            }
            Action::Rename(name) => {
                if let Some(tab) = self.docs.tabs.get_mut(index) {
                    if let Err(e) = tab.rename(&name) {
                        self.toast(e, true);
                    }
                }
            }
            Action::ToggleDock => self.dock_open = !self.dock_open,
            Action::Export(format) => self.export_document(index, format),
            Action::Claude(asked) => self.ask_claude_in_doc(index, asked),
            Action::OpenDock => self.dock_open = true,
            Action::ClaudeAcknowledged => self.docs_claude.acknowledged = true,
            Action::ExplainSelection => {
                if let Some(tab) = self.docs.tabs.get(index) {
                    let quote = tab.editor.selected_text();
                    if !quote.trim().is_empty() {
                        self.ask_claude_in_doc(index, super::claude::Asked::Chat(format!("Explain this passage: “{}”", quote.trim())));
                    }
                }
                self.dock_open = true;
            }
        }
    }
}

/// Draws a document and reads the input to it; what it asks for besides.
pub(crate) fn surface(ui: &mut Ui, p: &Palette, tab: &mut DocTab, around: &Around) -> Vec<Action> {
    let ctx = ui.ctx().clone();
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());

    let dock_open = around.dock_open;
    let outline_rect = Rect::from_min_size(full.min, vec2(OUTLINE_W, full.height()));
    let dock_rect = dock_open.then(|| Rect::from_min_max(pos2(full.right() - DOCK_W, full.top()), full.max));
    let well = Rect::from_min_max(
        pos2(outline_rect.right() + theme::GAP, full.top()),
        pos2(dock_rect.map_or(full.right(), |d| d.left() - theme::GAP), full.bottom()),
    );
    let painter = ui.painter().clone();
    theme::paint_island(&painter, outline_rect, p);
    if let Some(d) = dock_rect {
        theme::paint_island(&painter, d, p);
    }
    painter.rect(well, CornerRadius::same(theme::RADIUS_ISLAND), p.canvas, Stroke::new(1.0, p.wash(0x0a)), egui::StrokeKind::Inside);

    let mut actions = Vec::new();
    // The page has the keyboard unless something else asked for it.
    if !around.modal && ctx.memory(|m| m.focused().is_none()) {
        ctx.memory_mut(|m| m.request_focus(paper_id()));
    }
    let focused = !around.modal && ctx.memory(|m| m.has_focus(paper_id()));
    if focused && tab.ask_box.is_none() && tab.link_draft.is_none() {
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(paper_id(), egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
        });
        for ask in input::read(tab, &ctx) {
            handle_ask(tab, ask, &mut actions);
        }
    }

    // --- the outline ---
    let mut outline_ui = ui.new_child(UiBuilder::new().max_rect(outline_rect.shrink2(vec2(10.0, 10.0))));
    outline(&mut outline_ui, p, tab);

    // --- the dock ---
    if let Some(d) = dock_rect {
        let mut dock_ui = ui.new_child(UiBuilder::new().max_rect(d.shrink2(vec2(10.0, 10.0))));
        super::comments::dock(&mut dock_ui, p, tab, &around.claude, &mut actions);
    }

    // --- the well ---
    let header = Rect::from_min_size(well.min, vec2(well.width(), HEADER_H));
    let mut header_ui = ui.new_child(UiBuilder::new().max_rect(header.shrink2(vec2(12.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    header_bar(&mut header_ui, p, tab, dock_open, &mut actions);

    let body = Rect::from_min_max(pos2(well.left(), header.bottom()), well.max);
    let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body));
    body_ui.set_clip_rect(body);
    sheet(&mut body_ui, p, tab, focused, &mut actions);

    format_bar(&ctx, p, tab, pos2(well.center().x, header.bottom() + 4.0), &mut actions);
    count_bar(&ctx, p, tab, pos2(well.center().x, well.bottom() - 14.0));
    link_editor(&ctx, p, tab);
    super::claude::ask_box(&ctx, p, tab, &mut actions);

    theme::mask_corners(ui.painter(), well, theme::RADIUS_ISLAND as f32 - 1.0, p.ground);
    ui.painter().rect_stroke(well, CornerRadius::same(theme::RADIUS_ISLAND), Stroke::new(1.0, p.wash(0x0a)), egui::StrokeKind::Inside);
    actions
}

/// What a key asked for, done where the page alone cannot.
fn handle_ask(tab: &mut DocTab, ask: Ask, actions: &mut Vec<Action>) {
    match ask {
        Ask::Link => {
            let head = tab.editor.selection().head;
            let current = tab.editor.link_at(head).map(|(_, _, url)| url).unwrap_or_default();
            tab.link_draft = Some(current);
        }
        Ask::Comment => super::comments::start_comment(tab, actions),
        Ask::Find { replace } => {
            let selected = tab.editor.selected_text();
            let find = tab.find.get_or_insert_with(Find::default);
            if !selected.is_empty() && !selected.contains('\n') {
                find.query = selected;
            }
            find.replace = replace;
            find.focus = true;
        }
        Ask::FindNext { back } => find_step(tab, back),
        Ask::Claude => {
            if tab.editor.selection().is_caret() || tab.editor.selected_text().trim().is_empty() {
                tab.dock = DockTab::Claude;
                tab.focus_chat = true;
                actions.push(Action::OpenDock);
            } else {
                tab.ask_box = Some(String::new());
            }
        }
        Ask::Accept(id) => {
            tab.editor.accept(id);
        }
        Ask::Reject(id) => tab.editor.reject(id),
    }
}

/// Moves to the next (or previous) hit of the find query, selecting it.
pub(crate) fn find_step(tab: &mut DocTab, back: bool) {
    let Some(find) = &tab.find else { return };
    let hits = survey::find(&tab.editor.doc, &find.query, find.match_case);
    if hits.is_empty() {
        return;
    }
    let head = tab.editor.selection().end();
    let i = if back {
        let start = tab.editor.selection().start();
        hits.iter().rposition(|(a, _)| *a < start).unwrap_or(hits.len() - 1)
    } else {
        hits.iter().position(|(a, _)| *a >= head).unwrap_or(0)
    };
    let (a, b) = hits[i];
    tab.editor.set_selection(Selection::new(a, b));
    if let Some(find) = &mut tab.find {
        find.current = i;
    }
    tab.reveal = true;
}

// --- the outline island ---

fn outline(ui: &mut Ui, p: &Palette, tab: &mut DocTab) {
    let in_find = tab.find.is_some();
    let segments = [widgets::Segment::Text("Outline"), widgets::Segment::Text("Find")];
    if let Some(i) = widgets::segmented(ui, p, &segments, usize::from(in_find), true, 34.0) {
        if i == 1 && !in_find {
            tab.find = Some(Find { focus: true, ..Find::default() });
        } else if i == 0 {
            tab.find = None;
        }
    }
    ui.add_space(10.0);
    if tab.find.is_some() {
        super::comments::find_panel(ui, p, tab);
        return;
    }

    let headings = survey::outline(&tab.editor.doc);
    let here = tab.editor.selection().head.block;
    let current = headings.iter().rposition(|h| h.block <= here);
    let top = headings.iter().map(|h| h.level).min().unwrap_or(1);
    let list_h = (ui.available_height() - 150.0).max(80.0);
    egui::ScrollArea::vertical().id_salt("doc-outline").max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
        if headings.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Headings appear here as you write them.").font(theme::body(12.5)).color(p.text_faint));
        }
        for (i, h) in headings.iter().enumerate() {
            let depth = h.level.saturating_sub(top) as f32;
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let on = current == Some(i);
            if on {
                ui.painter().rect_filled(rect, CornerRadius::same(11), p.accent_soft);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(11), p.wash(0x0d));
            }
            let font = if depth == 0.0 { theme::heavy(12.5) } else { theme::strong(12.5) };
            let colour = if on {
                p.accent_text
            } else if depth == 0.0 {
                p.text
            } else {
                p.text_muted
            };
            let text = widgets::elide(&h.text, (34.0 - depth * 4.0) as usize, false);
            ui.painter().text(rect.left_center() + vec2(12.0 + depth * 14.0, 0.0), Align2::LEFT_CENTER, text, font, colour);
            if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                tab.editor.caret_to(Pos::new(h.block, 0));
                tab.jump = Some(h.block);
            }
        }
    });

    // What the document comes to.
    let stats = survey::stats(&tab.editor.doc);
    let open = tab.editor.doc.comments.iter().filter(|c| !c.resolved).count();
    let card = Rect::from_min_max(pos2(ui.max_rect().left(), ui.max_rect().bottom() - 128.0), ui.max_rect().max);
    ui.painter().rect_filled(card, CornerRadius::same(16), p.recess);
    let mut y = card.top() + 18.0;
    ui.painter().text(pos2(card.left() + 14.0, y), Align2::LEFT_CENTER, widgets::micro_caps("This document"), theme::heavy(10.5), p.text_faint);
    y += 24.0;
    let rows = [
        ("Words", group_thousands(stats.words)),
        ("Reading time", if stats.reading_minutes == 0 { "—".into() } else { format!("{} min", stats.reading_minutes) }),
        ("Open comments", open.to_string()),
        ("Saved as", "Markdown".into()),
    ];
    for (label, value) in rows {
        ui.painter().text(pos2(card.left() + 14.0, y), Align2::LEFT_CENTER, label, theme::body(12.5), p.text_muted);
        ui.painter().text(pos2(card.right() - 14.0, y), Align2::RIGHT_CENTER, value, theme::strong(12.5), p.text);
        y += 21.0;
    }
}

pub(crate) fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// --- the header ---

fn header_bar(ui: &mut Ui, p: &Palette, tab: &mut DocTab, dock_open: bool, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let (chip, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
    ui.painter().rect_filled(chip, CornerRadius::same(11), p.accent_soft);
    doc_icons::draw(ui.painter(), Rect::from_center_size(chip.center(), Vec2::splat(17.0)), DocIcon::Page, p.accent_text);
    ui.add_space(6.0);

    ui.vertical(|ui| {
        ui.add_space(9.0);
        ui.spacing_mut().item_spacing.y = 1.0;
        let file = tab.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(draft) = &mut tab.rename {
            let edit = egui::TextEdit::singleline(draft).font(theme::strong(13.0)).desired_width(260.0).margin(vec2(4.0, 1.0));
            let r = ui.add(edit);
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            let done = r.lost_focus();
            if done {
                let name = draft.clone();
                tab.rename = None;
                if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    actions.push(Action::Rename(name));
                }
            }
        } else {
            let r = ui.add(egui::Label::new(egui::RichText::new(&file).font(theme::strong(13.0)).color(p.text)).sense(Sense::click()));
            if r.on_hover_text("Rename").on_hover_cursor(egui::CursorIcon::Text).clicked() {
                tab.rename = Some(tab.title());
            }
        }
        let folder = tab.path.parent().map(crate::chrome::tilde).unwrap_or_default();
        let (status, colour) = if tab.conflict {
            ("Changed elsewhere — ⌘S to save over it".to_string(), p.warning)
        } else if let Some(e) = &tab.last_error {
            (widgets::elide(e, 48, false), p.error)
        } else if tab.editor.is_dirty() && (tab.exists || !tab.editor.doc.is_empty()) {
            ("Saving…".to_string(), p.text_faint)
        } else if !tab.exists {
            ("Not saved yet — saves as you write".to_string(), p.text_faint)
        } else {
            ("Saved".to_string(), p.success)
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(egui::RichText::new(format!("{folder} ·")).font(theme::body(11.0)).color(p.text_faint));
            ui.label(egui::RichText::new(status).font(theme::body(11.0)).color(colour));
        });
    });

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if widgets::icon_button(ui, p, Icon::Sidebar, if dock_open { "Hide comments and Claude" } else { "Show comments and Claude" }, 34.0, dock_open, true).clicked() {
            actions.push(Action::ToggleDock);
        }
        let export = export_button(ui, p);
        egui::Popup::menu(&export).show(|ui| {
            ui.set_min_width(220.0);
            for format in super::export::Format::ALL {
                if ui.button(format.label()).clicked() {
                    if format == super::export::Format::CopyMarkdown {
                        let md = quark_doc::markdown::write_blocks(&tab.editor.doc.blocks);
                        ui.ctx().copy_text(md.trim_end().to_string());
                        actions.push(Action::Toast("Copied as Markdown".into(), false));
                    } else {
                        actions.push(Action::Export(format));
                    }
                    ui.close();
                }
            }
        });
        widgets::separator(ui, p);
        let ed = &mut tab.editor;
        if widgets::icon_button(ui, p, Icon::Redo, &widgets::platform_keys("Redo (Ctrl+Shift+Z)"), 34.0, false, ed.can_redo()).clicked() {
            ed.redo();
        }
        if widgets::icon_button(ui, p, Icon::Undo, &widgets::platform_keys("Undo (Ctrl+Z)"), 34.0, false, ed.can_undo()).clicked() {
            ed.undo();
        }
    });
}

fn export_button(ui: &mut Ui, p: &Palette) -> egui::Response {
    let galley = ui.painter().layout_no_wrap("Export".into(), theme::strong(12.5), p.text);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 52.0, 34.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(12), p.wash(0x0d));
    }
    doc_icons::draw(ui.painter(), Rect::from_center_size(rect.left_center() + vec2(19.0, 0.0), Vec2::splat(16.0)), DocIcon::Export, p.text);
    ui.painter().galley(rect.left_center() + vec2(32.0, -galley.size().y / 2.0), galley, p.text);
    icons::draw(ui.painter(), Rect::from_center_size(rect.right_center() - vec2(13.0, 0.0), Vec2::splat(11.0)), Icon::ChevronDown, p.text_faint);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// --- the formatting bar ---

/// The styles the paragraph menu offers, as the menu names them.
const STYLES: [(&str, BlockKind); 6] = [
    ("Normal text", BlockKind::Paragraph),
    ("Title", BlockKind::Heading(1)),
    ("Heading", BlockKind::Heading(2)),
    ("Subheading", BlockKind::Heading(3)),
    ("Quote", BlockKind::Quote),
    ("Code", BlockKind::Code { lang: String::new() }),
];

fn style_name(kind: &BlockKind) -> &'static str {
    match kind {
        BlockKind::Heading(1) => "Title",
        BlockKind::Heading(2) => "Heading",
        BlockKind::Heading(3) => "Subheading",
        BlockKind::Heading(_) => "Small heading",
        BlockKind::Quote => "Quote",
        BlockKind::Code { .. } => "Code",
        BlockKind::Bullet { .. } => "Bulleted list",
        BlockKind::Numbered { .. } => "Numbered list",
        BlockKind::Task { .. } => "Checklist",
        BlockKind::Rule => "Divider",
        BlockKind::Paragraph => "Normal text",
    }
}

fn format_bar(ctx: &egui::Context, p: &Palette, tab: &mut DocTab, at: Pos2, actions: &mut Vec<Action>) {
    egui::Area::new(Id::new("doc-format-bar")).order(egui::Order::Middle).pivot(Align2::CENTER_TOP).fixed_pos(at).show(ctx, |ui| {
        capsule_frame(p).show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                let kind = tab.editor.current_kind().clone();
                let marks = tab.editor.current_marks();
                let style = style_button(ui, p, style_name(&kind));
                egui::Popup::menu(&style).show(|ui| {
                    ui.set_min_width(170.0);
                    for (name, k) in STYLES.iter() {
                        if ui.selectable_label(kind.same_family(k), *name).clicked() {
                            tab.editor.set_kind(k.clone());
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Divider").clicked() {
                        tab.editor.insert_rule();
                        ui.close();
                    }
                });
                widgets::separator(ui, p);
                let marks_on = |m: Marks| marks.contains(m);
                let glyphs: [(&str, Marks, &str, egui::FontId); 4] = [
                    ("B", Marks::BOLD, "Bold (Ctrl+B)", theme::heavy(14.5)),
                    ("I", Marks::ITALIC, "Italic (Ctrl+I)", egui::FontId::new(15.0, egui::FontFamily::Proportional)),
                    ("U", Marks::UNDERLINE, "Underline (Ctrl+U)", theme::strong(14.0)),
                    ("S", Marks::STRIKE, "Strikethrough (Ctrl+Shift+X)", theme::strong(14.0)),
                ];
                for (glyph, mark, tip, font) in glyphs {
                    if glyph_button(ui, p, glyph, font, mark, marks_on(mark), &widgets::platform_keys(tip)).clicked() {
                        tab.editor.toggle_mark(mark);
                    }
                }
                if doc_button(ui, p, DocIcon::Code, &widgets::platform_keys("Code (Ctrl+E)"), marks_on(Marks::CODE)).clicked() {
                    tab.editor.toggle_mark(Marks::CODE);
                }
                let in_link = tab.editor.link_at(tab.editor.selection().head).is_some();
                if doc_button(ui, p, DocIcon::Link, &widgets::platform_keys("Link (Ctrl+K)"), in_link).clicked() {
                    handle_ask(tab, Ask::Link, actions);
                }
                widgets::separator(ui, p);
                let lists: [(DocIcon, BlockKind, &str); 4] = [
                    (DocIcon::Bullets, BlockKind::Bullet { depth: 0 }, "Bulleted list (Ctrl+Shift+8)"),
                    (DocIcon::Numbers, BlockKind::Numbered { depth: 0 }, "Numbered list (Ctrl+Shift+7)"),
                    (DocIcon::Checklist, BlockKind::Task { checked: false, depth: 0 }, "Checklist (Ctrl+Shift+9)"),
                    (DocIcon::Quote, BlockKind::Quote, "Quote"),
                ];
                for (icon, k, tip) in lists {
                    if doc_button(ui, p, icon, &widgets::platform_keys(tip), kind.same_family(&k)).clicked() {
                        tab.editor.toggle_kind(k);
                    }
                }
                if doc_button(ui, p, DocIcon::AddComment, &widgets::platform_keys("Comment (Ctrl+Alt+M)"), false).clicked() {
                    super::comments::start_comment(tab, actions);
                }
                if doc_button(ui, p, DocIcon::ClearFormat, &widgets::platform_keys("Clear formatting (Ctrl+\\)"), false).clicked() {
                    tab.editor.clear_formatting();
                }
                widgets::separator(ui, p);
                let on = tab.dock == DockTab::Claude;
                if claude_pill(ui, p, on).on_hover_text(widgets::platform_keys("Ask Claude about the selection (Ctrl+J)")).clicked() {
                    handle_ask(tab, Ask::Claude, actions);
                }
            });
        });
    });
}

fn capsule_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.popover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(18))
        .inner_margin(egui::Margin::same(5))
        .shadow(theme::float_shadow(p))
}

fn style_button(ui: &mut Ui, p: &Palette, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(136.0, 32.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(12), p.wash(0x0d));
    }
    ui.painter().text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, label, theme::strong(12.5), p.text);
    icons::draw(ui.painter(), Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), Vec2::splat(11.0)), Icon::ChevronDown, p.text_faint);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// B, I, U and S, drawn as what they do.
fn glyph_button(ui: &mut Ui, p: &Palette, glyph: &str, font: egui::FontId, mark: Marks, on: bool, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    paint_toggle(ui, p, rect, on, response.hovered());
    let colour = if on { p.accent_text } else { p.text };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        glyph,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: colour,
            italics: mark == Marks::ITALIC,
            underline: if mark == Marks::UNDERLINE { Stroke::new(1.2, colour) } else { Stroke::NONE },
            strikethrough: if mark == Marks::STRIKE { Stroke::new(1.2, colour) } else { Stroke::NONE },
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, colour);
    response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn doc_button(ui: &mut Ui, p: &Palette, icon: DocIcon, tip: &str, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    paint_toggle(ui, p, rect, on, response.hovered());
    let colour = if on {
        p.accent_text
    } else if response.hovered() {
        p.text
    } else {
        p.text_muted
    };
    doc_icons::draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(17.0)), icon, colour);
    response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_toggle(ui: &Ui, p: &Palette, rect: Rect, on: bool, hovered: bool) {
    if on {
        ui.painter().rect_filled(rect, CornerRadius::same(12), p.accent_soft);
    } else if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(12), p.wash(0x0d));
    }
}

fn claude_pill(ui: &mut Ui, p: &Palette, on: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap("Ask Claude".into(), theme::strong(12.5), p.text);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 40.0, 32.0), Sense::click());
    let (fill, colour) = if on { (p.accent, p.on_accent) } else { (p.accent_soft, p.accent_text) };
    let fill = if response.hovered() && !on { p.accent_alpha(0x38) } else { fill };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    icons::draw(ui.painter(), Rect::from_center_size(rect.left_center() + vec2(17.0, 0.0), Vec2::splat(15.0)), Icon::Sparkle, colour);
    ui.painter().galley(rect.left_center() + vec2(29.0, -galley.size().y / 2.0), galley, colour);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// --- the count and zoom ---

fn count_bar(ctx: &egui::Context, p: &Palette, tab: &mut DocTab, at: Pos2) {
    let words = survey::stats(&tab.editor.doc).words;
    let selected = if tab.editor.selection().is_caret() { None } else { Some(quark_doc::motion::word_count(&tab.editor.selected_text())) };
    egui::Area::new(Id::new("doc-count-bar")).order(egui::Order::Middle).pivot(Align2::CENTER_BOTTOM).fixed_pos(at).show(ctx, |ui| {
        capsule_frame(p).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(10.0);
                let text = match selected {
                    Some(n) => format!("{} of {} words", group_thousands(n), group_thousands(words)),
                    None => format!("{} words", group_thousands(words)),
                };
                ui.label(egui::RichText::new(text).font(theme::strong(12.5)).color(p.text));
                ui.add_space(8.0);
                widgets::separator(ui, p);
                if widgets::icon_button(ui, p, Icon::Minus, &widgets::platform_keys("Zoom out (Ctrl+-)"), 32.0, false, tab.zoom > 0.5).clicked() {
                    tab.zoom = (tab.zoom - 0.1).max(0.5);
                }
                let pct = format!("{:.0}%", tab.zoom * 100.0);
                if ui.add(egui::Label::new(egui::RichText::new(pct).font(theme::strong(12.5)).color(p.text)).sense(Sense::click())).on_hover_text("Back to 100%").clicked() {
                    tab.zoom = 1.0;
                }
                if widgets::icon_button(ui, p, Icon::Plus, &widgets::platform_keys("Zoom in (Ctrl+=)"), 32.0, false, tab.zoom < 2.5).clicked() {
                    tab.zoom = (tab.zoom + 0.1).min(2.5);
                }
            });
        });
    });
}

// --- the link editor ---

fn link_editor(ctx: &egui::Context, p: &Palette, tab: &mut DocTab) {
    let Some(draft) = &mut tab.link_draft else { return };
    let caret = tab.sheet.caret(tab.editor.selection().head).translate(tab.sheet_origin.to_vec2());
    let mut done: Option<Option<String>> = None;
    let mut cancel = false;
    egui::Area::new(Id::new("doc-link-editor")).order(egui::Order::Foreground).fixed_pos(caret.left_bottom() + vec2(-12.0, 8.0)).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(p.popover).corner_radius(CornerRadius::same(14)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let field = egui::TextEdit::singleline(draft).hint_text("Paste or type a link").desired_width(260.0);
                let r = ui.add(field);
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(Some(draft.clone()));
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                if widgets::primary_button(ui, p, "Apply", !draft.trim().is_empty()).clicked() {
                    done = Some(Some(draft.clone()));
                }
                if widgets::ghost_button(ui, p, "Remove").clicked() {
                    done = Some(None);
                }
            });
        });
    });
    if let Some(url) = done {
        let url = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        let url = url.map(|u| if u.contains("://") || u.starts_with("mailto:") { u } else { format!("https://{u}") });
        tab.editor.set_link(url.as_deref());
        tab.link_draft = None;
        ctx.memory_mut(|m| m.request_focus(paper_id()));
    } else if cancel {
        tab.link_draft = None;
        ctx.memory_mut(|m| m.request_focus(paper_id()));
    }
}

// --- the sheet ---

fn sheet(ui: &mut Ui, p: &Palette, tab: &mut DocTab, focused: bool, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let decor = layout::Decor::of(&tab.editor.doc, tab.active_comment);
    let zoom = tab.zoom;
    let fresh = layout::lay_out(&ctx, &tab.editor.doc, &decor, zoom, &mut tab.cache);
    tab.sheet = fresh;
    let paper_w = layout::PAPER_W * zoom;
    let reveal = std::mem::take(&mut tab.reveal);
    let jump = tab.jump.take();

    egui::ScrollArea::vertical().id_salt(("doc-scroll", tab.path.clone())).auto_shrink([false, false]).show_viewport(ui, |ui, _viewport| {
        let width = ui.available_width().max(paper_w + 48.0);
        let height = TOP_ROOM + tab.sheet.height + BOTTOM_ROOM;
        let (content, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let paper = Rect::from_min_size(pos2(content.left() + (width - paper_w) / 2.0, content.top() + TOP_ROOM), vec2(paper_w, tab.sheet.height));
        let origin = paper.min;
        tab.sheet_origin = origin;
        let response = ui.interact(paper.expand2(vec2((width - paper_w) / 2.0, 0.0)), paper_id(), Sense::click_and_drag());

        let painter = ui.painter();
        let clip = ui.clip_rect();
        painter.add(egui::Shadow { offset: [0, 22], blur: 56, spread: 0, color: p.page_shadow.gamma_multiply(0.8) }.as_shape(paper, CornerRadius::same(8)));
        painter.rect_filled(paper, CornerRadius::same(8), Color32::WHITE);

        let sel = tab.editor.selection();
        let (a, b) = sel.ordered();
        let selection = if sel.is_caret() { Vec::new() } else { tab.sheet.selection(a, b) };
        super::comments::paint_hits(painter, tab, origin);
        for (i, block) in tab.sheet.blocks.iter().enumerate() {
            let band = block.band.translate(origin.to_vec2());
            if !band.intersects(clip) {
                continue;
            }
            if let Some(panel) = block.panel {
                let fill = if matches!(tab.editor.doc.block(i).kind, BlockKind::Code { .. }) { layout::CODE_BG } else { layout::QUOTE_BG };
                painter.rect_filled(panel.translate(origin.to_vec2()), CornerRadius::same((10.0 * zoom) as u8), fill);
            }
            for r in selection.iter().filter(|r| r.intersects(block.band)) {
                painter.rect_filled(r.translate(origin.to_vec2()), CornerRadius::same(2), layout::SELECTION);
            }
            painter.galley(origin + block.origin, block.galley.clone(), layout::INK);
            match &block.marker {
                Marker::None => {}
                Marker::Bullet { centre, depth } => {
                    let c = origin + *centre;
                    let r = 2.7 * zoom;
                    match depth % 3 {
                        0 => painter.circle_filled(c, r, layout::INK),
                        1 => painter.circle_stroke(c, r, Stroke::new(1.2 * zoom, layout::INK)),
                        _ => painter.rect_filled(Rect::from_center_size(c, Vec2::splat(r * 1.8)), CornerRadius::ZERO, layout::INK),
                    };
                }
                Marker::Number { right, text } => {
                    painter.text(origin + *right, Align2::RIGHT_CENTER, text, theme::body(15.5 * zoom), layout::INK_MUTED);
                }
                Marker::Task { square, checked } => {
                    let sq = square.translate(origin.to_vec2());
                    let radius = CornerRadius::same((4.5 * zoom) as u8);
                    if *checked {
                        painter.rect_filled(sq, radius, layout::MARK);
                        let s = sq.width();
                        let pt = |x: f32, y: f32| pos2(sq.left() + x * s, sq.top() + y * s);
                        painter.add(egui::Shape::line(vec![pt(0.26, 0.52), pt(0.43, 0.68), pt(0.75, 0.33)], Stroke::new(2.0 * zoom, Color32::WHITE)));
                    } else {
                        painter.rect_stroke(sq, radius, Stroke::new(1.5 * zoom, layout::INK_FAINT), egui::StrokeKind::Inside);
                    }
                }
                Marker::Rule { y } => {
                    let y = origin.y + y;
                    painter.line_segment([pos2(paper.left() + layout::MARGIN_X * zoom, y), pos2(paper.right() - layout::MARGIN_X * zoom, y)], Stroke::new(1.0, layout::RULE));
                }
            }
        }

        // The caret, solid while in use and blinking when left.
        let caret = tab.sheet.caret(sel.head).translate(origin.to_vec2());
        if focused && sel.is_caret() {
            let since = tab.blink.elapsed().as_secs_f32();
            let on = since < 0.6 || (since % 1.1) < 0.65;
            let mut x = caret.left();
            if !tab.preedit.is_empty() {
                let g = painter.layout_no_wrap(tab.preedit.clone(), theme::body(15.5 * zoom), layout::INK);
                let at = pos2(caret.left(), caret.center().y - g.size().y / 2.0);
                painter.rect_filled(Rect::from_min_size(at, g.size()), CornerRadius::ZERO, Color32::WHITE);
                painter.line_segment([at + vec2(0.0, g.size().y), at + g.size()], Stroke::new(1.0, layout::INK));
                x += g.size().x;
                painter.galley(at, g, layout::INK);
            }
            if on {
                painter.rect_filled(Rect::from_min_max(pos2(x - 0.5 * zoom, caret.top()), pos2(x + 1.5 * zoom, caret.bottom())), CornerRadius::same(1), layout::CARET);
            }
            ctx.request_repaint_after(Duration::from_millis(if since < 0.6 { 600 } else { 300 }));
            // The input method's window goes beside the caret.
            ctx.output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    purpose: Default::default(),
                    rect: paper,
                    cursor_rect: Rect::from_min_size(pos2(x, caret.top()), vec2(1.0, caret.height())),
                    should_interrupt_composition: false,
                })
            });
        }

        if reveal && caret.is_finite() {
            ui.scroll_to_rect(caret.expand2(vec2(0.0, 64.0 * zoom)), None);
        }
        if let Some(block) = jump.and_then(|b| tab.sheet.blocks.get(b)) {
            ui.scroll_to_rect(block.band.translate(origin.to_vec2()).translate(vec2(0.0, -24.0)), Some(egui::Align::TOP));
        }

        mouse(ui, &response, tab, origin, actions);
    });
}

/// Clicks and drags on the sheet.
fn mouse(ui: &Ui, response: &egui::Response, tab: &mut DocTab, origin: Pos2, actions: &mut Vec<Action>) {
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else { return };
    let local = pointer - origin.to_vec2();
    let (mods, pressed) = ui.input(|i| (i.modifiers, i.pointer.primary_pressed()));

    // What is under the pointer: a box to tick, or a link to follow.
    let at = tab.sheet.hit(local);
    let task = tab.sheet.blocks.iter().position(|b| matches!(&b.marker, Marker::Task { square, .. } if square.expand(3.0).contains(local)));
    let link = tab.editor.link_at(at).filter(|_| tab.sheet.caret(at).expand2(vec2(8.0, 2.0)).contains(local)).map(|(_, _, url)| url);
    if response.hovered() {
        let cursor = if task.is_some() || (link.is_some() && mods.command) { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Text };
        ui.ctx().set_cursor_icon(cursor);
        if let Some(url) = &link {
            if !mods.command {
                response.clone().on_hover_text(format!("{url}\n{}", widgets::platform_keys("Ctrl+click to open")));
            }
        }
    }

    if pressed && response.hovered() {
        response.request_focus();
        tab.blink = Instant::now();
        tab.goal_x = None;
        if let Some(i) = task {
            tab.editor.toggle_task(i);
            return;
        }
        if let (Some(url), true) = (&link, mods.command) {
            actions.push(Action::OpenLink(url.clone()));
            return;
        }
        if mods.shift {
            tab.editor.extend_to(at);
        } else {
            tab.editor.caret_to(at);
        }
        tab.drag = Some(Drag { anchor: tab.editor.selection().anchor });
        // The comment under a click comes forward in the dock.
        let style = tab.editor.doc.block(at.block).style_at(at.offset).or_else(|| at.offset.checked_sub(1).and_then(|o| tab.editor.doc.block(at.block).style_at(o)));
        let comment = style.and_then(|s| s.anchors.iter().copied().find(|id| tab.editor.doc.comment(*id).is_some_and(|c| !c.resolved)));
        if comment.is_some() {
            tab.active_comment = comment;
            tab.dock = DockTab::Comments;
        }
    }
    if response.triple_clicked() {
        tab.editor.select_block(at.block);
        tab.drag = None;
    } else if response.double_clicked() {
        tab.editor.select_word_at(at);
        tab.drag = None;
    } else if let Some(drag) = tab.drag {
        if ui.input(|i| i.pointer.primary_down()) {
            if response.dragged() {
                tab.editor.set_selection(Selection::new(drag.anchor, at));
                tab.blink = Instant::now();
                // Selecting past the edge of what shows scrolls with it.
                let clip = ui.clip_rect();
                if pointer.y < clip.top() + 24.0 || pointer.y > clip.bottom() - 24.0 {
                    let caret = tab.sheet.caret(at).translate(origin.to_vec2());
                    ui.scroll_to_rect(caret, None);
                    ui.ctx().request_repaint();
                }
            }
        } else {
            tab.drag = None;
        }
    }
}
