//! The dock's comments, and the outline island's find and replace.
//!
//! A comment is made on a selection, the way a Google Doc's is: the text is
//! marked amber on the page and the thread sits in the dock. Clicking either
//! brings the other forward. Resolved threads leave the page and the list,
//! and come back with "Show resolved".

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, Ui, Vec2, vec2};
use quark_doc::{Author, Note, Pos, Selection, survey};
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

use super::view::{self, Action};
use super::{DocTab, DockTab};

/// A comment being written, on the text it will anchor to.
#[derive(Debug, Clone)]
pub(crate) struct NewComment {
    pub(crate) from: Pos,
    pub(crate) to: Pos,
    pub(crate) text: String,
}

/// ⌘⌥M or the toolbar's comment button: a new comment on the selection, or
/// on the word at the caret when nothing is selected.
pub(crate) fn start_comment(tab: &mut DocTab, actions: &mut Vec<Action>) {
    if tab.editor.selection().is_caret() {
        let head = tab.editor.selection().head;
        tab.editor.select_word_at(head);
    }
    let (from, to) = tab.editor.selection().ordered();
    if from == to || tab.editor.selected_text().trim().is_empty() {
        actions.push(Action::Toast("Select some text to comment on".into(), false));
        return;
    }
    tab.new_comment = Some(NewComment { from, to, text: String::new() });
    tab.dock = DockTab::Comments;
    actions.push(Action::OpenDock);
}

/// The dock: comments, or Claude.
pub(crate) fn dock(ui: &mut Ui, p: &Palette, tab: &mut DocTab, claude: &super::claude::Status, actions: &mut Vec<Action>) {
    let open = tab.editor.doc.comments.iter().filter(|c| !c.resolved).count();
    let segments = [widgets::Segment::Counted("Comments", open), widgets::Segment::Labelled(Icon::Sparkle, "Claude")];
    let current = usize::from(tab.dock == DockTab::Claude);
    if let Some(i) = widgets::segmented(ui, p, &segments, current, true, 34.0) {
        tab.dock = if i == 1 { DockTab::Claude } else { DockTab::Comments };
    }
    ui.add_space(10.0);
    match tab.dock {
        DockTab::Comments => comments(ui, p, tab, actions),
        DockTab::Claude => super::claude::panel(ui, p, tab, claude, actions),
    }
}

