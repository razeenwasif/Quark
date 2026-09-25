//! Shared widgets.

use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2, vec2};

use crate::icons::{self, Icon};
use crate::theme::{Palette, RADIUS_CONTROL};

/// A square toolbar button showing one icon.
///
/// `active` draws the accent state, which is how the current tool is shown.
pub fn tool_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    tooltip: &str,
    active: bool,
    enabled: bool,
) -> Response {
    let size = Vec2::splat(30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && enabled;
        let painter = ui.painter();

        if active {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p.accent_soft);
            painter.rect_stroke(
                rect,
                CornerRadius::same(RADIUS_CONTROL),
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        } else if hovered {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p.card_hover);
        }

        let color = if !enabled {
            // Faint rather than a different hue: a greyed control should read
            // as unavailable, not as a different kind of control.
            p.text_faint
        } else if active {
            p.accent_text
        } else {
            p.text
        };
        icons::draw(painter, rect.shrink(7.0), icon, color);
    }

    let response = if enabled {
        response
    } else {
        // Consumes the click so a disabled button cannot fire.
        response.on_disabled_hover_text(tooltip)
    };
    if enabled && !tooltip.is_empty() {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

/// A text button styled as a toolbar item.
pub fn text_button(ui: &mut Ui, p: &Palette, label: &str, active: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        p.text,
    );
    let size = vec2(galley.size().x + 16.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if active {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p.accent_soft);
        } else if response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p.card_hover);
        }
        let color = if active { p.accent_text } else { p.text };
        let pos = rect.center() - galley.size() / 2.0;
        painter.galley(pos, galley, color);
    }
    response
}

/// A thin vertical rule between toolbar groups.
pub fn separator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 26.0), Sense::hover());
    let x = rect.center().x.round();
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.top() + 4.0),
            egui::pos2(x, rect.bottom() - 4.0),
        ],
        Stroke::new(1.0, p.border),
    );
}

/// A small header above a group inside a side panel.
pub fn panel_header(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(micro_caps(title))
            .size(10.0)
            .color(p.text_faint),
    );
    ui.add_space(2.0);
}

/// Spaced small capitals, for section headings.
pub fn micro_caps(label: &str) -> String {
    label
        .to_uppercase()
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\u{2009}")
}

/// A colour swatch that reports when it is picked.
pub fn swatch(ui: &mut Ui, p: &Palette, color: Color32, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect.shrink(2.0), CornerRadius::same(4), color);
        // The ring is drawn outside the swatch, so a dark colour still shows a
        // visible selection against a dark panel.
        let stroke = if selected {
            Stroke::new(2.0, p.accent_text)
        } else {
            Stroke::new(1.0, p.border)
        };
        painter.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(5),
            stroke,
            egui::StrokeKind::Middle,
        );
    }
    response
}

/// A row in a list panel, with hover and selection states.
pub fn list_row(ui: &mut Ui, p: &Palette, height: f32, selected: bool) -> (Rect, Response) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, CornerRadius::same(6), p.accent_soft);
            // A vertical accent bar on the leading edge, matching Neutron's
            // active navigation item.
            painter.rect_filled(
                Rect::from_min_size(rect.left_top() + vec2(0.0, 3.0), vec2(2.5, rect.height() - 6.0)),
                CornerRadius::same(2),
                p.accent,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(6), p.card_hover);
        }
    }
    (rect, response)
}

/// Truncates a label to fit `max_width`, with a trailing ellipsis.
///
/// Middle-truncates paths so the filename — the part that identifies the
/// document — survives, which a plain trailing cut would destroy.
pub fn elide(text: &str, max_chars: usize, keep_tail: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 4 {
        return text.to_owned();
    }
    if keep_tail {
        let tail = max_chars - 1;
        let start = chars.len() - tail;
        format!("…{}", chars[start..].iter().collect::<String>())
    } else {
        format!("{}…", chars[..max_chars - 1].iter().collect::<String>())
    }
}


