//! Quark's home screen.
//!
//! # Why this is a tab
//!
//! Home is not an empty state. It is a destination that sits first in the tab
//! strip and stays there while documents come and go, so there is always
//! somewhere to go back to — closing the last PDF lands here rather than on a
//! blank window. That is the shape Acrobat uses and the reason its home feels
//! like part of the application rather than a splash screen.
//!
//! # Why the metadata is cached
//!
//! The ambient ground repaints continuously, so anything this screen does runs
//! thirty times a second. Sizing twenty recent files means twenty `stat` calls
//! per frame — six hundred a second to draw a list that changes when the user
//! opens something. So the rows are built once and rebuilt only when the recent
//! list itself changes.

use std::path::PathBuf;

use egui::{Align2, Rect, Sense, Ui, UiBuilder, vec2};
use quark_core::command::Command;
use quark_core::prefs::Prefs;
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

/// Width of the navigation island.
const NAV_WIDTH: f32 = 248.0;
/// Height of one recent-file row.
const ROW_HEIGHT: f32 = 60.0;
/// Widest the content column is allowed to get.
///
/// Without a cap the recent list stretches to the window, which on a wide
/// monitor strands the date and size a thousand pixels from the filename they
/// describe. Every file manager caps this for the same reason.
const MAX_CONTENT: f32 = 1040.0;
/// Where the filename column starts, clearing the row's paper thumbnail.
const NAME_LEFT: f32 = 66.0;
/// Room reserved for the widest column heading ("LAST OPENED" in micro-caps,
/// which is wider than any date that sits under it).
const HEADER_WIDTH: f32 = 88.0;
/// Height of a quick-action card; the width is a quarter of the column.
const CARD_HEIGHT: f32 = 124.0;
/// Height of the drop hint at the foot of the nav.
const DROP_HEIGHT: f32 = 160.0;

/// Which part of the home screen the rail has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Section {
    /// The library: the books in the person's folders.
    #[default]
    Books,
    Recent,
    /// LaTeX projects.
    Latex,
    /// Documents written in Quark.
    Documents,
    Tools,
}

/// What the home screen wants the application to do.
pub(crate) enum Action {
    None,
    Open(PathBuf),
    Run(Command),
    Books(crate::books::BooksAction),
    /// A LaTeX project opened again.
    OpenTex(PathBuf),
    /// Something from the Documents section.
    Docs(crate::docs::home::DocsAction),
}

/// One row of the recent list, with its filesystem facts already resolved.
struct Entry {
    path: PathBuf,
    name: String,
    folder: String,
    /// `None` when the file has been moved or deleted since it was opened.
    size: Option<u64>,
    opened: String,
    page: usize,
}

pub(crate) struct HomeState {
    /// Whether home is the surface currently on screen. Home is always *in*
    /// the tab strip, so this is not "does it exist" but "is it showing".
    pub(crate) visible: bool,
    pub(crate) section: Section,
    /// The library, as last walked.
    pub(crate) books: crate::books::BooksState,
    /// The Documents section: the files listed, and a draft being asked for.
    pub(crate) docs: crate::docs::home::DocsHome,
    entries: Vec<Entry>,
    /// Identifies the recent list the rows were built from, so they are only
    /// rebuilt when it actually changes.
    fingerprint: u64,
}

impl Default for HomeState {
    fn default() -> Self {
        Self {
            // Quark opens on home; a file on the command line replaces it.
            visible: true,
            section: Section::default(),
            books: crate::books::BooksState::default(),
            docs: crate::docs::home::DocsHome::default(),
            entries: Vec::new(),
            fingerprint: 0,
        }
    }
}

/// A cheap order-sensitive hash of the recent list.
///
/// FNV-1a over the paths and their remembered pages. Only used to notice
/// change, so collision resistance is irrelevant.
fn fingerprint(prefs: &Prefs) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for r in &prefs.recent {
        eat(r.path.as_bytes());
        eat(&r.page.to_le_bytes());
        eat(b"\0");
    }
    h
}