fn comments(ui: &mut Ui, p: &Palette, tab: &mut DocTab, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(widgets::micro_caps(if tab.show_resolved { "All" } else { "Open" })).font(theme::heavy(10.5)).color(p.text_faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if tab.show_resolved { "Hide resolved" } else { "Show resolved" };
            if widgets::ghost_button(ui, p, label).clicked() {
                tab.show_resolved = !tab.show_resolved;
            }
        });
    });
    ui.add_space(6.0);

    let composer_h = 56.0;
    let list_h = (ui.available_height() - composer_h - 8.0).max(60.0);
    egui::ScrollArea::vertical().id_salt("doc-comments").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if let Some(new) = &mut tab.new_comment {
            let quote = tab.editor.doc.text_between(new.from, new.to);
            let mut post = false;
            let mut cancel = false;
            card(ui, p, true, |ui| {
                quote_box(ui, p, &quote);
                let field = egui::TextEdit::multiline(&mut new.text).hint_text("Add a comment, or @Claude to ask…").desired_rows(2).desired_width(f32::INFINITY);
                let r = ui.add(field);
                if !r.has_focus() && new.text.is_empty() {
                    r.request_focus();
                }
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                    post = true;
                }
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, p, "Comment", !new.text.trim().is_empty()).clicked() {
                        post = true;
                    }
                    if widgets::ghost_button(ui, p, "Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if post && !new.text.trim().is_empty() {
                let text = new.text.trim().to_string();
                let (from, to) = (new.from, new.to);
                tab.new_comment = None;
                tab.editor.set_selection(Selection::new(from, to));
                if let Some(id) = tab.editor.add_comment(&text) {
                    tab.active_comment = Some(id);
                    if mentions_claude(&text) {
                        actions.push(Action::Claude(super::claude::Asked::Comment(id)));
                    }
                }
                tab.editor.caret_to(to);
                ui.ctx().memory_mut(|m| m.request_focus(view::paper_id()));
            } else if cancel {
                tab.new_comment = None;
                ui.ctx().memory_mut(|m| m.request_focus(view::paper_id()));
            }
        }

        let shown: Vec<u32> = ordered(tab).into_iter().filter(|id| tab.show_resolved || tab.editor.doc.comment(*id).is_some_and(|c| !c.resolved)).collect();
        if shown.is_empty() && tab.new_comment.is_none() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                icons::draw(ui.painter(), Rect::from_center_size(ui.cursor().center_top() + vec2(0.0, 14.0), Vec2::splat(26.0)), Icon::Comment, p.text_faint);
                ui.add_space(36.0);
                ui.label(egui::RichText::new("No comments yet").font(theme::strong(13.0)).color(p.text_muted));
                ui.label(egui::RichText::new(widgets::platform_keys("Select text and press Ctrl+Alt+M")).font(theme::body(12.0)).color(p.text_faint));
            });
        }
        for id in shown {
            thread(ui, p, tab, id, actions);
        }
    });

    // The reply box, for the comment chosen.
    ui.add_space(6.0);
    let Some(active) = tab.active_comment.filter(|id| tab.editor.doc.comment(*id).is_some()) else { return };
    let mut send = false;
    egui::Frame::new().fill(p.recess).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin { left: 14, right: 6, top: 6, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut tab.draft).hint_text("Reply, or @Claude to ask…").frame(egui::Frame::NONE).desired_width(ui.available_width() - 44.0);
            let r = ui.add(field);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                send = true;
            }
            if send_button(ui, p, !tab.draft.trim().is_empty()).clicked() {
                send = true;
            }
        });
    });
    if send && !tab.draft.trim().is_empty() {
        let text = std::mem::take(&mut tab.draft).trim().to_string();
        let claude = mentions_claude(&text);
        if let Some(c) = tab.editor.doc.comment_mut(active) {
            c.notes.push(Note::now(Author::You, text));
            c.resolved = false;
        }
        tab.editor.touch_comments();
        if claude {
            actions.push(Action::Claude(super::claude::Asked::Comment(active)));
        }
    }
}

fn mentions_claude(text: &str) -> bool {
    text.to_lowercase().contains("@claude")
}

/// Comments in the order their text appears, unanchored ones last.
fn ordered(tab: &DocTab) -> Vec<u32> {
    let doc = &tab.editor.doc;
    let mut ids: Vec<(Option<Pos>, u32)> = doc.comments.iter().map(|c| (doc.anchor_range(c.id).map(|(a, _)| a), c.id)).collect();
    ids.sort_by_key(|(at, id)| (at.is_none(), *at, *id));
    ids.into_iter().map(|(_, id)| id).collect()
}

fn card(ui: &mut Ui, p: &Palette, active: bool, add: impl FnOnce(&mut Ui)) -> egui::Response {
    let frame = egui::Frame::new()
        .fill(if active { p.wash(0x0a) } else { p.wash(0x06) })
        .stroke(if active { Stroke::new(1.0, p.accent_alpha(0x80)) } else { Stroke::new(1.0, p.wash(0x0a)) })
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin::same(12));
    frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui);
        })
        .response
}

fn quote_box(ui: &mut Ui, p: &Palette, quote: &str) {
    egui::Frame::new().fill(p.recess).corner_radius(CornerRadius::same(11)).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let shown = widgets::elide(&quote.replace('\n', " "), 140, false);
        ui.label(egui::RichText::new(format!("“{shown}”")).font(theme::body(11.5)).italics().color(p.text_muted));
    });
}

