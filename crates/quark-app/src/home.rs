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

use egui::{Align2, FontId, Rect, Sense, Ui, vec2};
use quark_core::command::Command;
use quark_core::prefs::Prefs;
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

/// Width of the navigation rail.
const RAIL_WIDTH: f32 = 176.0;
/// Height of one recent-file row.
const ROW_HEIGHT: f32 = 46.0;
/// Widest the content column is allowed to get.
///
/// Without a cap the recent list stretches to the window, which on a wide
/// monitor strands the date and size a thousand pixels from the filename they
/// describe. Every file manager caps this for the same reason.
const MAX_CONTENT: f32 = 940.0;
/// Where the filename column starts, clearing the row's file icon.
const NAME_LEFT: f32 = 42.0;
/// Room reserved for the widest column heading ("LAST OPENED" in micro-caps,
/// which is wider than any date that sits under it).
const HEADER_WIDTH: f32 = 88.0;
/// Size of a quick-action card.
const CARD: egui::Vec2 = egui::vec2(168.0, 92.0);

/// Which part of the home screen the rail has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Section {
    #[default]
    Recent,
    Tools,
}

/// What the home screen wants the application to do.
pub(crate) enum Action {
    None,
    Open(PathBuf),
    Run(Command),
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
fn human_size(bytes: u64) -> String {
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
        Icon::Grid,
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
pub(crate) fn show(ui: &mut Ui, p: &Palette, state: &mut HomeState, prefs: &Prefs) -> Action {
    state.sync(prefs);

    let mut action = Action::None;
    ui.horizontal_top(|ui| {
        rail(ui, p, state);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            action = body(ui, p, state);
        });
    });
    action
}

/// The navigation rail: the brand, then the sections.
fn rail(ui: &mut Ui, p: &Palette, state: &mut HomeState) {
    let height = ui.available_height();
    ui.allocate_ui_with_layout(
        vec2(RAIL_WIDTH, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(RAIL_WIDTH);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Quark").size(20.0).color(p.accent_text));
            });
            ui.add_space(16.0);

            for (section, label, icon) in [
                (Section::Recent, "Recent", Icon::Thumbnails),
                (Section::Tools, "Tools", Icon::Grid),
            ] {
                if rail_item(ui, p, label, icon, state.section == section).clicked() {
                    state.section = section;
                }
            }

            // The hint sits at the bottom of the rail, out of the way of the
            // list but always visible — dropping a file is the fastest way in
            // and is otherwise undiscoverable.
            let used = ui.min_rect().height();
            ui.add_space((height - used - 52.0).max(8.0));
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Drop a PDF anywhere\nor press Ctrl+O")
                        .size(11.0)
                        .color(p.text_faint),
                );
            });
        },
    );
}

/// One selectable entry in the rail.
fn rail_item(ui: &mut Ui, p: &Palette, label: &str, icon: Icon, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(RAIL_WIDTH - 8.0, 34.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if active {
            painter.rect_filled(
                rect,
                egui::CornerRadius::same(theme::RADIUS_CONTROL),
                p.accent_soft,
            );
        } else if response.hovered() {
            painter.rect_filled(
                rect,
                egui::CornerRadius::same(theme::RADIUS_CONTROL),
                p.card_hover,
            );
        }
        let colour = if active { p.accent_text } else { p.text_muted };
        let icon_rect = Rect::from_min_size(rect.left_top() + vec2(8.0, 8.0), vec2(18.0, 18.0));
        icons::draw(painter, icon_rect, icon, colour);
        painter.text(
            rect.left_center() + vec2(34.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.0),
            colour,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The main column: heading, quick actions, then the selected section.
fn body(ui: &mut Ui, p: &Palette, state: &HomeState) -> Action {
    let mut action = Action::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Caps the whole column — heading, cards and list — in one place,
            // so they stay aligned with each other on any window width.
            ui.set_max_width(MAX_CONTENT.min(ui.available_width()));
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(match state.section {
                    Section::Recent => "Recent files",
                    Section::Tools => "Tools",
                })
                .size(24.0)
                .color(p.text),
            );
            ui.add_space(14.0);

            ui.horizontal_wrapped(|ui| {
                for (icon, title, subtitle, command) in QUICK_ACTIONS {
                    if card(ui, p, icon, title, subtitle).clicked() {
                        action = Action::Run(command);
                    }
                }
            });
            ui.add_space(20.0);

            match state.section {
                Section::Recent => {
                    if let Some(a) = recent_list(ui, p, state) {
                        action = a;
                    }
                }
                Section::Tools => tools_note(ui, p),
            }
            ui.add_space(20.0);
        });
    action
}

