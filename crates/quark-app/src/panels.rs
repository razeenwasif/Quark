//! The side panels: thumbnails, bookmarks, comments, search, fields, attachments.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, vec2};
use quark_core::annot::AnnotId;
use quark_core::prefs::{Prefs, SidePanel};
use quark_pdf::service::{PdfService, Request};
use quark_ui::theme::Palette;
use quark_ui::widgets;

use crate::tab::Tab;
use crate::textures::{PageKey, TextureCache};

/// Something a panel wants the application to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelAction {
    None,
    GoToPage(usize),
    /// Scroll to an annotation and select it.
    RevealAnnotation(AnnotId),
    DeleteAnnotation(AnnotId),
    /// Jump to a search hit by index.
    GoToHit(usize),
    RunSearch(String),
    /// A page thumbnail was selected; the selection is already updated.
    SelectPages,
    MovePage { from: usize, to: usize },
    DeleteSelectedPages,
    RotateSelectedPages(bool),
    ExtractAttachment(usize),
    FocusField(String),
}

/// Thumbnail edge length in pixels.
const THUMB_EDGE: u32 = 150;

pub fn show(
    ui: &mut Ui,
    p: &Palette,
    which: SidePanel,
    tab: &mut Tab,
    service: &PdfService,
    textures: &mut TextureCache,
    prefs: &Prefs,
) -> PanelAction {
    match which {
        SidePanel::None => PanelAction::None,
        SidePanel::Thumbnails => thumbnails(ui, p, tab, service, textures, prefs),
        SidePanel::Bookmarks => bookmarks(ui, p, tab),
        SidePanel::Comments => comments(ui, p, tab),
        SidePanel::Search => search(ui, p, tab),
        SidePanel::Fields => fields(ui, p, tab),
        SidePanel::Attachments => attachments(ui, p, tab),
        SidePanel::Layers => placeholder(ui, p, "Layers", "This document has no optional content groups."),
        SidePanel::Signatures => signatures(ui, p, tab),
    }
}

fn placeholder(ui: &mut Ui, p: &Palette, title: &str, body: &str) -> PanelAction {
    widgets::panel_header(ui, p, title);
    ui.add_space(6.0);
    ui.label(RichText::new(body).color(p.text_faint).size(12.0));
    PanelAction::None
}