fn thread(ui: &mut Ui, p: &Palette, tab: &mut DocTab, id: u32, actions: &mut Vec<Action>) {
    let Some(comment) = tab.editor.doc.comment(id).cloned() else { return };
    let active = tab.active_comment == Some(id);
    let anchored = tab.editor.doc.anchor_range(id);
    let mut resolve = false;
    let mut delete = false;
    let response = card(ui, p, active, |ui| {
        quote_box(ui, p, &comment.quote);
        if anchored.is_none() {
            ui.label(egui::RichText::new("The text this was about has been deleted.").font(theme::body(11.5)).color(p.text_faint));
        }
        for note in &comment.notes {
            ui.horizontal(|ui| {
                avatar(ui, p, note.author);
                ui.label(egui::RichText::new(match note.author {
                    Author::You => "You",
                    Author::Claude => "Claude",
                }).font(theme::strong(12.0)).color(p.text));
                ui.label(egui::RichText::new(relative(&note.at)).font(theme::body(11.0)).color(p.text_faint));
            });
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.add(egui::Label::new(egui::RichText::new(&note.text).font(theme::body(12.5)).color(p.text)).wrap());
            });
        }
        ui.horizontal(|ui| {
            if comment.resolved {
                ui.label(egui::RichText::new("Resolved").font(theme::strong(11.5)).color(p.success));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, p, Icon::Trash, "Delete this comment", 26.0, false, true).clicked() {
                    delete = true;
                }
                let label = if comment.resolved { "Reopen" } else { "Resolve" };
                if widgets::ghost_button(ui, p, label).clicked() {
                    resolve = true;
                }
                if !comment.resolved && widgets::ghost_button(ui, p, "Ask Claude").clicked() {
                    actions.push(Action::Claude(super::claude::Asked::Comment(id)));
                }
            });
        });
    });
    if resolve {
        if let Some(c) = tab.editor.doc.comment_mut(id) {
            c.resolved = !c.resolved;
        }
        tab.editor.touch_comments();
        if tab.active_comment == Some(id) {
            tab.active_comment = None;
        }
    } else if delete {
        tab.editor.remove_comment(id);
        if tab.active_comment == Some(id) {
            tab.active_comment = None;
        }
    } else if response.interact(Sense::click()).clicked() {
        tab.active_comment = Some(id);
        if let Some((a, b)) = anchored {
            tab.editor.set_selection(Selection::new(a, b));
            tab.reveal = true;
        }
    }
}

fn avatar(ui: &mut Ui, p: &Palette, who: Author) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    match who {
        Author::You => {
            ui.painter().circle_filled(rect.center(), 10.0, p.accent_soft);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Y", theme::heavy(9.5), p.accent_text);
        }
        Author::Claude => {
            let clay = egui::Color32::from_rgb(0xe8, 0x95, 0x7a);
            ui.painter().circle_filled(rect.center(), 10.0, clay.gamma_multiply(0.2));
            icons::draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(11.0)), Icon::Sparkle, clay);
        }
    }
}

pub(crate) fn send_button(ui: &mut Ui, p: &Palette, enabled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    let fill = if enabled { p.accent } else { p.wash(0x0a) };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    icons::draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(16.0)), Icon::ArrowUp, if enabled { p.on_accent } else { p.text_disabled });
    if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

/// An RFC 3339 time as a recent list says it.
pub(crate) fn relative(stamp: &str) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(stamp) else { return String::new() };
    let mins = chrono::Local::now().signed_duration_since(then).num_minutes();
    match mins {
        m if m < 1 => "just now".into(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 60 * 24 => format!("{} h ago", m / 60),
        m if m < 60 * 24 * 7 => format!("{} d ago", m / (60 * 24)),
        _ => then.format("%e %b").to_string().trim().to_string(),
    }
}

// --- find and replace ---

