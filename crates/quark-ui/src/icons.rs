//! Toolbar icons, drawn as vector paths.
//!
//! Drawn rather than shipped as a font or an atlas: a PDF reader needs a few
//! dozen glyphs, and an icon font is a megabyte of dependency plus a licence to
//! track. These are a few lines of `Painter` calls each, scale to any size, and
//! take the theme colour directly.

use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

/// Every icon Quark draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Cursor,
    Hand,
    ZoomIn,
    ZoomOut,
    Search,
    Highlight,
    Underline,
    StrikeOut,
    Note,
    Text,
    Pen,
    Eraser,
    Line,
    Arrow,
    Square,
    Circle,
    Stamp,
    Redact,
    Signature,
    Measure,
    Thumbnails,
    Bookmark,
    Comment,
    Attachment,
    Layers,
    Fields,
    Save,
    Print,
    Open,
    Undo,
    Redo,
    RotateCw,
    RotateCcw,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    Close,
    Menu,
    Plus,
    Minus,
    Trash,
    Lock,
    Sun,
    Moon,
    FitWidth,
    FitPage,
    Grid,
    Check,
    Warning,
    Document,
    Home,
    /// The assistant.
    Sparkle,
    /// Send, in a composer.
    ArrowUp,
    More,
    Reply,
    Upload,
    Clock,
    Image,
    /// The dock toggle: a window with its right-hand column marked.
    Sidebar,
    /// The side-panel toggle: the left-hand column marked.
    SidebarLeft,
    Shapes,
    Minimize,
    Maximize,
}

