//! The page grid: every page small, in place of the pages being read, to be
//! chosen and rearranged. A click chooses a page, ⌘-click adds or takes one
//! away, ⇧-click chooses all between; a page dragged — or the pages chosen,
//! when it is one of them — goes where it is dropped. The bar above turns,
//! deletes and puts a blank page after the pages chosen. A double-click, or
//! Escape, goes back to reading.

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, Ui, vec2};
use quark_core::prefs::Prefs;
use quark_pdf::service::{PdfService, Request};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

use crate::tab::Tab;
use crate::textures::{PageKey, TextureCache};

/// A cell: the page fitted in a box this size, its label under it.
const BOX_W: f32 = 132.0;
const BOX_H: f32 = 170.0;
const CELL_H: f32 = BOX_H + 34.0;
const GAP: f32 = 22.0;

/// What the grid asks for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GridAction {
    None,
    /// Back to reading, at this page.
    Read(usize),
    /// The pages chosen turned clockwise, or not.
    Turn(bool),
    /// A blank page after the last chosen.
    Blank,
    Delete,
    /// The pages chosen, into a file of their own.
    Extract,
    /// The pages chosen moved to before page `before` (the page count for the
    /// end).
    Move { pages: Vec<usize>, before: usize },
}

/// The document's new order when `pages` move to before `before`: the old
/// index of each new slot, and where the moved pages now start. None when
/// they would land where they are.
pub(crate) fn moved(count: usize, pages: &[usize], before: usize) -> Option<(Vec<usize>, usize)> {
    let mut chosen: Vec<usize> = pages.iter().copied().filter(|p| *p < count).collect();
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.is_empty() {
        return None;
    }
    let rest: Vec<usize> = (0..count).filter(|p| !chosen.contains(p)).collect();
    // Where they go among the pages that stay.
    let at = rest.iter().filter(|p| **p < before).count();
    let mut order = rest[..at].to_vec();
    order.extend(&chosen);
    order.extend(&rest[at..]);
    (order != (0..count).collect::<Vec<_>>()).then_some((order, at))
}

/// Pages chosen by a click on `page`, as a list chooses: alone, toggled with
/// ⌘, or a run from the last chosen with ⇧.
pub(crate) fn choose(chosen: &mut Vec<usize>, page: usize, toggle: bool, run: bool) {
    if run {
        if let Some(&from) = chosen.last() {
            let (a, b) = (from.min(page), from.max(page));
            if !toggle {
                chosen.clear();
            }
            for p in a..=b {
                if !chosen.contains(&p) {
                    chosen.push(p);
                }
            }
            // The page clicked stays the one a run goes on from.
            chosen.retain(|p| *p != page);
            chosen.push(page);
            return;
        }
    }
    if toggle {
        match chosen.iter().position(|p| *p == page) {
            Some(i) => {
                chosen.remove(i);
            }
            None => chosen.push(page),
        }
    } else {
        *chosen = vec![page];
    }
}

pub(crate) fn show(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut Tab,
    service: &PdfService,
    textures: &mut TextureCache,
    prefs: &Prefs,
) -> GridAction {
    let mut action = GridAction::None;
    let count = tab.page_count();
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    if !typing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return GridAction::Read(tab.selected_pages.first().copied().unwrap_or(tab.current_page));
    }

    // The bar: what is chosen, and what can be done to it.
    egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let n = tab.selected_pages.len();
            let said = match n {
                0 => "Choose pages: click, ⌘-click, ⇧-click".to_string(),
                1 => "1 page chosen".to_string(),
                n => format!("{n} pages chosen"),
            };
            ui.label(egui::RichText::new(said).font(theme::body(12.5)).color(p.text_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if widgets::primary_button(ui, p, "Done", true).on_hover_text("Back to reading  Esc").clicked() {
                    action = GridAction::Read(tab.selected_pages.first().copied().unwrap_or(tab.current_page));
                }
                if widgets::ghost_button(ui, p, "All").on_hover_text("Choose every page  ⌘A").clicked() {
                    tab.selected_pages = (0..count).collect();
                }
                let some = n > 0;
                let keeps_one = n < count;
                if ui.add_enabled(some && keeps_one, egui::Button::new("Delete")).on_hover_text("⌫").clicked() {
                    action = GridAction::Delete;
                }
                if ui.add_enabled(some, egui::Button::new("Extract…")).on_hover_text("Into a file of their own").clicked() {
                    action = GridAction::Extract;
                }
                if ui.add_enabled(some, egui::Button::new("+ Blank")).on_hover_text("A blank page after").clicked() {
                    action = GridAction::Blank;
                }
                if ui.add_enabled(some, egui::Button::new("⟳")).on_hover_text("Turn right").clicked() {
                    action = GridAction::Turn(true);
                }
                if ui.add_enabled(some, egui::Button::new("⟲")).on_hover_text("Turn left").clicked() {
                    action = GridAction::Turn(false);
                }
            });
        });
    });

    let drag_id = egui::Id::new("quark-grid-drag");
    let mut drop_before: Option<usize> = None;
    let dragging = ui.memory(|m| m.data.get_temp::<bool>(drag_id)).unwrap_or(false);
    let edge = crate::panels::thumb_edge(ui.ctx());

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(26, 10)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                for page in 0..count {
                    let (rect, response) = ui.allocate_exact_size(vec2(BOX_W, CELL_H), Sense::click_and_drag());
                    if ui.is_rect_visible(rect) {
                        paint_cell(ui, p, tab, service, textures, prefs, page, rect, edge);
                    }
                    if response.clicked() {
                        let m = ui.input(|i| i.modifiers);
                        choose(&mut tab.selected_pages, page, m.command || m.ctrl, m.shift);
                    }
                    if response.double_clicked() {
                        action = GridAction::Read(page);
                    }
                    if response.drag_started() {
                        if !tab.selected_pages.contains(&page) {
                            tab.selected_pages = vec![page];
                        }
                        ui.memory_mut(|m| m.data.insert_temp(drag_id, true));
                    }
                    // Dropped on its far half, a cell's page has them after it.
                    if dragging {
                        if let Some(at) = ui.input(|i| i.pointer.hover_pos()).filter(|at| rect.expand(GAP / 2.0).contains(*at)) {
                            let after = at.x > rect.center().x;
                            drop_before = Some(if after { page + 1 } else { page });
                            let x = if after { rect.right() + GAP / 2.0 } else { rect.left() - GAP / 2.0 };
                            ui.painter().line_segment(
                                [egui::pos2(x, rect.top()), egui::pos2(x, rect.top() + BOX_H)],
                                Stroke::new(3.0, p.accent),
                            );
                        }
                    }
                }
            });
        });
    });

    if dragging && ui.input(|i| i.pointer.any_released()) {
        ui.memory_mut(|m| m.data.remove::<bool>(drag_id));
        if let Some(before) = drop_before {
            action = GridAction::Move { pages: tab.selected_pages.clone(), before };
        }
    }
    action
}

