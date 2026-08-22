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