/// Draws `icon` centred in `rect`, in `color`.
pub fn draw(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    // Everything below is expressed in a 0..1 box so the shapes are readable as
    // proportions; this maps that box onto the target rect.
    let s = rect.width().min(rect.height());
    let o = rect.center() - Vec2::splat(s / 2.0);
    let p = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let w = (s * 0.09).clamp(1.2, 2.4);
    let stroke = Stroke::new(w, color);

    // Round caps and joins, so no icon has a sharp corner. egui strokes are
    // mitred; a dot of the stroke's width at every vertex rounds them off.
    // Only for opaque colours — under a translucent one the dots would show
    // as darker beads where they overlap the stroke.
    let round = color.a() == 255;
    let cap = |at: Pos2| {
        if round {
            painter.circle_filled(at, w / 2.0, color);
        }
    };
    let line = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], stroke);
        cap(a);
        cap(b);
    };
    let poly = |pts: Vec<Pos2>| {
        for &pt in &pts {
            cap(pt);
        }
        painter.add(egui::Shape::line(pts, stroke));
    };
    let rounded = |r: Rect, radius: f32| {
        painter.rect_stroke(r, radius, stroke, egui::StrokeKind::Middle);
    };

    match icon {
        Icon::Cursor => poly(vec![
            p(0.28, 0.18),
            p(0.28, 0.78),
            p(0.44, 0.63),
            p(0.55, 0.85),
            p(0.66, 0.79),
            p(0.55, 0.58),
            p(0.74, 0.55),
            p(0.28, 0.18),
        ]),
        Icon::Hand => {
            // Three fingers and a thumb, each a rounded stroke, closing into
            // the palm — drawn on a 24-unit grid like the rest of the set.
            let g = |x: f32, y: f32| p(x / 24.0, y / 24.0);
            let finger = |x: f32, base: f32, tip: f32| {
                let mut pts = vec![g(x, base), g(x, tip)];
                for i in 0..=8 {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 8.0);
                    pts.push(g(x + 1.5 + 1.5 * a.cos(), tip + 1.5 * a.sin()));
                }
                pts.push(g(x + 3.0, base));
                pts
            };
            poly(finger(8.5, 12.5, 6.5));
            poly(finger(11.5, 11.0, 5.0));
            let mut palm = finger(14.5, 11.5, 7.0);
            palm.pop();
            palm.push(g(17.5, 14.0));
            for i in 1..=8 {
                let a = std::f32::consts::FRAC_PI_2 * (i as f32 / 8.0);
                palm.push(g(11.0 + 6.5 * a.cos(), 14.0 + 6.5 * a.sin()));
            }
            palm.extend([g(9.6, 20.4), g(7.6, 19.6), g(5.9, 18.1), g(3.6, 14.6), g(3.8, 13.0), g(5.0, 12.3), g(6.3, 12.8), g(8.5, 15.5)]);
            poly(palm);
        }
        Icon::ZoomIn | Icon::ZoomOut | Icon::Search => {
            painter.circle_stroke(p(0.44, 0.44), s * 0.24, stroke);
            line(p(0.62, 0.62), p(0.82, 0.82));
            if icon != Icon::Search {
                line(p(0.33, 0.44), p(0.55, 0.44));
            }
            if icon == Icon::ZoomIn {
                line(p(0.44, 0.33), p(0.44, 0.55));
            }
        }
        Icon::Highlight => {
            // A chisel-tipped marker over the line it has just drawn.
            let g = |x: f32, y: f32| p(x / 24.0, y / 24.0);
            poly(vec![g(15.0, 4.5), g(19.5, 9.0), g(11.0, 17.5), g(6.5, 17.5), g(6.5, 13.0), g(15.0, 4.5)]);
            line(g(12.5, 7.0), g(17.0, 11.5));
            painter.rect_filled(
                Rect::from_min_max(g(4.0, 19.6), g(13.0, 21.4)),
                s * 0.04,
                color.gamma_multiply(0.55),
            );
        }
        Icon::Underline => {
            poly(vec![p(0.32, 0.22), p(0.32, 0.52), p(0.68, 0.52), p(0.68, 0.22)]);
            line(p(0.24, 0.76), p(0.76, 0.76));
        }
        Icon::StrikeOut => {
            poly(vec![p(0.32, 0.24), p(0.32, 0.46)]);
            poly(vec![p(0.68, 0.24), p(0.68, 0.46)]);
            poly(vec![p(0.32, 0.46), p(0.68, 0.46)]);
            line(p(0.22, 0.52), p(0.78, 0.52));
        }
        Icon::Note | Icon::Comment => {
            poly(vec![
                p(0.20, 0.26),
                p(0.80, 0.26),
                p(0.80, 0.66),
                p(0.46, 0.66),
                p(0.32, 0.80),
                p(0.32, 0.66),
                p(0.20, 0.66),
                p(0.20, 0.26),
            ]);
        }
        Icon::Text => {
            line(p(0.26, 0.26), p(0.74, 0.26));
            line(p(0.50, 0.26), p(0.50, 0.76));
            line(p(0.38, 0.76), p(0.62, 0.76));
        }
        Icon::Pen => {
            poly(vec![p(0.22, 0.78), p(0.30, 0.56), p(0.66, 0.20), p(0.80, 0.34), p(0.44, 0.70), p(0.22, 0.78)]);
            line(p(0.62, 0.24), p(0.76, 0.38));
        }
        Icon::Eraser => {
            poly(vec![p(0.22, 0.70), p(0.52, 0.28), p(0.78, 0.46), p(0.48, 0.80), p(0.22, 0.70)]);
            line(p(0.36, 0.78), p(0.72, 0.78));
        }
        Icon::Line => {
            line(p(0.22, 0.78), p(0.78, 0.22));
        }
        Icon::Arrow => {
            line(p(0.22, 0.78), p(0.78, 0.22));
            poly(vec![p(0.54, 0.22), p(0.78, 0.22), p(0.78, 0.46)]);
        }
        Icon::Square => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.22, 0.24), p(0.78, 0.76)),
                3.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Circle => {
            painter.circle_stroke(p(0.5, 0.5), s * 0.28, stroke);
        }
        Icon::Stamp => {
            poly(vec![p(0.36, 0.46), p(0.36, 0.32), p(0.64, 0.32), p(0.64, 0.46)]);
            painter.rect_stroke(
                Rect::from_min_max(p(0.24, 0.46), p(0.76, 0.60)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(p(0.24, 0.72), p(0.76, 0.72));
        }
        Icon::Redact => {
            painter.rect_filled(Rect::from_min_max(p(0.20, 0.38), p(0.80, 0.62)), 2.0, color);
            line(p(0.20, 0.26), p(0.62, 0.26));
            line(p(0.20, 0.76), p(0.56, 0.76));
        }
        Icon::Signature => {
            poly(vec![
                p(0.20, 0.62),
                p(0.34, 0.36),
                p(0.44, 0.62),
                p(0.56, 0.30),
                p(0.66, 0.60),
                p(0.80, 0.44),
            ]);
            line(p(0.20, 0.78), p(0.80, 0.78));
        }
        Icon::Measure => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.16, 0.38), p(0.84, 0.62)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            for i in 1..4 {
                let x = 0.16 + 0.68 * (i as f32 / 4.0);
                line(p(x, 0.38), p(x, 0.50));
            }
        }
        Icon::Thumbnails | Icon::Grid => {
            for (cx, cy) in [(0.22, 0.22), (0.56, 0.22), (0.22, 0.56), (0.56, 0.56)] {
                painter.rect_stroke(
                    Rect::from_min_size(p(cx, cy), vec2(s * 0.22, s * 0.22)),
                    2.0,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Icon::Bookmark => poly(vec![
            p(0.30, 0.20),
            p(0.70, 0.20),
            p(0.70, 0.80),
            p(0.50, 0.62),
            p(0.30, 0.80),
            p(0.30, 0.20),
        ]),
        Icon::Attachment => poly(vec![
            p(0.66, 0.34),
            p(0.38, 0.62),
            p(0.38, 0.72),
            p(0.48, 0.72),
            p(0.76, 0.44),
            p(0.76, 0.30),
            p(0.62, 0.22),
            p(0.30, 0.54),
            p(0.30, 0.70),
        ]),
        Icon::Layers => {
            poly(vec![p(0.50, 0.20), p(0.82, 0.38), p(0.50, 0.56), p(0.18, 0.38), p(0.50, 0.20)]);
            poly(vec![p(0.18, 0.56), p(0.50, 0.74), p(0.82, 0.56)]);
        }
        Icon::Fields => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.18, 0.34), p(0.82, 0.66)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(p(0.28, 0.42), p(0.28, 0.58));
        }
        Icon::Save => {
            poly(vec![p(0.22, 0.22), p(0.66, 0.22), p(0.78, 0.34), p(0.78, 0.78), p(0.22, 0.78), p(0.22, 0.22)]);
            painter.rect_stroke(
                Rect::from_min_max(p(0.34, 0.22), p(0.64, 0.42)),
                1.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                Rect::from_min_max(p(0.34, 0.56), p(0.66, 0.78)),
                1.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Print => {
            poly(vec![p(0.30, 0.36), p(0.30, 0.20), p(0.70, 0.20), p(0.70, 0.36)]);
            painter.rect_stroke(
                Rect::from_min_max(p(0.18, 0.36), p(0.82, 0.64)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                Rect::from_min_max(p(0.30, 0.58), p(0.70, 0.82)),
                1.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Open => poly(vec![
            p(0.18, 0.72),
            p(0.18, 0.28),
            p(0.42, 0.28),
            p(0.50, 0.38),
            p(0.78, 0.38),
            p(0.78, 0.72),
            p(0.18, 0.72),
        ]),
        Icon::Undo | Icon::Redo => {
            let flip = icon == Icon::Redo;
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            painter.add(egui::Shape::line(
                vec![
                    p(fx(0.30), 0.36),
                    p(fx(0.62), 0.36),
                    p(fx(0.74), 0.50),
                    p(fx(0.62), 0.66),
                    p(fx(0.40), 0.66),
                ],
                stroke,
            ));
            poly(vec![p(fx(0.42), 0.24), p(fx(0.28), 0.36), p(fx(0.42), 0.48)]);
        }
        Icon::RotateCw | Icon::RotateCcw => {
            let flip = icon == Icon::RotateCcw;
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            let c = p(0.5, 0.5);
            let r = s * 0.26;
            let mut pts = Vec::new();
            // An open arc, so the arrowhead has somewhere to sit.
            for i in 0..=24 {
                let t = -0.35 + (i as f32 / 24.0) * 5.2;
                let (x, y) = (t.cos() * r, t.sin() * r);
                pts.push(pos2(c.x + if flip { -x } else { x }, c.y + y));
            }
            painter.add(egui::Shape::line(pts, stroke));
            poly(vec![p(fx(0.62), 0.20), p(fx(0.80), 0.30), p(fx(0.64), 0.42)]);
        }
        Icon::ChevronLeft => poly(vec![p(0.60, 0.26), p(0.38, 0.50), p(0.60, 0.74)]),
        Icon::ChevronRight => poly(vec![p(0.40, 0.26), p(0.62, 0.50), p(0.40, 0.74)]),
        Icon::ChevronUp => poly(vec![p(0.26, 0.60), p(0.50, 0.38), p(0.74, 0.60)]),
        Icon::ChevronDown => poly(vec![p(0.26, 0.40), p(0.50, 0.62), p(0.74, 0.40)]),
        Icon::Close => {
            line(p(0.28, 0.28), p(0.72, 0.72));
            line(p(0.72, 0.28), p(0.28, 0.72));
        }
        Icon::Menu => {
            for y in [0.32, 0.50, 0.68] {
                line(p(0.22, y), p(0.78, y));
            }
        }
        Icon::Plus => {
            line(p(0.50, 0.24), p(0.50, 0.76));
            line(p(0.24, 0.50), p(0.76, 0.50));
        }
        Icon::Minus => {
            line(p(0.24, 0.50), p(0.76, 0.50));
        }
        Icon::Trash => {
            line(p(0.22, 0.30), p(0.78, 0.30));
            poly(vec![p(0.30, 0.30), p(0.34, 0.80), p(0.66, 0.80), p(0.70, 0.30)]);
            poly(vec![p(0.40, 0.30), p(0.40, 0.20), p(0.60, 0.20), p(0.60, 0.30)]);
        }
        Icon::Lock => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.28, 0.46), p(0.72, 0.80)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(
                vec![p(0.38, 0.46), p(0.38, 0.32), p(0.62, 0.32), p(0.62, 0.46)],
                stroke,
            ));
        }
        Icon::Sun => {
            painter.circle_stroke(p(0.5, 0.5), s * 0.18, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let c = rect.center();
                let (i0, i1) = (s * 0.28, s * 0.40);
                painter.line_segment(
                    [
                        pos2(c.x + a.cos() * i0, c.y + a.sin() * i0),
                        pos2(c.x + a.cos() * i1, c.y + a.sin() * i1),
                    ],
                    stroke,
                );
            }
        }
        Icon::Moon => {
            // A crescent, drawn as an arc rather than two overlapping circles
            // so it works on any background.
            let c = rect.center();
            let r = s * 0.30;
            let mut pts = Vec::new();
            for i in 0..=20 {
                let t = 0.6 + (i as f32 / 20.0) * 4.9;
                pts.push(pos2(c.x + t.cos() * r, c.y + t.sin() * r));
            }
            for i in 0..=20 {
                let t = 5.5 - (i as f32 / 20.0) * 4.9;
                pts.push(pos2(
                    c.x + t.cos() * r * 1.25 - s * 0.10,
                    c.y + t.sin() * r * 0.95,
                ));
            }
            painter.add(egui::Shape::line(pts, stroke));
        }
        Icon::FitWidth => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.30, 0.24), p(0.70, 0.76)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(p(0.10, 0.50), p(0.26, 0.50));
            line(p(0.74, 0.50), p(0.90, 0.50));
            poly(vec![p(0.18, 0.42), p(0.10, 0.50), p(0.18, 0.58)]);
            poly(vec![p(0.82, 0.42), p(0.90, 0.50), p(0.82, 0.58)]);
        }
        Icon::FitPage => {
            painter.rect_stroke(
                Rect::from_min_max(p(0.30, 0.30), p(0.70, 0.70)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            for (a, b) in [
                ((0.16, 0.16), (0.28, 0.28)),
                ((0.84, 0.16), (0.72, 0.28)),
                ((0.16, 0.84), (0.28, 0.72)),
                ((0.84, 0.84), (0.72, 0.72)),
            ] {
                line(p(a.0, a.1), p(b.0, b.1));
            }
        }
        Icon::Check => poly(vec![p(0.24, 0.52), p(0.42, 0.70), p(0.76, 0.30)]),
        Icon::Document => {
            poly(vec![
                p(0.58, 0.16),
                p(0.30, 0.16),
                p(0.26, 0.20),
                p(0.26, 0.80),
                p(0.30, 0.84),
                p(0.70, 0.84),
                p(0.74, 0.80),
                p(0.74, 0.32),
                p(0.58, 0.16),
            ]);
            poly(vec![p(0.56, 0.18), p(0.56, 0.34), p(0.72, 0.34)]);
        }
        Icon::Home => poly(vec![
            p(0.18, 0.46),
            p(0.50, 0.20),
            p(0.82, 0.46),
            p(0.78, 0.80),
            p(0.60, 0.80),
            p(0.60, 0.60),
            p(0.40, 0.60),
            p(0.40, 0.80),
            p(0.22, 0.80),
            p(0.18, 0.46),
        ]),
        Icon::Sparkle => {
            // A four-pointed star with concave sides, as a smooth curve.
            let c = rect.center();
            let mut pts = Vec::new();
            for i in 0..=64 {
                let t = std::f32::consts::TAU * (i as f32 / 64.0);
                // Astroid: x = cos³, y = sin³.
                let (x, y) = (t.cos().powi(3), t.sin().powi(3));
                pts.push(pos2(c.x + x * s * 0.34, c.y + y * s * 0.34));
            }
            painter.add(egui::Shape::line(pts, stroke));
        }
        Icon::ArrowUp => {
            line(p(0.50, 0.78), p(0.50, 0.24));
            poly(vec![p(0.28, 0.46), p(0.50, 0.24), p(0.72, 0.46)]);
        }
        Icon::More => {
            for x in [0.26, 0.50, 0.74] {
                painter.circle_filled(p(x, 0.50), w * 0.85, color);
            }
        }
        Icon::Reply => {
            poly(vec![p(0.40, 0.30), p(0.20, 0.50), p(0.40, 0.70)]);
            poly(vec![p(0.20, 0.50), p(0.58, 0.50), p(0.72, 0.58), p(0.80, 0.76)]);
        }
        Icon::Upload => {
            line(p(0.50, 0.64), p(0.50, 0.20));
            poly(vec![p(0.32, 0.38), p(0.50, 0.20), p(0.68, 0.38)]);
            poly(vec![p(0.20, 0.62), p(0.20, 0.80), p(0.80, 0.80), p(0.80, 0.62)]);
        }
        Icon::Clock => {
            painter.circle_stroke(p(0.5, 0.5), s * 0.32, stroke);
            poly(vec![p(0.50, 0.32), p(0.50, 0.50), p(0.62, 0.58)]);
        }
        Icon::Image => {
            rounded(Rect::from_min_max(p(0.16, 0.22), p(0.84, 0.78)), s * 0.10);
            painter.circle_stroke(p(0.38, 0.42), s * 0.06, stroke);
            poly(vec![p(0.24, 0.74), p(0.56, 0.48), p(0.80, 0.68)]);
        }
        Icon::Sidebar => {
            rounded(Rect::from_min_max(p(0.16, 0.22), p(0.84, 0.78)), s * 0.12);
            line(p(0.60, 0.22), p(0.60, 0.78));
        }
        Icon::SidebarLeft => {
            rounded(Rect::from_min_max(p(0.16, 0.22), p(0.84, 0.78)), s * 0.12);
            line(p(0.40, 0.22), p(0.40, 0.78));
        }
        Icon::Shapes => {
            rounded(Rect::from_min_max(p(0.16, 0.42), p(0.58, 0.84)), s * 0.10);
            painter.circle_stroke(p(0.64, 0.36), s * 0.20, stroke);
        }
        Icon::Minimize => line(p(0.26, 0.50), p(0.74, 0.50)),
        Icon::Maximize => rounded(Rect::from_min_max(p(0.26, 0.26), p(0.74, 0.74)), s * 0.12),
        Icon::Warning => {
            poly(vec![p(0.50, 0.20), p(0.84, 0.78), p(0.16, 0.78), p(0.50, 0.20)]);
            line(p(0.50, 0.40), p(0.50, 0.58));
            painter.circle_filled(p(0.50, 0.68), w * 0.6, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_a_distinct_discriminant() {
        // Two names for the same variant would silently draw the wrong glyph.
        let all = [
            Icon::Cursor,
            Icon::Hand,
            Icon::ZoomIn,
            Icon::ZoomOut,
            Icon::Search,
            Icon::Highlight,
            Icon::Note,
            Icon::Pen,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn zoom_in_and_out_are_different_icons() {
        assert_ne!(Icon::ZoomIn, Icon::ZoomOut);
    }
}