pub(crate) fn find_panel(ui: &mut Ui, p: &Palette, tab: &mut DocTab) {
    let Some(find) = &mut tab.find else { return };
    let mut step: Option<bool> = None;
    let mut replace_one = false;
    let mut replace_all = false;
    let mut close = false;

    let field = egui::TextEdit::singleline(&mut find.query).hint_text("Find in document").desired_width(f32::INFINITY);
    let r = ui.add(field);
    if std::mem::take(&mut find.focus) {
        r.request_focus();
    }
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        step = Some(ui.input(|i| i.modifiers.shift));
        r.request_focus();
    }
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    let hits = survey::find(&tab.editor.doc, &find.query, find.match_case);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let label = if find.query.is_empty() {
            String::new()
        } else if hits.is_empty() {
            "No matches".into()
        } else {
            let sel = tab.editor.selection().ordered();
            match hits.iter().position(|h| *h == sel) {
                Some(i) => format!("{} of {}", i + 1, hits.len()),
                None => format!("{} matches", hits.len()),
            }
        };
        ui.label(egui::RichText::new(label).font(theme::body(12.0)).color(p.text_muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::icon_button(ui, p, Icon::ChevronDown, "Next (Enter)", 28.0, false, !hits.is_empty()).clicked() {
                step = Some(false);
            }
            if widgets::icon_button(ui, p, Icon::ChevronUp, "Previous (Shift+Enter)", 28.0, false, !hits.is_empty()).clicked() {
                step = Some(true);
            }
        });
    });
    ui.checkbox(&mut find.match_case, "Match case");
    ui.add_space(8.0);
    ui.checkbox(&mut find.replace, "Replace");
    if find.replace {
        ui.add(egui::TextEdit::singleline(&mut find.with).hint_text("Replace with").desired_width(f32::INFINITY));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if widgets::text_button(ui, p, "Replace", false).clicked() {
                replace_one = true;
            }
            if widgets::text_button(ui, p, "Replace all", false).clicked() {
                replace_all = true;
            }
        });
    }

    let with = find.with.clone();
    if close {
        tab.find = None;
        ui.ctx().memory_mut(|m| m.request_focus(view::paper_id()));
        return;
    }
    if replace_all && !hits.is_empty() {
        // From the end back, so earlier hits keep their places.
        for (a, b) in hits.iter().rev() {
            tab.editor.replace(*a, *b, &with);
        }
        tab.reveal = true;
    } else if replace_one {
        let sel = tab.editor.selection().ordered();
        if hits.contains(&sel) {
            tab.editor.replace(sel.0, sel.1, &with);
        }
        view::find_step(tab, false);
    } else if let Some(back) = step {
        view::find_step(tab, back);
    }
}

/// Every hit of the find query, for the sheet to tint.
pub(crate) fn hits(tab: &DocTab) -> Vec<(Pos, Pos)> {
    match &tab.find {
        Some(f) if !f.query.is_empty() => survey::find(&tab.editor.doc, &f.query, f.match_case),
        _ => Vec::new(),
    }
}

/// The amber of a hit, under the text.
pub(crate) fn paint_hits(painter: &egui::Painter, tab: &DocTab, origin: Pos2) {
    for (a, b) in hits(tab) {
        for r in tab.sheet.selection(a, b) {
            painter.rect_filled(r.translate(origin.to_vec2()), CornerRadius::same(2), egui::Color32::from_rgba_premultiplied(0x5c, 0x46, 0x0e, 0x5c));
        }
    }
}

use egui::Pos2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_is_asked_by_name_in_any_case() {
        assert!(mentions_claude("what do you think @Claude?"));
        assert!(mentions_claude("@claude"));
        assert!(!mentions_claude("Claudette"));
    }

    #[test]
    fn times_read_as_a_person_says_them() {
        assert_eq!(relative(&chrono::Local::now().to_rfc3339()), "just now");
        let earlier = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(relative(&earlier), "5 min ago");
        assert_eq!(relative("garbage"), "");
    }
}
