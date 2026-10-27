//! Home's Documents section: ways to start one, and the documents there are.
//!
//! The list is the Markdown files in the documents folder, with any opened
//! from elsewhere since, the last edited first. Each card's thumbnail is the
//! shape of the document's first lines — a title bar, a bulleted list, a
//! checklist — read from the file, and kept until the file changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Ui, Vec2, pos2, vec2};
use quark_core::prefs::Prefs;
use quark_doc::{BlockKind, markdown};
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

use super::icons::{self as doc_icons, DocIcon};
use crate::app::App;

/// How often the folder is looked at again while home shows.
const RESCAN: Duration = Duration::from_secs(3);
const CARD_W: f32 = 196.0;
const THUMB_H: f32 = 168.0;

/// What the section asks the application to do.
pub(crate) enum DocsAction {
    New,
    Draft(String),
    Template(&'static str),
    Open(PathBuf),
    OpenDialog,
    Reveal(PathBuf),
}

/// One line of a thumbnail: what kind of block, and how much of the width
/// its text fills.
#[derive(Clone, Copy)]
enum Line {
    Title(f32),
    Heading(f32),
    Text(f32),
    Bullet(f32),
    Task(bool, f32),
    Panel,
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    title: String,
    modified: SystemTime,
    lines: Vec<Line>,
    comments: usize,
}

#[derive(Default)]
pub(crate) struct DocsHome {
    entries: Vec<Entry>,
    scanned: Option<Instant>,
    /// Thumbnails by file and modification time.
    previews: HashMap<PathBuf, (SystemTime, Vec<Line>, usize)>,
    /// The "Draft with Claude" prompt, while it is being written.
    draft: Option<String>,
}

const TEMPLATES: [(&str, &str); 4] = [
    ("Meeting notes", "# Meeting notes\n\n*Date · Attendees*\n\n## Agenda\n\n1. \n\n## Notes\n\n## Decisions\n\n## Actions\n\n- [ ] \n"),
    ("Project brief", "# Project brief\n\n## Problem\n\n## Goals\n\n- \n\n## Not in scope\n\n- \n\n## Plan\n\n## Open questions\n"),
    ("Weekly plan", "# This week\n\n## Must do\n\n- [ ] \n\n## Should do\n\n- [ ] \n\n## Notes\n"),
    ("Letter", "Dear ,\n\n\n\nKind regards,\n\n"),
];

impl DocsHome {
    /// Lists the folder and the recent documents again, if it is time.
    fn sync(&mut self, prefs: &Prefs) {
        if self.scanned.is_some_and(|t| t.elapsed() < RESCAN) {
            return;
        }
        self.scanned = Some(Instant::now());
        let folder = super::expand(&prefs.docs_folder);
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map(|dir| dir.flatten().map(|e| e.path()).filter(|p| p.is_file() && super::is_document(p) && !hidden(p)).collect())
            .unwrap_or_default();
        for typed in &prefs.docs_recent {
            let p = PathBuf::from(typed);
            if p.is_file() && !paths.contains(&p) {
                paths.push(p);
            }
        }
        let mut entries: Vec<Entry> = paths
            .into_iter()
            .filter_map(|path| {
                let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
                let (lines, comments) = match self.previews.get(&path) {
                    Some((when, lines, comments)) if *when == modified => (lines.clone(), *comments),
                    _ => {
                        let (lines, comments) = preview(&path);
                        self.previews.insert(path.clone(), (modified, lines.clone(), comments));
                        (lines, comments)
                    }
                };
                let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                Some(Entry { path, title, modified, lines, comments })
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
        entries.truncate(60);
        self.entries = entries;
    }

    pub(crate) fn rescan(&mut self) {
        self.scanned = None;
    }
}

fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// The shape of a document's first lines, and how many comments are open.
fn preview(path: &Path) -> (Vec<Line>, usize) {
    let Ok(text) = std::fs::read_to_string(path) else { return (Vec::new(), 0) };
    let doc = markdown::read(&text);
    let comments = doc.comments.iter().filter(|c| !c.resolved).count();
    let width = |s: &str, per_line: f32| (s.chars().count() as f32 / per_line).clamp(0.25, 1.0);
    let mut lines = Vec::new();
    for b in doc.blocks.iter().take(12) {
        let t = b.text();
        match &b.kind {
            BlockKind::Heading(1) => lines.push(Line::Title(width(t, 22.0))),
            BlockKind::Heading(_) => lines.push(Line::Heading(width(t, 30.0))),
            BlockKind::Bullet { .. } | BlockKind::Numbered { .. } => lines.push(Line::Bullet(width(t, 40.0))),
            BlockKind::Task { checked, .. } => lines.push(Line::Task(*checked, width(t, 40.0))),
            BlockKind::Quote | BlockKind::Code { .. } => lines.push(Line::Panel),
            BlockKind::Rule => {}
            BlockKind::Paragraph if t.trim().is_empty() => {}
            BlockKind::Paragraph => {
                // A paragraph is as many lines as it would wrap to, the last short.
                let n = (t.chars().count() as f32 / 48.0).ceil().clamp(1.0, 3.0) as usize;
                for i in 0..n {
                    lines.push(Line::Text(if i + 1 == n { width(t, 48.0).max(0.4) * 0.8 } else { 1.0 }));
                }
            }
        }
        if lines.len() >= 12 {
            break;
        }
    }
    (lines, comments)
}

fn ago(t: SystemTime) -> String {
    let Ok(age) = SystemTime::now().duration_since(t) else { return "Edited just now".into() };
    let mins = age.as_secs() / 60;
    match mins {
        0 => "Edited just now".into(),
        m if m < 60 => format!("Edited {m} min ago"),
        m if m < 60 * 24 => format!("Edited {} hour{} ago", m / 60, if m / 60 == 1 { "" } else { "s" }),
        m if m < 60 * 24 * 2 => "Edited yesterday".into(),
        m if m < 60 * 24 * 7 => format!("Edited {} days ago", m / (60 * 24)),
        _ => {
            let date: chrono::DateTime<chrono::Local> = t.into();
            format!("Edited {}", date.format("%e %b").to_string().trim())
        }
    }
}

/// The section's body.
pub(crate) fn show(ui: &mut Ui, p: &Palette, state: &mut DocsHome, prefs: &Prefs) -> Option<DocsAction> {
    state.sync(prefs);
    let mut action = None;

    let gap = 14.0;
    let width = ((ui.available_width() - gap * 3.0) / 4.0).floor().max(150.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        if start_card(ui, p, Plate::Icon(Icon::Plus), "Blank document", "Start writing", width, true).clicked() {
            action = Some(DocsAction::New);
        }
        if start_card(ui, p, Plate::Claude, "Draft with Claude", "Describe it, get a first draft", width, false).clicked() {
            state.draft = Some(String::new());
        }
        let template = start_card(ui, p, Plate::Doc(DocIcon::Page), "From a template", "Notes, a brief, a plan, a letter", width, false);
        egui::Popup::menu(&template).show(|ui| {
            ui.set_min_width(200.0);
            for (name, body) in TEMPLATES {
                if ui.button(name).clicked() {
                    action = Some(DocsAction::Template(body));
                    ui.close();
                }
            }
        });
        if start_card(ui, p, Plate::Icon(Icon::Open), "Open a file", "Markdown or plain text", width, false).clicked() {
            action = Some(DocsAction::OpenDialog);
        }
    });

    if let Some(draft) = &mut state.draft {
        ui.add_space(14.0);
        let mut go = false;
        let mut cancel = false;
        egui::Frame::new().fill(p.recess).stroke(egui::Stroke::new(1.0, p.accent_alpha(0x80))).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin { left: 14, right: 6, top: 6, bottom: 6 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (spark, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::draw(ui.painter(), spark, Icon::Sparkle, Color32::from_rgb(0xe8, 0x95, 0x7a));
                ui.add_space(6.0);
                let field = egui::TextEdit::singleline(draft).hint_text("What should Claude draft? A grant summary, a seminar plan, a cover letter…").frame(egui::Frame::NONE).desired_width(ui.available_width() - 150.0);
                let r = ui.add(field);
                if !r.has_focus() && draft.is_empty() {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                if widgets::ghost_button(ui, p, "Cancel").clicked() {
                    cancel = true;
                }
                if widgets::primary_button(ui, p, "Draft", !draft.trim().is_empty()).clicked() {
                    go = true;
                }
            });
        });
        if go && !draft.trim().is_empty() {
            action = Some(DocsAction::Draft(draft.trim().to_string()));
            state.draft = None;
        } else if cancel {
            state.draft = None;
        }
    }