/// A page small, fitted in its box, chosen or not, its label under it.
#[allow(clippy::too_many_arguments)]
fn paint_cell(
    ui: &Ui,
    p: &Palette,
    tab: &Tab,
    service: &PdfService,
    textures: &mut TextureCache,
    prefs: &Prefs,
    page: usize,
    rect: Rect,
    edge: u32,
) {
    let painter = ui.painter();
    let size = tab.info.page_size(page);
    let turned = matches!(tab.info.pages.get(page).map(|i| i.rotation.degrees()), Some(90 | 270));
    let (w, h) = if turned { (size.height, size.width) } else { (size.width, size.height) };
    let fit = (BOX_W / w.max(1.0)).min(BOX_H / h.max(1.0));
    let shown = vec2(w * fit, h * fit);
    let img = Rect::from_min_size(
        egui::pos2(rect.center().x - shown.x / 2.0, rect.top() + BOX_H - shown.y),
        shown,
    );
    let chosen = tab.selected_pages.contains(&page);
    let radius = CornerRadius::same(theme::RADIUS_THUMB);
    if chosen {
        painter.rect_filled(img.expand(7.0), CornerRadius::same(theme::RADIUS_THUMB + 6), p.accent_soft);
    }
    painter.add(
        egui::epaint::Shadow { offset: [0, 5], blur: 14, spread: 0, color: p.page_shadow.gamma_multiply(0.7) }
            .as_shape(img, radius),
    );
    // Kept under the side panel's key, so the two share their pictures.
    let scale = edge as f32 / size.width.max(size.height).max(1.0);
    let key = PageKey::new(page, scale, quark_core::geom::Rot::D0, prefs.tint);
    match textures.get(&key) {
        Some(tex) => {
            painter.add(
                egui::epaint::RectShape::filled(img, radius, Color32::WHITE)
                    .with_texture(tex.id(), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))),
            );
        }
        None => {
            painter.rect_filled(img, radius, Color32::WHITE);
            if textures.mark_pending(key, 0) {
                service.send(Request::Thumbnail { id: tab.id, page, max_edge: edge, tint: prefs.tint, token: 0 });
            }
        }
    }
    if chosen {
        painter.rect_stroke(img.expand(2.0), radius, Stroke::new(2.0, p.accent), egui::StrokeKind::Outside);
    }
    let label = tab.info.page_label(page);
    let current = tab.current_page == page;
    painter.text(
        egui::pos2(rect.center().x, rect.top() + BOX_H + 16.0),
        egui::Align2::CENTER_CENTER,
        label,
        theme::strong(11.5),
        if current { p.accent_text } else { p.text_faint },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_moved_go_before_the_page_dropped_on_in_their_own_order() {
        // 0 1 2 3 4 5: 1 and 3 dropped before 5.
        assert_eq!(moved(6, &[3, 1], 5), Some((vec![0, 2, 4, 1, 3, 5], 3)));
        // To the front, and to the end.
        assert_eq!(moved(4, &[2], 0), Some((vec![2, 0, 1, 3], 0)));
        assert_eq!(moved(4, &[0, 1], 4), Some((vec![2, 3, 0, 1], 2)));
        // Dropped on themselves, or just after: nothing moves.
        assert_eq!(moved(4, &[1, 2], 1), None);
        assert_eq!(moved(4, &[1, 2], 3), None);
        assert_eq!(moved(4, &[], 2), None);
        assert_eq!(moved(4, &[9], 2), None, "a page that is not there is not moved");
    }

    #[test]
    fn a_click_chooses_one_command_toggles_and_shift_runs_from_the_last() {
        let mut c = Vec::new();
        choose(&mut c, 2, false, false);
        assert_eq!(c, [2]);
        choose(&mut c, 5, true, false);
        assert_eq!(c, [2, 5]);
        choose(&mut c, 2, true, false);
        assert_eq!(c, [5]);
        choose(&mut c, 7, false, true);
        assert_eq!(c, [5, 6, 7]);
        // ⌘⇧ adds a run to what is chosen.
        let mut c = vec![0];
        choose(&mut c, 3, false, false);
        choose(&mut c, 5, true, true);
        assert_eq!(c, [3, 4, 5]);
        let mut c = vec![0, 1];
        choose(&mut c, 9, false, false);
        assert_eq!(c, [9], "a plain click starts again");
    }
}
