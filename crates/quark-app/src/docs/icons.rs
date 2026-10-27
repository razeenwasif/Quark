//! The few icons only the docs editor uses, drawn the way `quark_ui::icons`
//! draws the rest: strokes in a unit box.

use egui::{Color32, Painter, Rect, Stroke, Vec2, pos2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocIcon {
    Bullets,
    Numbers,
    Checklist,
    Quote,
    Code,
    Link,
    AddComment,
    /// A page with lines on it: a document, as against a PDF.
    Page,
    Export,
    ClearFormat,
}

pub(crate) fn draw(painter: &Painter, rect: Rect, icon: DocIcon, color: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center() - Vec2::splat(s / 2.0);
    let p = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let w = (s * 0.075).clamp(1.2, 2.2);
    let stroke = Stroke::new(w, color);
    let line = |a: (f32, f32), b: (f32, f32)| painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
    let dot = |x: f32, y: f32| painter.circle_filled(p(x, y), w * 0.95, color);

    match icon {
        DocIcon::Bullets => {
            for y in [0.27, 0.5, 0.73] {
                dot(0.2, y);
                line((0.38, y), (0.84, y));
            }
        }
        DocIcon::Numbers => {
            for y in [0.27, 0.5, 0.73] {
                line((0.42, y), (0.84, y));
            }
            line((0.19, 0.18), (0.24, 0.16));
            line((0.24, 0.16), (0.24, 0.36));
            painter.add(egui::Shape::line(vec![p(0.15, 0.6), p(0.22, 0.56), p(0.28, 0.6), p(0.16, 0.8), p(0.3, 0.8)], stroke));
        }
        DocIcon::Checklist => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.14, 0.17), p(0.36, 0.39)),
                egui::CornerRadius::same((s * 0.07) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(vec![p(0.16, 0.68), p(0.23, 0.75), p(0.36, 0.6)], stroke));
            line((0.5, 0.28), (0.86, 0.28));
            line((0.5, 0.68), (0.86, 0.68));
        }
        DocIcon::Quote => {
            for x in [0.2, 0.56] {
                painter.add(egui::Shape::line(
                    vec![p(x + 0.18, 0.3), p(x, 0.3), p(x, 0.5), p(x + 0.18, 0.5), p(x + 0.18, 0.56), p(x + 0.1, 0.74)],
                    stroke,
                ));
            }
        }
        DocIcon::Code => {
            painter.add(egui::Shape::line(vec![p(0.38, 0.3), p(0.18, 0.5), p(0.38, 0.7)], stroke));
            painter.add(egui::Shape::line(vec![p(0.62, 0.3), p(0.82, 0.5), p(0.62, 0.7)], stroke));
        }
        DocIcon::Link => {
            let r = s * 0.13;
            painter.add(egui::Shape::line(vec![p(0.45, 0.62), p(0.36, 0.71)], stroke));
            painter.rect_stroke(
                Rect::from_center_size(p(0.36, 0.64), Vec2::new(s * 0.3, s * 0.2)),
                egui::CornerRadius::same(r as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                Rect::from_center_size(p(0.64, 0.36), Vec2::new(s * 0.3, s * 0.2)),
                egui::CornerRadius::same(r as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            line((0.42, 0.58), (0.58, 0.42));
        }
        DocIcon::AddComment => {
            painter.add(egui::Shape::closed_line(
                vec![p(0.16, 0.22), p(0.84, 0.22), p(0.84, 0.68), p(0.48, 0.68), p(0.3, 0.84), p(0.3, 0.68), p(0.16, 0.68)],
                stroke,
            ));
            line((0.5, 0.33), (0.5, 0.57));
            line((0.38, 0.45), (0.62, 0.45));
        }
        DocIcon::Page => {
            painter.add(egui::Shape::closed_line(
                vec![p(0.27, 0.14), p(0.56, 0.14), p(0.78, 0.36), p(0.78, 0.86), p(0.27, 0.86)],
                stroke,
            ));
            line((0.37, 0.55), (0.66, 0.55));
            line((0.37, 0.7), (0.56, 0.7));
        }
        DocIcon::Export => {
            line((0.5, 0.16), (0.5, 0.62));
            painter.add(egui::Shape::line(vec![p(0.32, 0.34), p(0.5, 0.16), p(0.68, 0.34)], stroke));
            painter.add(egui::Shape::line(vec![p(0.18, 0.58), p(0.18, 0.84), p(0.82, 0.84), p(0.82, 0.58)], stroke));
        }
        DocIcon::ClearFormat => {
            line((0.22, 0.22), (0.66, 0.22));
            line((0.44, 0.22), (0.34, 0.78));
            line((0.6, 0.58), (0.82, 0.8));
            line((0.82, 0.58), (0.6, 0.8));
        }
    }
}