    ui.add_space(28.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(widgets::micro_caps("Recently edited")).font(theme::heavy(10.5)).color(p.text_faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(crate::chrome::tilde(&super::expand(&prefs.docs_folder))).font(theme::body(11.5)).color(p.text_faint));
        });
    });
    ui.add_space(12.0);
    if state.entries.is_empty() {
        ui.label(egui::RichText::new("Nothing here yet. Documents you write are kept in this folder as Markdown.").font(theme::body(12.5)).color(p.text_muted));
        return action;
    }

    let cols = ((ui.available_width() + 16.0) / (CARD_W + 16.0)).floor().max(1.0) as usize;
    let w = ((ui.available_width() - 16.0 * (cols as f32 - 1.0)) / cols as f32).floor();
    for row in state.entries.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for entry in row {
                let r = doc_card(ui, p, entry, w);
                if r.clicked() {
                    action = Some(DocsAction::Open(entry.path.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Open").clicked() {
                        action = Some(DocsAction::Open(entry.path.clone()));
                        ui.close();
                    }
                    if ui.button(if cfg!(target_os = "macos") { "Show in Finder" } else { "Show in folder" }).clicked() {
                        action = Some(DocsAction::Reveal(entry.path.clone()));
                        ui.close();
                    }
                });
            }
        });
        ui.add_space(18.0);
    }
    action
}