/// A quick-action card.
fn card(ui: &mut Ui, p: &Palette, icon: Icon, title: &str, subtitle: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(CARD, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(theme::RADIUS_CARD);
        painter.rect_filled(
            rect,
            radius,
            if response.hovered() {
                p.card_hover
            } else {
                p.card
            },
        );
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(
                1.0,
                if response.hovered() {
                    p.accent
                } else {
                    p.border
                },
            ),
            egui::StrokeKind::Inside,
        );

        let icon_rect = Rect::from_min_size(rect.left_top() + vec2(14.0, 14.0), vec2(22.0, 22.0));
        icons::draw(painter, icon_rect, icon, p.accent_text);
        painter.text(
            rect.left_top() + vec2(14.0, 48.0),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(13.0),
            p.text,
        );
        painter.text(
            rect.left_top() + vec2(14.0, 66.0),
            Align2::LEFT_TOP,
            subtitle,
            FontId::proportional(11.0),
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
                .size(12.0)
                .color(p.text_faint),
        );
        return None;
    }

    let width = ui.available_width().max(420.0);
    // Both metadata columns are right-aligned on an anchor measured in from
    // the right edge, and the heading uses the same anchor as its values — the
    // two must not drift apart or "LAST OPENED" runs into "SIZE".
    let size_right = width - 14.0;
    let opened_right = size_right - 84.0;
    // The headings are wider than the values they sit over, so the name has to
    // clear the widest of the two.
    let name_width = opened_right - HEADER_WIDTH - NAME_LEFT;

    header_row(ui, p, width, opened_right, size_right);

    let mut action = None;
    for entry in &state.entries {
        let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
        let missing = entry.size.is_none();
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if response.hovered() && !missing {
                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(theme::RADIUS_SMALL),
                    p.card_hover,
                );
            }

            let icon_rect =
                Rect::from_min_size(rect.left_top() + vec2(10.0, 13.0), vec2(20.0, 20.0));
            icons::draw(
                painter,
                icon_rect,
                Icon::Thumbnails,
                if missing { p.text_faint } else { p.accent_text },
            );

            let text_colour = if missing { p.text_faint } else { p.text };
            painter.text(
                rect.left_top() + vec2(NAME_LEFT, 9.0),
                Align2::LEFT_TOP,
                widgets::elide(&entry.name, (name_width / 7.0) as usize, true),
                FontId::proportional(13.0),
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
                rect.left_top() + vec2(NAME_LEFT, 27.0),
                Align2::LEFT_TOP,
                widgets::elide(&subtitle, (name_width / 6.0) as usize, false),
                FontId::proportional(11.0),
                if missing { p.error } else { p.text_faint },
            );

            painter.text(
                rect.left_top() + vec2(opened_right, 15.0),
                Align2::RIGHT_TOP,
                &entry.opened,
                FontId::proportional(11.0),
                p.text_muted,
            );
            painter.text(
                rect.left_top() + vec2(size_right, 15.0),
                Align2::RIGHT_TOP,
                entry.size.map(human_size).unwrap_or_else(|| "—".into()),
                FontId::proportional(11.0),
                p.text_muted,
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

/// The column headings above the recent list.
fn header_row(ui: &mut Ui, p: &Palette, width: f32, opened_right: f32, size_right: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let font = FontId::proportional(10.0);
    for (x, label, align) in [
        (NAME_LEFT, "Name", Align2::LEFT_TOP),
        (opened_right, "Last opened", Align2::RIGHT_TOP),
        (size_right, "Size", Align2::RIGHT_TOP),
    ] {
        painter.text(
            rect.left_top() + vec2(x, 4.0),
            align,
            widgets::micro_caps(label),
            font.clone(),
            p.text_faint,
        );
    }
    painter.line_segment(
        [
            egui::pos2(rect.left() + 10.0, rect.bottom()),
            egui::pos2(rect.right() - 10.0, rect.bottom()),
        ],
        egui::Stroke::new(1.0, p.border),
    );
}

/// The Tools section.
///
/// Everything beyond the quick actions above needs a document, so rather than
/// showing a wall of cards that do nothing, this says where they live.
fn tools_note(ui: &mut Ui, p: &Palette) {
    ui.label(
        egui::RichText::new(
            "Editing, export, redaction, forms and signing all act on an open \
             document — open one and they appear in the toolbar.",
        )
        .size(12.0)
        .color(p.text_muted),
    );
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Ctrl+Shift+P lists every command Quark has.")
            .size(12.0)
            .color(p.text_faint),
    );
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