/// Renders a byte count the way a file manager would.
pub(crate) fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

/// Renders an RFC 3339 timestamp as a phrase, the way a recent list should.
///
/// Anything older than a week becomes a date: "37 days ago" is a number the
/// reader has to convert, while a date is one they can recognise.
fn relative_time(stamp: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        // A prefs file written by a future version, or hand-edited. Showing
        // the raw value beats showing nothing.
        return stamp.chars().take(10).collect();
    };
    let then = then.with_timezone(&chrono::Local);
    let delta = now.signed_duration_since(then);

    let mins = delta.num_minutes();
    if mins < 1 {
        "Just now".into()
    } else if mins < 60 {
        format!("{mins} min ago")
    } else if delta.num_hours() < 24 {
        let h = delta.num_hours();
        format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
    } else if delta.num_days() < 7 {
        let d = delta.num_days();
        format!("{d} day{} ago", if d == 1 { "" } else { "s" })
    } else {
        then.format("%e %b %Y").to_string().trim().to_string()
    }
}

impl HomeState {
    /// Rebuilds the rows if the recent list has changed since they were built.
    fn sync(&mut self, prefs: &Prefs) {
        let fp = fingerprint(prefs);
        if fp == self.fingerprint && !self.entries.is_empty() {
            return;
        }
        self.fingerprint = fp;
        let now = chrono::Local::now();
        self.entries = prefs
            .recent
            .iter()
            .map(|r| {
                let path = PathBuf::from(&r.path);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| r.path.clone());
                let folder = path
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // One stat per row, at rebuild time rather than per frame. It
                // doubles as the existence check: a file that cannot be sized
                // cannot be opened either.
                let size = std::fs::metadata(&path).ok().map(|m| m.len());
                Entry {
                    path,
                    name,
                    folder,
                    size,
                    opened: relative_time(&r.opened, now),
                    page: r.page,
                }
            })
            .collect();
    }
}

/// The actions offered on home, all of which work with no document open.
///
/// Deliberately not the whole tool list: a card that needs a document is a card
/// that does nothing when clicked, which is worse than not offering it. These
/// are exactly the commands `App::command_enabled` permits with no document.
const QUICK_ACTIONS: [(Icon, &str, &str, Command); 4] = [
    (Icon::Open, "Open a file", "Browse for a PDF", Command::Open),
    (
        Icon::Plus,
        "Blank document",
        "Start from an empty page",
        Command::NewFromBlank,
    ),
    (
        Icon::Image,
        "From images",
        "Build a PDF from pictures",
        Command::NewFromImages,
    ),
    (
        Icon::Layers,
        "Combine files",
        "Merge several PDFs into one",
        Command::CombineFiles,
    ),
];

/// Draws the home screen and reports what the user asked for.
///
/// Two islands side by side, the same shape as the document view: the
/// sections on the left, the selected section on the right.
pub(crate) fn show(ui: &mut Ui, p: &Palette, state: &mut HomeState, prefs: &Prefs) -> Action {
    state.sync(prefs);

    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let nav_rect = Rect::from_min_size(full.min, vec2(NAV_WIDTH, full.height()));
    let body_rect = Rect::from_min_max(
        egui::pos2(nav_rect.right() + theme::GAP, full.top()),
        full.max,
    );

    // Painted through the layer's unclipped painter rather than as frames:
    // the central panel clips to its rect, which would cut the islands'
    // shadows off a few pixels below them.
    let painter = ui.ctx().layer_painter(ui.layer_id());
    theme::paint_island(&painter, nav_rect, p);
    theme::paint_island(&painter, body_rect, p);

    let mut nav_ui = ui.new_child(UiBuilder::new().max_rect(nav_rect.shrink2(vec2(10.0, 14.0))));
    nav(&mut nav_ui, p, state);

    let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body_rect.shrink2(vec2(44.0, 36.0))));
    body(&mut body_ui, p, state, prefs)
}