enum Plate {
    Icon(Icon),
    Doc(DocIcon),
    Claude,
}

fn start_card(ui: &mut Ui, p: &Palette, plate: Plate, title: &str, subtitle: &str, width: f32, primary: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 124.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(18);
        let hovered = response.hovered();
        if hovered || primary {
            painter.add(theme::island_shadow(p).as_shape(rect, radius));
        }
        painter.rect_filled(rect, radius, p.wash(if hovered || primary { 0x12 } else { 0x08 }));
        painter.rect_stroke(rect, radius, egui::Stroke::new(1.0, if hovered || primary { p.accent_alpha(0x80) } else { p.wash(0x0c) }), egui::StrokeKind::Inside);
        theme::paint_sheen(painter, rect, 18.0, p);
        let square = Rect::from_min_size(rect.left_top() + vec2(18.0, 18.0), vec2(40.0, 40.0));
        let clay = Color32::from_rgb(0xe8, 0x95, 0x7a);
        let (fill, ink) = match plate {
            Plate::Claude => (clay.gamma_multiply(0.18), clay),
            _ => (p.accent_soft, p.accent_text),
        };
        painter.rect_filled(square, CornerRadius::same(13), fill);
        let glyph = Rect::from_center_size(square.center(), vec2(19.0, 19.0));
        match plate {
            Plate::Icon(i) => icons::draw(painter, glyph, i, ink),
            Plate::Doc(i) => doc_icons::draw(painter, glyph, i, ink),
            Plate::Claude => icons::draw(painter, glyph, Icon::Sparkle, ink),
        }
        painter.text(rect.left_bottom() + vec2(18.0, -38.0), Align2::LEFT_TOP, title, theme::strong(13.5), p.text);
        painter.text(rect.left_bottom() + vec2(18.0, -19.0), Align2::LEFT_TOP, subtitle, theme::body(12.0), p.text_muted);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn doc_card(ui: &mut Ui, p: &Palette, entry: &Entry, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, THUMB_H + 54.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let thumb = Rect::from_min_size(rect.min, vec2(width, THUMB_H));
    let radius = CornerRadius::same(14);
    if response.hovered() {
        painter.rect_filled(thumb.expand(4.0), CornerRadius::same(17), p.accent_alpha(0x80));
    }
    painter.add(egui::Shadow { offset: [0, 8], blur: 20, spread: 0, color: Color32::from_black_alpha(0x60) }.as_shape(thumb, radius));
    painter.rect_filled(thumb, radius, Color32::WHITE);

    // The first lines, in shape.
    let ink = Color32::from_rgb(0x2c, 0x28, 0x34);
    let grey = Color32::from_rgb(0xd6, 0xd2, 0xdd);
    let inner = thumb.shrink2(vec2(16.0, 18.0));
    let mut y = inner.top();
    let bar = |y: f32, x: f32, w: f32, h: f32, c: Color32| {
        painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, h)), CornerRadius::same((h / 2.0) as u8), c);
    };
    for line in &entry.lines {
        if y > inner.bottom() - 6.0 {
            break;
        }
        match *line {
            Line::Title(f) => {
                bar(y, inner.left(), inner.width() * f * 0.9, 6.0, ink);
                y += 14.0;
            }
            Line::Heading(f) => {
                y += 3.0;
                bar(y, inner.left(), inner.width() * f * 0.6, 4.0, ink);
                y += 10.0;
            }
            Line::Text(f) => {
                bar(y, inner.left(), inner.width() * f, 3.0, grey);
                y += 8.0;
            }
            Line::Bullet(f) => {
                painter.circle_filled(pos2(inner.left() + 2.0, y + 1.5), 1.6, Color32::from_rgb(0x6f, 0x6a, 0x78));
                bar(y, inner.left() + 8.0, (inner.width() - 8.0) * f, 3.0, grey);
                y += 8.0;
            }
            Line::Task(done, f) => {
                let sq = Rect::from_min_size(pos2(inner.left(), y - 1.5), Vec2::splat(6.0));
                if done {
                    painter.rect_filled(sq, CornerRadius::same(2), Color32::from_rgb(0x6a, 0x4f, 0xd6));
                } else {
                    painter.rect_stroke(sq, CornerRadius::same(2), egui::Stroke::new(1.0, Color32::from_rgb(0x9c, 0x97, 0xa6)), egui::StrokeKind::Inside);
                }
                bar(y, inner.left() + 10.0, (inner.width() - 10.0) * f, 3.0, grey);
                y += 9.0;
            }
            Line::Panel => {
                painter.rect_filled(Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 18.0)), CornerRadius::same(4), Color32::from_rgb(0xf1, 0xef, 0xf5));
                y += 24.0;
            }
        }
    }

    painter.text(pos2(rect.left() + 4.0, thumb.bottom() + 12.0), Align2::LEFT_TOP, widgets::elide(&entry.title, (width / 8.0) as usize, false), theme::strong(13.5), p.text);
    let mut meta = ago(entry.modified);
    if entry.comments > 0 {
        meta.push_str(&format!(" · {} comment{}", entry.comments, if entry.comments == 1 { "" } else { "s" }));
    }
    painter.text(pos2(rect.left() + 4.0, thumb.bottom() + 32.0), Align2::LEFT_TOP, meta, theme::body(11.5), p.text_faint);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl App {
    pub(crate) fn docs_home_action(&mut self, action: DocsAction) {
        match action {
            DocsAction::New => self.new_document(),
            DocsAction::Draft(prompt) => {
                self.new_document();
                if let Some(i) = self.docs.shown {
                    self.ask_claude_in_doc(i, super::claude::Asked::Draft(prompt));
                }
            }
            DocsAction::Template(body) => self.new_drafted_document(body),
            DocsAction::Open(path) => self.open_document(&path),
            DocsAction::OpenDialog => self.open_document_dialog(),
            DocsAction::Reveal(path) => {
                let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" })
                    .args(if cfg!(target_os = "macos") { vec!["-R".into(), path.display().to_string()] } else { vec![path.parent().unwrap_or(&path).display().to_string()] })
                    .spawn();
            }
        }
        self.home.docs.rescan();
    }

    pub(crate) fn open_document_dialog(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Documents", &super::EXTENSIONS)
            .set_directory(self.docs_folder())
            .set_title("Open Document")
            .pick_files()
        {
            for f in files {
                self.open_document(&f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbnail_follows_the_documents_shape() {
        let dir = std::env::temp_dir().join(format!("quark-docs-home-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Plan.md");
        std::fs::write(&path, "# Plan\n\nSome words.\n\n- one\n- [x] done\n").unwrap();
        let (lines, comments) = preview(&path);
        assert!(matches!(lines[0], Line::Title(_)));
        assert!(matches!(lines[1], Line::Text(_)));
        assert!(matches!(lines[2], Line::Bullet(_)));
        assert!(matches!(lines[3], Line::Task(true, _)));
        assert_eq!(comments, 0);
    }

    #[test]
    fn edits_read_as_people_say_them() {
        assert_eq!(ago(SystemTime::now()), "Edited just now");
        assert_eq!(ago(SystemTime::now() - Duration::from_secs(3 * 3600)), "Edited 3 hours ago");
        assert_eq!(ago(SystemTime::now() - Duration::from_secs(30 * 3600)), "Edited yesterday");
    }
}