fn thumbnails(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut Tab,
    service: &PdfService,
    textures: &mut TextureCache,
    prefs: &Prefs,
) -> PanelAction {
    widgets::panel_header(ui, p, "Page Thumbnails");
    let mut action = PanelAction::None;

    let width = ui.available_width();
    // Thumbnails are laid out at a fixed aspect so the list scrolls smoothly;
    // the real page aspect is applied when the image arrives.
    let thumb_w = (width - 24.0).clamp(60.0, 200.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for page in 0..tab.page_count() {
                let size = tab.info.page_size(page);
                let aspect = if size.width > 0.0 {
                    size.height / size.width
                } else {
                    1.294
                };
                let thumb_h = (thumb_w * aspect).clamp(40.0, 400.0);

                let (rect, response) =
                    ui.allocate_exact_size(vec2(width, thumb_h + 24.0), Sense::click_and_drag());

                if ui.is_rect_visible(rect) {
                    let selected = tab.selected_pages.contains(&page);
                    let current = tab.current_page == page;
                    let painter = ui.painter();

                    let img_rect = egui::Rect::from_center_size(
                        egui::pos2(rect.center().x, rect.top() + thumb_h / 2.0 + 4.0),
                        vec2(thumb_w, thumb_h),
                    );

                    if selected || current {
                        painter.rect_filled(
                            img_rect.expand(4.0),
                            CornerRadius::same(4),
                            if selected { p.accent_soft } else { p.card_hover },
                        );
                    }

                    // Thumbnails are cached under a page key at their own
                    // scale, so they never collide with the main view's
                    // textures for the same page.
                    let scale = THUMB_EDGE as f32 / size.width.max(size.height).max(1.0);
                    let key = PageKey::new(page, scale, quark_core::geom::Rot::D0, prefs.tint);
                    match textures.get(&key) {
                        Some(tex) => {
                            painter.image(
                                tex.id(),
                                img_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        None => {
                            painter.rect_filled(img_rect, CornerRadius::same(2), Color32::WHITE);
                            if textures.mark_pending(key, 0) {
                                service.send(Request::Thumbnail {
                                    id: tab.id,
                                    page,
                                    max_edge: THUMB_EDGE,
                                    tint: prefs.tint,
                                    token: 0,
                                });
                            }
                        }
                    }

                    painter.rect_stroke(
                        img_rect,
                        CornerRadius::same(2),
                        Stroke::new(
                            if current { 1.5 } else { 1.0 },
                            if current { p.accent } else { p.page_border },
                        ),
                        egui::StrokeKind::Outside,
                    );

                    painter.text(
                        egui::pos2(rect.center().x, rect.bottom() - 9.0),
                        egui::Align2::CENTER_CENTER,
                        tab.info.page_label(page),
                        egui::FontId::proportional(10.0),
                        if current { p.accent_text } else { p.text_muted },
                    );
                }

                if response.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    if modifiers.ctrl || modifiers.command {
                        // Ctrl-click toggles, so a mis-click can be undone
                        // without losing the whole selection.
                        if let Some(i) = tab.selected_pages.iter().position(|&x| x == page) {
                            tab.selected_pages.remove(i);
                        } else {
                            tab.selected_pages.push(page);
                        }
                    } else if modifiers.shift && !tab.selected_pages.is_empty() {
                        let anchor = *tab.selected_pages.last().unwrap();
                        let (a, b) = (anchor.min(page), anchor.max(page));
                        tab.selected_pages = (a..=b).collect();
                    } else {
                        tab.selected_pages = vec![page];
                        action = PanelAction::GoToPage(page);
                    }
                    if action == PanelAction::None {
                        action = PanelAction::SelectPages;
                    }
                }

                // --- drag to reorder ---
                //
                // The drag is tracked in egui memory rather than in the tab,
                // because the panel is drawn from a borrow that ends with the
                // frame; anything written to the tab here would survive, but
                // the *source* page has to persist across frames while the
                // pointer is down, and memory is where per-widget state lives.
                let drag_id = egui::Id::new("quark-thumb-drag");
                if response.drag_started() {
                    ui.memory_mut(|m| m.data.insert_temp(drag_id, page));
                }
                let dragging: Option<usize> =
                    ui.memory(|m| m.data.get_temp::<usize>(drag_id));
                if let Some(from) = dragging {
                    if response.hovered() && from != page {
                        // Show where the page would land.
                        let y = if page > from { rect.bottom() } else { rect.top() };
                        ui.painter().line_segment(
                            [
                                egui::pos2(rect.left() + 8.0, y),
                                egui::pos2(rect.right() - 8.0, y),
                            ],
                            Stroke::new(2.0, p.accent),
                        );
                    }
                    if ui.input(|i| i.pointer.any_released()) {
                        ui.memory_mut(|m| m.data.remove::<usize>(drag_id));
                        if response.hovered() && from != page {
                            action = PanelAction::MovePage { from, to: page };
                        }
                    }
                }

                response.context_menu(|ui| {
                    let n = tab.selected_pages.len().max(1);
                    ui.label(
                        RichText::new(format!("{n} page(s) selected"))
                            .size(10.0)
                            .color(p.text_faint),
                    );
                    ui.separator();
                    if ui.button("Rotate clockwise").clicked() {
                        action = PanelAction::RotateSelectedPages(true);
                        ui.close();
                    }
                    if ui.button("Rotate counterclockwise").clicked() {
                        action = PanelAction::RotateSelectedPages(false);
                        ui.close();
                    }
                    ui.separator();
                    // Never offer an action that would empty the document.
                    let can_delete = n < tab.page_count();
                    if ui
                        .add_enabled(can_delete, egui::Button::new("Delete page(s)"))
                        .clicked()
                    {
                        action = PanelAction::DeleteSelectedPages;
                        ui.close();
                    }
                });
            }
        });

    action
}

fn bookmarks(ui: &mut Ui, p: &Palette, tab: &Tab) -> PanelAction {
    widgets::panel_header(ui, p, "Bookmarks");
    if tab.outline.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new("This document has no bookmarks.")
                .color(p.text_faint)
                .size(12.0),
        );
        return PanelAction::None;
    }

    let mut action = PanelAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut flat = Vec::new();
            for b in &tab.outline {
                b.flatten(&mut flat);
            }
            for b in &flat {
                let selected = b.page == Some(tab.current_page);
                let (rect, response) = widgets::list_row(ui, p, 24.0, selected);
                if ui.is_rect_visible(rect) {
                    let indent = 8.0 + b.depth as f32 * 12.0;
                    ui.painter().text(
                        egui::pos2(rect.left() + indent, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        widgets::elide(&b.title, 40, false),
                        egui::FontId::proportional(12.0),
                        if selected { p.accent_text } else { p.text },
                    );
                }
                if response.clicked() {
                    if let Some(page) = b.page {
                        action = PanelAction::GoToPage(page);
                    }
                }
            }
        });
    action
}