/// The sections, and the drop hint at the foot.
fn nav(ui: &mut Ui, p: &Palette, state: &mut HomeState) {
    let height = ui.available_height();
    let top = ui.cursor().top();
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(widgets::micro_caps("Library"))
                .font(theme::strong(10.5))
                .color(p.text_faint),
        );
    });
    ui.add_space(4.0);

    let recent = state.entries.len();
    let books = state.books.books.len();
    for (section, label, icon, count) in [
        (Section::Books, "Books", Icon::Bookmark, Some(books).filter(|n| *n > 0)),
        (Section::Recent, "Recent", Icon::Clock, Some(recent).filter(|n| *n > 0)),
        (Section::Documents, "Documents", Icon::Document, None),
        (Section::Latex, "LaTeX", Icon::Text, None),
        (Section::Tools, "Tools", Icon::Grid, None),
    ] {
        if widgets::nav_item(ui, p, icon, label, state.section == section, count).clicked() {
            state.section = section;
        }
    }

    // The hint sits at the bottom, out of the way of the list but always
    // visible — dropping a file is the fastest way in and is otherwise
    // undiscoverable.
    let used = ui.cursor().top() - top;
    ui.add_space((height - used - DROP_HEIGHT).max(8.0));
    drop_zone(ui, p);
}

/// The dashed well that says a PDF can be dropped anywhere.
fn drop_zone(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), DROP_HEIGHT), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let radius = theme::RADIUS_CARD as f32 + 2.0;
    painter.rect_filled(rect, egui::CornerRadius::same(radius as u8), p.recess);
    painter.extend(egui::Shape::dashed_line(
        &rounded_outline(rect.shrink(0.75), radius),
        egui::Stroke::new(1.5, p.wash(0x22)),
        6.0,
        5.0,
    ));

    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(12.0, 20.0)))
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let (plate, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
            let painter = ui.painter();
            painter.circle_filled(plate.center(), 23.0, p.accent_soft);
            icons::draw(
                painter,
                Rect::from_center_size(plate.center(), vec2(20.0, 20.0)),
                Icon::Upload,
                p.accent_text,
            );
            ui.label(
                egui::RichText::new("Drop a PDF or a .md anywhere")
                    .font(theme::strong(13.0))
                    .color(p.text),
            );
            ui.horizontal(|ui| {
                // Centre the row by hand: a horizontal layout inside a
                // centred column still packs from the left.
                let width = 108.0;
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                ui.label(egui::RichText::new("or press").font(theme::body(11.5)).color(p.text_muted));
                widgets::shortcut(ui, p, "Ctrl+O");
            });
        },
    );
}

/// Points round a rounded rectangle, for a dashed outline.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let centres = [
        (egui::pos2(rect.right() - r, rect.top() + r), -FRAC_PI_2),
        (egui::pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (egui::pos2(rect.left() + r, rect.bottom() - r), FRAC_PI_2),
        (egui::pos2(rect.left() + r, rect.top() + r), PI),
    ];
    let mut pts = vec![egui::pos2(rect.left() + r, rect.top())];
    for (c, start) in centres {
        for i in 0..=8 {
            let a = start + FRAC_PI_2 * (i as f32 / 8.0);
            pts.push(c + vec2(a.cos(), a.sin()) * r);
        }
    }
    pts.push(egui::pos2(rect.left() + r, rect.top()));
    pts
}