/// A tool button on the dark rail.
///
/// Separate from [`tool_button`] because the rail is dark in both themes: the
/// panel-tuned hover and icon colours are invisible against it.
pub fn rail_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    tooltip: &str,
    active: bool,
    enabled: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && enabled;
        let painter = ui.painter();
        if active {
            // A tinted plate rather than a solid fill: the rail is already
            // dark, so a solid accent would shout.
            painter.rect_filled(
                rect,
                CornerRadius::same(RADIUS_CONTROL),
                Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 0x2b),
            );
        } else if hovered {
            painter.rect_filled(
                rect,
                CornerRadius::same(RADIUS_CONTROL),
                Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0x12),
            );
        }
        let color = if !enabled {
            Color32::from_rgba_unmultiplied(p.rail_icon.r(), p.rail_icon.g(), p.rail_icon.b(), 0x66)
        } else if active {
            p.accent_text
        } else {
            p.rail_icon
        };
        icons::draw(painter, rect.shrink(7.0), icon, color);
    }
    if enabled {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

/// The document name at the head of the nav sidebar.
pub fn nav_title(ui: &mut Ui, p: &Palette, title: &str) {
    ui.horizontal(|ui| {
        let (badge, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        if ui.is_rect_visible(badge) {
            let painter = ui.painter();
            painter.rect_filled(badge, CornerRadius::same(5), p.accent_soft);
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                "PDF",
                egui::FontId::proportional(8.0),
                p.accent_text,
            );
        }
        ui.add_space(2.0);
        ui.label(egui::RichText::new(title).size(12.5).strong().color(p.text));
    });
}

/// One entry in the nav sidebar.
pub fn nav_item(ui: &mut Ui, p: &Palette, icon: Icon, label: &str, active: bool) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if active {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p.accent_soft);
        } else if response.hovered() {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p.card_hover);
        }
        let color = if active { p.accent_text } else { p.text_muted };
        let icon_rect = Rect::from_min_size(rect.left_top() + vec2(8.0, 7.0), vec2(16.0, 16.0));
        icons::draw(painter, icon_rect, icon, color);
        painter.text(
            rect.left_center() + vec2(32.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.5),
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The document identity chip at the left of the toolbar: a type badge, the
/// filename, and the folder it came from underneath.
///
/// The folder is the disambiguator — two documents called `invoice.pdf` are
/// only told apart by where they live.
pub fn file_chip(ui: &mut Ui, p: &Palette, name: &str, folder: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(190.0, 30.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let badge = Rect::from_min_size(rect.left_top() + vec2(0.0, 3.0), Vec2::splat(24.0));
    painter.rect_filled(badge, CornerRadius::same(5), p.accent_soft);
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        "PDF",
        egui::FontId::proportional(8.0),
        p.accent_text,
    );
    // Two lines when there is a folder, one centred line when there is not.
    if folder.is_empty() {
        painter.text(
            rect.left_center() + vec2(32.0, 0.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(12.5),
            p.text,
        );
    } else {
        painter.text(
            rect.left_top() + vec2(32.0, 3.0),
            egui::Align2::LEFT_TOP,
            name,
            egui::FontId::proportional(12.5),
            p.text,
        );
        painter.text(
            rect.left_top() + vec2(32.0, 17.0),
            egui::Align2::LEFT_TOP,
            folder,
            egui::FontId::proportional(10.5),
            p.text_faint,
        );
    }
}

/// Groups related controls inside a hairline capsule.
///
/// The reference gathers page navigation and zoom into two such capsules; the
/// border is what makes each read as one control rather than as loose buttons.
pub fn pill<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui)
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_caps_uppercases_and_spaces() {
        let s = micro_caps("Pages");
        assert!(s.starts_with('P'));
        assert!(s.contains('\u{2009}'));
        assert!(!s.contains("Pa"), "letters should be spaced apart");
    }

    #[test]
    fn short_labels_are_left_alone() {
        assert_eq!(elide("short", 20, false), "short");
    }

    #[test]
    fn eliding_a_path_keeps_the_filename() {
        // The end of a path is what identifies the document; cutting it makes
        // every tab in a deep folder look identical.
        let s = elide("/very/long/path/to/report.pdf", 14, true);
        assert!(s.ends_with("report.pdf"), "got {s:?}");
        assert!(s.starts_with('…'));
        assert_eq!(s.chars().count(), 14);
    }

    #[test]
    fn eliding_a_title_keeps_the_start() {
        let s = elide("A very long document title indeed", 10, false);
        assert!(s.starts_with('A'));
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), 10);
    }

    #[test]
    fn eliding_handles_multibyte_without_panicking() {
        // Byte slicing here would panic mid-character.
        let s = elide("日本語のドキュメントのタイトル", 6, false);
        assert_eq!(s.chars().count(), 6);
    }

    #[test]
    fn an_absurdly_small_budget_returns_the_original() {
        assert_eq!(elide("abcdef", 2, false), "abcdef");
    }
}