fn comments(ui: &mut Ui, p: &Palette, tab: &Tab) -> PanelAction {
    widgets::panel_header(ui, p, "Comments");
    let all = tab.all_annotations();
    if all.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new("No comments yet. Use the markup tools to add one.")
                .color(p.text_faint)
                .size(12.0),
        );
        return PanelAction::None;
    }

    let mut action = PanelAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for ann in all {
                let selected = tab.selected_annotation == Some(ann.id);
                let frame = egui::Frame::new()
                    .fill(if selected { p.accent_soft } else { Color32::TRANSPARENT })
                    .stroke(Stroke::new(1.0, if selected { p.accent } else { p.border }))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(7));

                let r = frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // A swatch of the annotation's own colour, so the list
                        // maps onto the page at a glance.
                        let (sw, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().rect_filled(
                            sw,
                            CornerRadius::same(2),
                            Color32::from_rgb(ann.color.r, ann.color.g, ann.color.b),
                        );
                        ui.label(
                            RichText::new(ann.kind.subtype_name())
                                .size(10.0)
                                .color(p.text_faint),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("p. {}", ann.page + 1))
                                    .size(10.0)
                                    .color(p.text_faint),
                            );
                        });
                    });

                    if !ann.subject.trim().is_empty() {
                        ui.label(
                            RichText::new(format!("“{}”", widgets::elide(&ann.subject, 70, false)))
                                .size(11.0)
                                .italics()
                                .color(p.text_muted),
                        );
                    }
                    let body = ann.summary();
                    if !body.is_empty() {
                        ui.label(RichText::new(body).size(12.0).color(p.text));
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&ann.author).size(10.0).color(p.text_faint),
                        );
                        if ann.thread_len() > 1 {
                            ui.label(
                                RichText::new(format!("{} replies", ann.replies.len()))
                                    .size(10.0)
                                    .color(p.accent_text),
                            );
                        }
                    });
                });

                let response = r.response.interact(Sense::click());
                if response.clicked() {
                    action = PanelAction::RevealAnnotation(ann.id);
                }
                response.context_menu(|ui| {
                    if ui.button("Delete comment").clicked() {
                        action = PanelAction::DeleteAnnotation(ann.id);
                        ui.close();
                    }
                });
                ui.add_space(4.0);
            }
        });
    action
}

fn search(ui: &mut Ui, p: &Palette, tab: &mut Tab) -> PanelAction {
    widgets::panel_header(ui, p, "Search");
    let mut action = PanelAction::None;

    let mut query = tab.search.query.clone();
    let edit = ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("Find in document")
            .desired_width(f32::INFINITY),
    );
    if edit.changed() {
        tab.search.query = query.clone();
    }
    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !query.is_empty() {
        action = PanelAction::RunSearch(query.clone());
    }

    ui.horizontal(|ui| {
        ui.checkbox(&mut tab.search.options.match_case, "Match case");
        ui.checkbox(&mut tab.search.options.whole_words, "Whole words");
    });

    ui.add_space(4.0);
    ui.label(
        RichText::new(tab.search.position_label())
            .size(11.0)
            .color(p.text_muted),
    );
    if tab.search.running {
        ui.add(egui::ProgressBar::new(tab.search.progress()).desired_height(3.0));
    }
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, hit) in tab.search.hits.iter().enumerate() {
                let selected = tab.search.current == Some(i);
                let (rect, response) = widgets::list_row(ui, p, 38.0, selected);
                if ui.is_rect_visible(rect) {
                    let painter = ui.painter();
                    painter.text(
                        egui::pos2(rect.left() + 8.0, rect.top() + 10.0),
                        egui::Align2::LEFT_CENTER,
                        format!("Page {}", hit.page + 1),
                        egui::FontId::proportional(10.0),
                        p.text_faint,
                    );
                    painter.text(
                        egui::pos2(rect.left() + 8.0, rect.top() + 25.0),
                        egui::Align2::LEFT_CENTER,
                        widgets::elide(&hit.context, 46, false),
                        egui::FontId::proportional(11.0),
                        if selected { p.text } else { p.text_muted },
                    );
                }
                if response.clicked() {
                    action = PanelAction::GoToHit(i);
                }
            }
        });
    action
}