/// The main column: heading, quick actions, then the selected section.
fn body(ui: &mut Ui, p: &Palette, state: &mut HomeState, prefs: &Prefs) -> Action {
    let mut action = Action::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Caps the whole column — heading, cards and list — in one place,
            // so they stay aligned with each other on any window width.
            ui.set_max_width(MAX_CONTENT.min(ui.available_width()));
            let (title, subtitle) = match state.section {
                Section::Books => ("Books", "Every PDF in your folders, shelved by the folder it is in."),
                Section::Latex => ("LaTeX", "A folder of sources, edited with Vim's keys."),
                Section::Documents => ("Documents", "Write, comment and ask Claude. Each one is a Markdown file any editor can open."),
                Section::Recent => ("Recent files", "Pick up where you left off, or start something new."),
                Section::Tools => ("Tools", "Start a new document here, or open one to edit, export and sign it."),
            };
            ui.label(egui::RichText::new(title).font(theme::heavy(28.0)).color(p.text));
            ui.add_space(2.0);
            ui.label(egui::RichText::new(subtitle).font(theme::body(13.5)).color(p.text_muted));
            ui.add_space(24.0);

            if state.section == Section::Documents {
                if let Some(a) = crate::docs::home::show(ui, p, &mut state.docs, prefs) {
                    action = Action::Docs(a);
                }
                ui.add_space(20.0);
                return;
            }
            if state.section == Section::Latex {
                action = latex_projects(ui, p, prefs);
                ui.add_space(20.0);
                return;
            }
            // The library is for reading: its shelves, not the tools.
            if state.section == Section::Books {
                action = match crate::books::show(ui, p, &mut state.books) {
                    crate::books::BooksAction::None => Action::None,
                    crate::books::BooksAction::Open(path) => Action::Open(path),
                    other => Action::Books(other),
                };
                ui.add_space(20.0);
                return;
            }

            let gap = 14.0;
            let width = ((ui.available_width() - gap * 3.0) / 4.0).floor().max(150.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                for (icon, title, subtitle, command) in QUICK_ACTIONS {
                    if card(ui, p, icon, title, subtitle, width).clicked() {
                        action = Action::Run(command);
                    }
                }
            });
            ui.add_space(28.0);

            match state.section {
                Section::Recent => {
                    if let Some(a) = recent_list(ui, p, state) {
                        action = a;
                    }
                }
                Section::Tools => tools_note(ui, p),
                // Drawn above, without the cards.
                Section::Books | Section::Latex | Section::Documents => {}
            }
            ui.add_space(20.0);
        });
    action
}

/// The LaTeX projects opened before, and a way to open another.
fn latex_projects(ui: &mut Ui, p: &Palette, prefs: &Prefs) -> Action {
    let mut action = Action::None;
    ui.horizontal(|ui| {
        if widgets::primary_button(ui, p, "New project…", true).clicked() {
            action = Action::Run(Command::NewTexProject);
        }
        if widgets::ghost_button(ui, p, "Open a project…").clicked() {
            action = Action::Run(Command::OpenTexProject);
        }
    });
    ui.add_space(20.0);
    if prefs.tex_projects.is_empty() {
        ui.label(
            egui::RichText::new("No projects yet. A project is a folder with a .tex file in it.")
                .font(theme::body(12.5))
                .color(p.text_muted),
        );
        return action;
    }
    for typed in &prefs.tex_projects {
        let path = PathBuf::from(typed);
        let name = path.file_name().map_or_else(|| typed.clone(), |n| n.to_string_lossy().into_owned());
        let folder = path.parent().map(|d| quark_pdf::library::tilde(d)).unwrap_or_default();
        let there = path.is_dir();
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width().min(640.0), 48.0), Sense::click());
        if response.hovered() && there {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(10), p.wash(0x0a));
        }
        ui.painter().text(rect.left_top() + vec2(14.0, 9.0), Align2::LEFT_TOP, name, theme::strong(13.5), if there { p.text } else { p.text_faint });
        ui.painter().text(
            rect.left_top() + vec2(14.0, 28.0),
            Align2::LEFT_TOP,
            if there { folder } else { format!("{folder} — not there now") },
            theme::body(11.5),
            p.text_faint,
        );
        if there && response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = Action::OpenTex(path);
        }
    }
    action
}

/// A quick-action card.
fn card(ui: &mut Ui, p: &Palette, icon: Icon, title: &str, subtitle: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(18);
        let hovered = response.hovered();
        if hovered {
            painter.add(theme::island_shadow(p).as_shape(rect, radius));
        }
        painter.rect_filled(rect, radius, p.wash(if hovered { 0x12 } else { 0x08 }));
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, if hovered { p.accent_alpha(0x80) } else { p.wash(0x0c) }),
            egui::StrokeKind::Inside,
        );
        theme::paint_sheen(painter, rect, 18.0, p);

        let plate = Rect::from_min_size(rect.left_top() + vec2(18.0, 18.0), vec2(40.0, 40.0));
        painter.rect_filled(plate, egui::CornerRadius::same(13), p.accent_soft);
        icons::draw(
            painter,
            Rect::from_center_size(plate.center(), vec2(19.0, 19.0)),
            icon,
            p.accent_text,
        );
        painter.text(
            rect.left_bottom() + vec2(18.0, -38.0),
            Align2::LEFT_TOP,
            title,
            theme::strong(13.5),
            p.text,
        );
        painter.text(
            rect.left_bottom() + vec2(18.0, -19.0),
            Align2::LEFT_TOP,
            subtitle,
            theme::body(12.0),
            p.text_muted,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The recent list, as a table with the columns a file list needs.
fn recent_list(ui: &mut Ui, p: &Palette, state: &HomeState) -> Option<Action> {
    if state.entries.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Nothing opened yet. Files you open will be listed here.")
                .font(theme::body(12.5))
                .color(p.text_muted),
        );
        return None;
    }

    let width = ui.available_width().max(420.0);
    // Both metadata columns are right-aligned on an anchor measured in from
    // the right edge, and the heading uses the same anchor as its values — the
    // two must not drift apart or "LAST OPENED" runs into "SIZE".
    let size_right = width - 16.0;
    let opened_right = size_right - 110.0;
    // The headings are wider than the values they sit over, so the name has to
    // clear the widest of the two.
    let name_width = opened_right - HEADER_WIDTH - NAME_LEFT;

    header_row(ui, p, width, opened_right, size_right);
    ui.spacing_mut().item_spacing.y = 2.0;

    let mut action = None;
    for entry in &state.entries {
        let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
        let missing = entry.size.is_none();
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if response.hovered() && !missing {
                painter.rect_filled(rect, egui::CornerRadius::same(theme::RADIUS_CARD), p.wash(0x0d));
            }

            paper_thumb(painter, p, rect.left_top() + vec2(16.0, 9.0), missing);

            let text_colour = if missing { p.text_muted } else { p.text };
            painter.text(
                rect.left_top() + vec2(NAME_LEFT, 12.0),
                Align2::LEFT_TOP,
                widgets::elide(&entry.name, (name_width / 7.5) as usize, true),
                theme::strong(13.5),
                text_colour,
            );
            // The folder is the disambiguator: two documents called
            // "invoice.pdf" are only told apart by where they live.
            let subtitle = if missing {
                "Moved or deleted".to_string()
            } else if entry.page > 0 {
                format!("{}  ·  page {}", entry.folder, entry.page + 1)
            } else {
                entry.folder.clone()
            };
            painter.text(
                rect.left_top() + vec2(NAME_LEFT, 32.0),
                Align2::LEFT_TOP,
                widgets::elide(&subtitle, (name_width / 6.0) as usize, false),
                theme::body(11.5),
                if missing { p.warning } else { p.text_faint },
            );

            painter.text(
                egui::pos2(rect.left() + opened_right, rect.center().y),
                Align2::RIGHT_CENTER,
                &entry.opened,
                theme::body(12.5),
                if missing { p.text_faint } else { p.text_muted },
            );
            painter.text(
                egui::pos2(rect.left() + size_right, rect.center().y),
                Align2::RIGHT_CENTER,
                entry.size.map(human_size).unwrap_or_else(|| "—".into()),
                theme::body(12.5),
                if missing { p.text_faint } else { p.text_muted },
            );
        }

        // A file that is gone is shown but not offered: a click that silently
        // does nothing is worse than a row that explains itself.
        if !missing {
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                action = Some(Action::Open(entry.path.clone()));
            }
        }
    }
    action
}