fn fields(ui: &mut Ui, p: &Palette, tab: &Tab) -> PanelAction {
    widgets::panel_header(ui, p, "Form Fields");
    if tab.fields.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new("This document has no interactive form fields.")
                .color(p.text_faint)
                .size(12.0),
        );
        return PanelAction::None;
    }

    let missing = tab.fields.iter().filter(|f| f.is_missing_required()).count();
    if missing > 0 {
        ui.label(
            RichText::new(format!("{missing} required field(s) still empty"))
                .size(11.0)
                .color(p.warning),
        );
        ui.add_space(4.0);
    }

    let mut action = PanelAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &tab.fields {
                let (rect, response) = widgets::list_row(ui, p, 34.0, false);
                if ui.is_rect_visible(rect) {
                    let painter = ui.painter();
                    let name = if f.name.is_empty() {
                        f.kind.label().to_string()
                    } else {
                        f.name.clone()
                    };
                    painter.text(
                        egui::pos2(rect.left() + 8.0, rect.top() + 10.0),
                        egui::Align2::LEFT_CENTER,
                        widgets::elide(&name, 34, false),
                        egui::FontId::proportional(11.5),
                        p.text,
                    );
                    let value = if f.password && !f.value.is_empty() {
                        // Never echo a password field's contents into a panel.
                        "••••••".to_string()
                    } else if f.value.is_empty() {
                        "—".to_string()
                    } else {
                        widgets::elide(&f.value, 34, false)
                    };
                    painter.text(
                        egui::pos2(rect.left() + 8.0, rect.top() + 24.0),
                        egui::Align2::LEFT_CENTER,
                        value,
                        egui::FontId::proportional(10.5),
                        if f.is_missing_required() {
                            p.warning
                        } else {
                            p.text_faint
                        },
                    );
                }
                if response.clicked() {
                    action = PanelAction::FocusField(f.name.clone());
                }
            }
        });
    action
}

fn attachments(ui: &mut Ui, p: &Palette, tab: &Tab) -> PanelAction {
    widgets::panel_header(ui, p, "Attachments");
    if tab.attachments.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new("No files are attached to this document.")
                .color(p.text_faint)
                .size(12.0),
        );
        return PanelAction::None;
    }
    let mut action = PanelAction::None;
    for a in &tab.attachments {
        let (rect, response) = widgets::list_row(ui, p, 30.0, false);
        if ui.is_rect_visible(rect) {
            ui.painter().text(
                egui::pos2(rect.left() + 8.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{}  ({})", widgets::elide(&a.name, 30, true), human_size(a.size)),
                egui::FontId::proportional(11.5),
                p.text,
            );
        }
        if response.double_clicked() {
            action = PanelAction::ExtractAttachment(a.index);
        }
    }
    action
}

fn signatures(ui: &mut Ui, p: &Palette, tab: &Tab) -> PanelAction {
    widgets::panel_header(ui, p, "Signatures");
    ui.add_space(6.0);
    if tab.info.signature_count == 0 {
        ui.label(
            RichText::new("This document is not signed.")
                .color(p.text_faint)
                .size(12.0),
        );
    } else {
        ui.label(
            RichText::new(format!(
                "{} signature field(s) present.",
                tab.info.signature_count
            ))
            .color(p.text)
            .size(12.0),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Quark reports that signatures exist but does not yet verify \
                 their cryptographic validity — treat them as unverified.",
            )
            .color(p.warning)
            .size(11.0),
        );
    }
    PanelAction::None
}

/// Formats a byte count for display.
pub fn human_size(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    // One decimal below 10 so "1.4 MB" is not rounded to "1 MB".
    if v < 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.0} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_counts_are_formatted_with_sensible_precision() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(20 * 1024), "20 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn very_large_sizes_do_not_run_off_the_unit_table() {
        let s = human_size(usize::MAX);
        assert!(s.ends_with("TB"), "got {s}");
    }

    #[test]
    fn panel_actions_compare_by_value() {
        assert_eq!(PanelAction::GoToPage(3), PanelAction::GoToPage(3));
        assert_ne!(PanelAction::GoToPage(3), PanelAction::GoToPage(4));
    }
}