/// A tiny sheet of paper standing in for a document in the recent list.
fn paper_thumb(painter: &egui::Painter, p: &Palette, at: egui::Pos2, missing: bool) {
    let rect = Rect::from_min_size(at, vec2(34.0, 42.0));
    let radius = egui::CornerRadius::same(7);
    if missing {
        painter.extend(egui::Shape::dashed_line(
            &rounded_outline(rect, 7.0),
            egui::Stroke::new(1.5, p.wash(0x28)),
            4.0,
            3.0,
        ));
        icons::draw(
            painter,
            Rect::from_center_size(rect.center(), vec2(16.0, 16.0)),
            Icon::Warning,
            p.text_faint,
        );
        return;
    }
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 4],
            blur: 10,
            spread: 0,
            color: p.page_shadow.gamma_multiply(0.6),
        }
        .as_shape(rect, radius),
    );
    painter.rect_filled(rect, radius, egui::Color32::WHITE);
    let ink = egui::Color32::from_rgb(0x2c, 0x28, 0x34);
    let rule = egui::Color32::from_rgb(0xd6, 0xd2, 0xdd);
    let bar = |y: f32, w: f32, h: f32, c: egui::Color32| {
        painter.rect_filled(
            Rect::from_min_size(rect.left_top() + vec2(6.0, y), vec2((rect.width() - 12.0) * w, h)),
            egui::CornerRadius::same(1),
            c,
        );
    };
    bar(7.0, 0.7, 3.0, ink);
    bar(13.0, 1.0, 2.0, rule);
    bar(18.0, 1.0, 2.0, p.accent_alpha(0x70));
    bar(23.0, 0.8, 2.0, rule);
    bar(28.0, 1.0, 2.0, rule);
}

/// The column headings above the recent list.
fn header_row(ui: &mut Ui, p: &Palette, width: f32, opened_right: f32, size_right: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    for (x, label, align) in [
        (NAME_LEFT, "Name", Align2::LEFT_CENTER),
        (opened_right, "Last opened", Align2::RIGHT_CENTER),
        (size_right, "Size", Align2::RIGHT_CENTER),
    ] {
        painter.text(
            egui::pos2(rect.left() + x, rect.center().y),
            align,
            widgets::micro_caps(label),
            theme::strong(10.5),
            p.text_faint,
        );
    }
}

/// The Tools section.
///
/// Everything beyond the quick actions above needs a document, so rather than
/// showing a wall of cards that do nothing, this says where they live.
fn tools_note(ui: &mut Ui, p: &Palette) {
    ui.label(
        egui::RichText::new(
            "Editing, export, redaction, forms and signing all act on an open \
             document — open one and they appear in the tool dock and the menus.",
        )
        .font(theme::body(12.5))
        .color(p.text_muted),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        widgets::shortcut(ui, p, "Ctrl+Shift+P");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("lists every command Quark has.")
                .font(theme::body(12.5))
                .color(p.text_faint),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn prefs_with(paths: &[&str]) -> Prefs {
        let mut p = Prefs::default();
        for path in paths {
            p.push_recent(path, 0);
        }
        p
    }

    #[test]
    fn home_is_what_quark_opens_on() {
        // Closing the last document has to land somewhere, and a blank window
        // is not somewhere.
        assert!(HomeState::default().visible);
    }

    #[test]
    fn the_rows_are_not_rebuilt_when_the_recent_list_is_unchanged() {
        // This is the whole reason the cache exists: the ambient ground
        // repaints 30 times a second and each rebuild is one stat per row.
        let prefs = prefs_with(&["/a.pdf", "/b.pdf"]);
        let mut state = HomeState::default();
        state.sync(&prefs);
        let first = state.fingerprint;
        state.sync(&prefs);
        assert_eq!(state.fingerprint, first);
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn opening_something_new_rebuilds_the_rows() {
        let mut prefs = prefs_with(&["/a.pdf"]);
        let mut state = HomeState::default();
        state.sync(&prefs);
        let before = state.fingerprint;
        prefs.push_recent("/b.pdf", 0);
        state.sync(&prefs);
        assert_ne!(state.fingerprint, before);
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn reopening_the_same_file_at_a_new_page_rebuilds_the_rows() {
        // The page is shown in the row, so a change to it has to invalidate
        // even though the path list is identical.
        let mut prefs = prefs_with(&["/a.pdf"]);
        let mut state = HomeState::default();
        state.sync(&prefs);
        let before = state.fingerprint;
        prefs.push_recent("/a.pdf", 7);
        state.sync(&prefs);
        assert_ne!(state.fingerprint, before);
    }

    #[test]
    fn a_missing_file_has_no_size() {
        let prefs = prefs_with(&["/definitely/not/here.pdf"]);
        let mut state = HomeState::default();
        state.sync(&prefs);
        assert!(state.entries[0].size.is_none());
    }

    #[test]
    fn the_row_keeps_the_folder_so_two_invoices_can_be_told_apart() {
        let prefs = prefs_with(&["/one/invoice.pdf", "/two/invoice.pdf"]);
        let mut state = HomeState::default();
        state.sync(&prefs);
        assert_eq!(state.entries[0].name, "invoice.pdf");
        assert_eq!(state.entries[1].name, "invoice.pdf");
        assert_ne!(state.entries[0].folder, state.entries[1].folder);
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_writes_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn recent_timestamps_become_phrases() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .unwrap();
        let at = |h: i64, m: i64| {
            (now - chrono::Duration::hours(h) - chrono::Duration::minutes(m)).to_rfc3339()
        };

        assert_eq!(relative_time(&at(0, 0), now), "Just now");
        assert_eq!(relative_time(&at(0, 5), now), "5 min ago");
        assert_eq!(relative_time(&at(1, 0), now), "1 hour ago");
        assert_eq!(relative_time(&at(5, 0), now), "5 hours ago");
        assert_eq!(relative_time(&at(24, 0), now), "1 day ago");
        assert_eq!(relative_time(&at(72, 0), now), "3 days ago");
    }

    #[test]
    fn an_old_timestamp_becomes_a_date_rather_than_a_day_count() {
        // "412 days ago" is a number the reader has to convert.
        let now = chrono::Local
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .unwrap();
        let then = (now - chrono::Duration::days(40)).to_rfc3339();
        let out = relative_time(&then, now);
        assert!(out.contains("2026"), "expected a date, got {out}");
        assert!(!out.contains("ago"), "expected a date, got {out}");
    }

    #[test]
    fn an_unparseable_timestamp_does_not_panic() {
        // Prefs are a file on disk and can be edited by hand.
        let now = chrono::Local
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .unwrap();
        assert_eq!(relative_time("", now), "");
        assert_eq!(relative_time("not a date at all", now), "not a date");
    }

    #[test]
    fn cycling_the_strip_wraps_through_home() {
        // Home is slot 0, so N documents make N+1 slots. Getting this wrong
        // either skips home or lands on a document index that does not exist.
        fn next(slot: usize, docs: usize) -> usize {
            (slot + 1) % (docs + 1)
        }
        fn prev(slot: usize, docs: usize) -> usize {
            let slots = docs + 1;
            (slot + slots - 1) % slots
        }

        // Two documents: home, doc 0, doc 1, back to home.
        assert_eq!(next(0, 2), 1);
        assert_eq!(next(1, 2), 2);
        assert_eq!(next(2, 2), 0);
        assert_eq!(prev(0, 2), 2);

        // With nothing open there is only home, and cycling stays on it
        // rather than dividing by zero.
        assert_eq!(next(0, 0), 0);
        assert_eq!(prev(0, 0), 0);
    }

    #[test]
    fn every_quick_action_works_with_no_document_open() {
        // A card that needs a document does nothing when clicked, which is
        // worse than not offering it. Mirrors the empty-state set in
        // `App::command_enabled`.
        for (_, _, _, command) in QUICK_ACTIONS {
            assert!(
                matches!(
                    command,
                    Command::Open
                        | Command::NewFromBlank
                        | Command::NewFromImages
                        | Command::CombineFiles
                ),
                "{command:?} needs a document"
            );
        }
    }
}
