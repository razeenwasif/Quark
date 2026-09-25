//! The document canvas: painting pages, and everything the pointer does on them.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect as ERect, Sense, Stroke, Ui, pos2, vec2};
use quark_core::annot::{AnnotKind, Annotation, Color as QColor};
use quark_core::geom::{PageTransform, Rect, Vec2};
use quark_core::layout::{self, ZoomMode};
use quark_core::prefs::Prefs;
use quark_core::tools::Tool;
use quark_pdf::service::{PdfService, Request};
use quark_ui::theme::Palette;

use crate::tab::{Drafting, Tab, TextSelection};
use crate::textures::{PageKey, TextureCache};

/// Something the viewer wants the application to do.
///
/// The viewer never mutates the document itself: it reports intent, and the
/// application turns that into an undoable edit. That keeps the undo stack in
/// one place instead of scattered through the paint code.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerAction {
    None,
    /// A finished annotation to commit.
    CreateAnnotation(Box<Annotation>),
    /// An annotation was dragged to a new position.
    MoveAnnotation {
        id: quark_core::annot::AnnotId,
        delta: Vec2,
    },
    DeleteAnnotation(quark_core::annot::AnnotId),
    /// A marquee zoom finished.
    ZoomToRect { page: usize, rect: Rect },
    /// A snapshot rectangle was swept.
    Snapshot { page: usize, rect: Rect },
    /// A crop rectangle was swept.
    Crop { page: usize, rect: Rect },
    /// A form field was clicked.
    ActivateField(String),
    /// A link was followed.
    GoToPage(usize),
}

/// Converts a Quark colour to an egui one.
fn to_egui(c: QColor) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Applies an annotation's opacity to a colour.
fn with_opacity(c: QColor, opacity: f32) -> Color32 {
    let a = (opacity.clamp(0.0, 1.0) * c.a as f32) as u8;
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, a)
}

/// Draws the document and handles input on it.
pub fn show(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut Tab,
    service: &PdfService,
    textures: &mut TextureCache,
    prefs: &Prefs,
    tool: Tool,
) -> ViewerAction {
    let avail = ui.available_size();
    let (canvas, response) = ui.allocate_exact_size(avail, Sense::click_and_drag());

    tab.viewport = Vec2::new(canvas.width(), canvas.height());
    tab.ensure_layout();

    let painter = ui.painter_at(canvas);
    painter.rect_filled(canvas, CornerRadius::ZERO, p.canvas);

    if tab.page_count() == 0 {
        return ViewerAction::None;
    }

    // --- input ---
    let action = handle_input(ui, tab, &response, canvas, tool, prefs);

    // --- request rasters for what is about to be visible ---
    request_visible(tab, service, textures, prefs);

    // --- paint ---
    let origin = canvas.min - egui::vec2(tab.scroll.x, tab.scroll.y);
    let visible = tab.prerender_rect(prefs.prerender_pages);
    let page_boxes: Vec<_> = tab
        .layout
        .visible(visible)
        .into_iter()
        .map(|pb| (pb.index, pb.rect, pb.size))
        .collect();

    for (index, rect, size) in &page_boxes {
        let screen = ERect::from_min_size(
            pos2(origin.x + rect.min.x, origin.y + rect.min.y),
            vec2(rect.width(), rect.height()),
        );
        // Skip anything genuinely off-screen; the prerender margin means the
        // list includes pages that are not actually in view.
        if !screen.intersects(canvas) {
            continue;
        }

        paint_page_chrome(&painter, p, screen);

        let key = PageKey::new(*index, tab.layout.scale, tab.rotation, prefs.tint);
        match textures.get_nearest(*index, &key) {
            Some(tex) => {
                painter.image(
                    tex.id(),
                    screen,
                    ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                // A blank sheet reads as "loading" far better than an empty
                // hole in the canvas.
                painter.rect_filled(screen, CornerRadius::same(2), Color32::WHITE);
                let label = if textures.is_pending(&key) {
                    format!("Rendering page {}…", index + 1)
                } else {
                    format!("Page {}", index + 1)
                };
                painter.text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(13.0),
                    p.text_faint,
                );
            }
        }

        let t = PageTransform::new(*rect, *size, tab.rotation);
        paint_search_hits(&painter, p, tab, *index, &t, origin);
        paint_selection(&painter, p, tab, *index, &t, origin);
        paint_annotations(&painter, p, tab, *index, &t, origin);
        if prefs.side_panel == quark_core::prefs::SidePanel::Fields {
            paint_field_outlines(&painter, p, tab, *index, &t, origin);
        }
    }

    paint_drafting(&painter, p, tab, tool, origin);

    // Last, so the bars sit over the page rather than under it.
    scrollbars(ui, tab, canvas, p);

    action
}

/// The paper, its shadow and its edge.
fn paint_page_chrome(painter: &Painter, p: &Palette, screen: ERect) {
    painter.rect_filled(
        screen.translate(vec2(0.0, 3.0)).expand(1.0),
        CornerRadius::same(3),
        p.page_shadow,
    );
    painter.rect_stroke(
        screen,
        CornerRadius::same(2),
        Stroke::new(1.0, p.page_border),
        egui::StrokeKind::Outside,
    );
}

/// Asks the worker for any page raster the next few frames will need.
fn request_visible(
    tab: &mut Tab,
    service: &PdfService,
    textures: &mut TextureCache,
    prefs: &Prefs,
) {
    let scale = tab.layout.scale * prefs.render_supersample.clamp(1.0, 3.0);
    let visible = tab.prerender_rect(prefs.prerender_pages);
    let pages: Vec<usize> = tab
        .layout
        .visible(visible)
        .into_iter()
        .map(|pb| pb.index)
        .collect();

    for page in pages {
        let key = PageKey::new(page, tab.layout.scale, tab.rotation, prefs.tint);
        if textures.mark_pending(key, tab.render_token) {
            service.send(Request::Render {
                id: tab.id,
                request: quark_pdf::render::RenderRequest {
                    page,
                    scale,
                    rotation: tab.rotation,
                    tint: prefs.tint,
                    annotations: true,
                    forms: true,
                    smooth: true,
                },
                token: tab.render_token,
            });
        }
        // Text is needed for selection and for the highlighter, and is cheap
        // enough to fetch for anything on screen.
        if !tab.page_text.contains_key(&page) && !tab.text_pending.contains(&page) {
            tab.text_pending.push(page);
            service.send(Request::PageText { id: tab.id, page });
        }
    }
}

fn content_to_screen(origin: Pos2, v: Vec2) -> Pos2 {
    pos2(origin.x + v.x, origin.y + v.y)
}

fn paint_quad(painter: &Painter, origin: Pos2, t: &PageTransform, r: Rect, fill: Color32) {
    let c = t.page_rect_to_content(r);
    let screen = ERect::from_min_max(
        content_to_screen(origin, c.min),
        content_to_screen(origin, c.max),
    );
    painter.rect_filled(screen, CornerRadius::same(1), fill);
}

fn paint_selection(
    painter: &Painter,
    p: &Palette,
    tab: &Tab,
    page: usize,
    t: &PageTransform,
    origin: Pos2,
) {
    let Some(sel) = tab.selection else { return };
    if sel.page != page || sel.is_empty() {
        return;
    }
    let Some(text) = tab.text(page) else { return };
    let (a, b) = sel.range();
    for q in text.quads_for_range(a, b) {
        paint_quad(painter, origin, t, q, p.selection);
    }
}

fn paint_search_hits(
    painter: &Painter,
    p: &Palette,
    tab: &Tab,
    page: usize,
    t: &PageTransform,
    origin: Pos2,
) {
    if tab.search.hits.is_empty() {
        return;
    }
    let Some(text) = tab.text(page) else { return };
    let current = tab.search.current;
    for (i, hit) in tab.search.hits.iter().enumerate() {
        if hit.page != page {
            continue;
        }
        // The current hit is a different colour so it is findable among a
        // page full of matches.
        let fill = if Some(i) == current {
            p.search_current
        } else {
            p.search_other
        };
        for q in text.quads_for_range(hit.char_index, hit.char_index + hit.char_len) {
            paint_quad(painter, origin, t, q, fill);
        }
    }
}

fn paint_field_outlines(
    painter: &Painter,
    p: &Palette,
    tab: &Tab,
    page: usize,
    t: &PageTransform,
    origin: Pos2,
) {
    for f in tab.fields.iter().filter(|f| f.page == page) {
        let c = t.page_rect_to_content(f.rect);
        let screen = ERect::from_min_max(
            content_to_screen(origin, c.min),
            content_to_screen(origin, c.max),
        );
        let fill = if f.is_missing_required() {
            p.field_required
        } else {
            p.field_highlight
        };
        painter.rect_filled(screen, CornerRadius::same(2), fill);
    }
}

fn paint_annotations(
    painter: &Painter,
    p: &Palette,
    tab: &Tab,
    page: usize,
    t: &PageTransform,
    origin: Pos2,
) {
    for ann in tab.annotations_on(page) {
        let selected = tab.selected_annotation == Some(ann.id);
        paint_annotation(painter, p, ann, t, origin, selected);
    }
}

/// Draws one annotation.
///
/// Annotations that PDFium already rendered into the page raster are still
/// drawn here when selected, so the selection handles have something to sit on;
/// the rest are drawn because they were created in this session and are not in
/// the file yet.
fn paint_annotation(
    painter: &Painter,
    p: &Palette,
    ann: &Annotation,
    t: &PageTransform,
    origin: Pos2,
    selected: bool,
) {
    let scale = t.scale();
    let color = with_opacity(ann.color, ann.opacity);
    let width = (ann.border_width * scale).max(1.0);
    let stroke = Stroke::new(width, color);

    let to_screen = |v: Vec2| content_to_screen(origin, t.page_to_content(v));
    let rect_screen = |r: Rect| {
        let c = t.page_rect_to_content(r);
        ERect::from_min_max(
            content_to_screen(origin, c.min),
            content_to_screen(origin, c.max),
        )
    };

    // Only pending annotations need drawing; saved ones are already in the
    // raster. `dirty` is the flag that distinguishes them.
    if ann.dirty {
        match &ann.kind {
            AnnotKind::Highlight { quads } => {
                for q in quads {
                    painter.rect_filled(rect_screen(*q), CornerRadius::ZERO, color);
                }
            }
            AnnotKind::Underline { quads } => {
                for q in quads {
                    let r = rect_screen(*q);
                    painter.line_segment(
                        [pos2(r.left(), r.bottom()), pos2(r.right(), r.bottom())],
                        stroke,
                    );
                }
            }
            AnnotKind::StrikeOut { quads } => {
                for q in quads {
                    let r = rect_screen(*q);
                    let y = r.center().y;
                    painter
                        .line_segment([pos2(r.left(), y), pos2(r.right(), y)], stroke);
                }
            }
            AnnotKind::Squiggly { quads } => {
                for q in quads {
                    let r = rect_screen(*q);
                    let mut pts = Vec::new();
                    let step = (4.0 * scale).max(2.0);
                    let mut x = r.left();
                    let mut up = true;
                    while x < r.right() {
                        pts.push(pos2(x, if up { r.bottom() - step * 0.6 } else { r.bottom() }));
                        up = !up;
                        x += step;
                    }
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
            AnnotKind::Note { .. } => {
                let r = rect_screen(ann.rect);
                let c = r.center();
                let s = (11.0 * scale).clamp(9.0, 22.0);
                let box_ = ERect::from_center_size(c, vec2(s * 1.4, s));
                painter.rect_filled(box_, CornerRadius::same(3), color);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        pos2(box_.left() + s * 0.2, box_.bottom()),
                        pos2(box_.left() + s * 0.55, box_.bottom()),
                        pos2(box_.left() + s * 0.2, box_.bottom() + s * 0.4),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
            AnnotKind::FreeText { text, font_size, .. } => {
                let r = rect_screen(ann.rect);
                painter.rect_stroke(
                    r,
                    CornerRadius::same(2),
                    Stroke::new(1.0, color),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    r.left_top() + vec2(4.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    text,
                    egui::FontId::proportional((font_size * scale).clamp(7.0, 96.0)),
                    color,
                );
            }
            AnnotKind::Ink { strokes } => {
                for s in strokes {
                    if s.points.len() < 2 {
                        continue;
                    }
                    let pts: Vec<Pos2> = s.points.iter().map(|v| to_screen(*v)).collect();
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
            AnnotKind::Line {
                start,
                end,
                end_ending,
                ..
            } => {
                let (a, b) = (to_screen(*start), to_screen(*end));
                painter.line_segment([a, b], stroke);
                if *end_ending != quark_core::annot::LineEnding::None {
                    paint_arrowhead(painter, a, b, width, color);
                }
            }
            AnnotKind::Square => {
                let r = rect_screen(ann.rect);
                if let Some(fill) = ann.interior_color {
                    painter.rect_filled(r, CornerRadius::same(2), with_opacity(fill, ann.opacity));
                }
                painter.rect_stroke(r, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
            }
            AnnotKind::Circle => {
                let r = rect_screen(ann.rect);
                if let Some(fill) = ann.interior_color {
                    painter.add(egui::Shape::ellipse_filled(
                        r.center(),
                        r.size() / 2.0,
                        with_opacity(fill, ann.opacity),
                    ));
                }
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            AnnotKind::Polygon { points } => {
                let mut pts: Vec<Pos2> = points.iter().map(|v| to_screen(*v)).collect();
                if let Some(first) = pts.first().copied() {
                    pts.push(first);
                }
                painter.add(egui::Shape::line(pts, stroke));
            }
            AnnotKind::Polyline { points, .. } => {
                let pts: Vec<Pos2> = points.iter().map(|v| to_screen(*v)).collect();
                painter.add(egui::Shape::line(pts, stroke));
            }
            AnnotKind::Redact { .. } => {
                let r = rect_screen(ann.rect);
                // Marked, not applied: an outline, so it is obvious the content
                // is still there.
                painter.rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(2.0, p.error),
                    egui::StrokeKind::Middle,
                );
                painter.rect_filled(r, CornerRadius::ZERO, p.error.gamma_multiply(0.18));
            }
            AnnotKind::Stamp { name, .. } => {
                let r = rect_screen(ann.rect);
                painter.rect_stroke(
                    r,
                    CornerRadius::same(4),
                    Stroke::new(2.0, color),
                    egui::StrokeKind::Middle,
                );
                painter.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    name,
                    egui::FontId::proportional((14.0 * scale).clamp(9.0, 40.0)),
                    color,
                );
            }
            AnnotKind::FileAttachment { .. } | AnnotKind::Unsupported { .. } => {}
        }
    }

    if selected {
        let r = rect_screen(ann.rect).expand(3.0);
        painter.rect_stroke(
            r,
            CornerRadius::same(3),
            Stroke::new(1.5, p.accent),
            egui::StrokeKind::Outside,
        );
        for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
            painter.rect_filled(
                ERect::from_center_size(c, vec2(6.0, 6.0)),
                CornerRadius::same(1),
                p.accent,
            );
        }
    }
}

fn paint_arrowhead(painter: &Painter, from: Pos2, to: Pos2, width: f32, color: Color32) {
    let dir = (to - from).normalized();
    if !dir.x.is_finite() || !dir.y.is_finite() {
        return;
    }
    let size = (width * 4.0).clamp(6.0, 24.0);
    let perp = vec2(-dir.y, dir.x);
    painter.add(egui::Shape::convex_polygon(
        vec![
            to,
            to - dir * size + perp * size * 0.4,
            to - dir * size - perp * size * 0.4,
        ],
        color,
        Stroke::NONE,
    ));
}

/// Draws whatever the current tool is mid-way through creating.
fn paint_drafting(painter: &Painter, p: &Palette, tab: &Tab, tool: Tool, origin: Pos2) {
    let Some(page) = tab.drafting.page() else {
        return;
    };
    let Some(pb) = tab.layout.page(page) else {
        return;
    };
    let t = PageTransform::new(pb.rect, pb.size, tab.rotation);
    let to_screen = |v: Vec2| content_to_screen(origin, t.page_to_content(v));
    let color = to_egui(p_tool_color(p, tool));
    let stroke = Stroke::new(2.0, color);

    match &tab.drafting {
        Drafting::Rect { .. } | Drafting::Line { .. } => {
            let Some(r) = tab.drafting.rect() else { return };
            let c = t.page_rect_to_content(r);
            let screen = ERect::from_min_max(
                content_to_screen(origin, c.min),
                content_to_screen(origin, c.max),
            );
            match tool {
                Tool::Ellipse => {
                    painter.add(egui::Shape::ellipse_stroke(
                        screen.center(),
                        screen.size() / 2.0,
                        stroke,
                    ));
                }
                Tool::Line | Tool::Arrow => {
                    if let Drafting::Line { start, current, .. } = &tab.drafting {
                        let (a, b) = (to_screen(*start), to_screen(*current));
                        painter.line_segment([a, b], stroke);
                        if tool == Tool::Arrow {
                            paint_arrowhead(painter, a, b, 2.0, color);
                        }
                    }
                }
                Tool::ZoomArea | Tool::Snapshot | Tool::Crop => {
                    painter.rect_filled(screen, CornerRadius::ZERO, p.accent_soft);
                    painter.rect_stroke(
                        screen,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, p.accent),
                        egui::StrokeKind::Middle,
                    );
                }
                Tool::Redact => {
                    painter.rect_filled(screen, CornerRadius::ZERO, p.error.gamma_multiply(0.3));
                    painter.rect_stroke(
                        screen,
                        CornerRadius::ZERO,
                        Stroke::new(2.0, p.error),
                        egui::StrokeKind::Middle,
                    );
                }
                _ => {
                    painter.rect_stroke(screen, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
                }
            }
        }
        Drafting::Ink { points, .. } => {
            if points.len() > 1 {
                let pts: Vec<Pos2> = points.iter().map(|v| to_screen(*v)).collect();
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
        Drafting::Vertices { points, cursor, .. } => {
            let mut pts: Vec<Pos2> = points.iter().map(|v| to_screen(*v)).collect();
            pts.push(to_screen(*cursor));
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, stroke));
            }
            for v in points {
                painter.circle_filled(to_screen(*v), 3.0, color);
            }
        }
        Drafting::None => {}
    }
}

fn p_tool_color(p: &Palette, tool: Tool) -> QColor {
    let _ = p;
    match tool {
        Tool::Highlight => QColor::YELLOW,
        Tool::Redact => QColor::BLACK,
        _ => QColor::RED,
    }
}

/// Handles pointer and scroll input over the canvas.
fn handle_input(
    ui: &mut Ui,
    tab: &mut Tab,
    response: &egui::Response,
    canvas: ERect,
    tool: Tool,
    prefs: &Prefs,
) -> ViewerAction {
    // --- scroll and zoom ---
    let (scroll_delta, zoom_delta, modifiers) = ui.input(|i| {
        (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers)
    });

    if response.hovered() {
        if modifiers.ctrl || modifiers.command {
            // Ctrl+wheel zooms about the pointer.
            let factor = if zoom_delta != 1.0 {
                zoom_delta
            } else if scroll_delta.y.abs() > 0.0 {
                1.0 + scroll_delta.y * 0.004
            } else {
                1.0
            };
            if (factor - 1.0).abs() > 0.0001 {
                if let Some(pos) = response.hover_pos() {
                    let anchor = Vec2::new(pos.x - canvas.min.x, pos.y - canvas.min.y);
                    tab.zoom_to(tab.layout.scale * factor, anchor);
                }
            }
        } else if scroll_delta != egui::Vec2::ZERO {
            tab.scroll = tab.scroll - Vec2::new(scroll_delta.x, scroll_delta.y);
            tab.clamp_scroll();
            tab.sync_current_page();
            // Scrolling changes which pages are wanted, but not their scale, so
            // the token is left alone and existing textures stay valid.
        }
    }

    let _ = prefs;

    // --- pointer ---
    let Some(pointer) = response.interact_pointer_pos().or(response.hover_pos()) else {
        return ViewerAction::None;
    };
    let content = Vec2::new(
        pointer.x - canvas.min.x + tab.scroll.x,
        pointer.y - canvas.min.y + tab.scroll.y,
    );

    let hit = tab
        .layout
        .page_at(content)
        .map(|pb| (pb.index, pb.rect, pb.size));

    // Panning works with the Hand tool, with the middle button, and with space
    // held — all three are muscle memory from other readers.
    let space = ui.input(|i| i.key_down(egui::Key::Space));
    let middle = ui.input(|i| i.pointer.middle_down());
    if (tool == Tool::Pan || space || middle) && response.dragged() {
        let d = response.drag_delta();
        tab.scroll = tab.scroll - Vec2::new(d.x, d.y);
        tab.clamp_scroll();
        tab.sync_current_page();
        return ViewerAction::None;
    }

    let Some((page, page_rect, page_size)) = hit else {
        // A click on empty canvas clears the selection, which is how every
        // document view behaves.
        if response.clicked() {
            tab.selection = None;
            tab.selected_annotation = None;
        }
        return ViewerAction::None;
    };
    let t = PageTransform::new(page_rect, page_size, tab.rotation);
    let page_pt = t.content_to_page(content);
    // A hit tolerance in page units, so it stays a constant size on screen.
    let tol = 5.0 / t.scale().max(0.01);

    match tool {
        Tool::Select => handle_select(ui, tab, response, page, page_pt, tol),

        Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly => {
            let act = handle_select(ui, tab, response, page, page_pt, tol);
            // On release, turn whatever is selected into a markup annotation.
            if response.drag_stopped() || response.clicked() {
                if let Some(ann) = markup_from_selection(tab, tool) {
                    tab.selection = None;
                    return ViewerAction::CreateAnnotation(Box::new(ann));
                }
            }
            act
        }

        Tool::Note => {
            if response.clicked() {
                let id = tab.next_annot_id();
                let r = Rect::from_xywh(page_pt.x, page_pt.y - 18.0, 24.0, 18.0);
                let ann = Annotation::new(id, page, AnnotKind::Note {
                    icon: quark_core::annot::NoteIcon::Comment,
                }, r, &quark_core::tools::whoami());
                return ViewerAction::CreateAnnotation(Box::new(ann));
            }
            ViewerAction::None
        }

        Tool::Ink | Tool::Eraser => handle_ink(tab, response, page, page_pt, tool),

        Tool::Line | Tool::Arrow => {
            if response.drag_started() {
                tab.drafting = Drafting::Line {
                    page,
                    start: page_pt,
                    current: page_pt,
                };
            } else if response.dragged() {
                if let Drafting::Line { current, .. } = &mut tab.drafting {
                    *current = page_pt;
                }
            } else if response.drag_stopped() {
                if let Drafting::Line { start, current, .. } = tab.drafting.clone() {
                    tab.drafting = Drafting::None;
                    if (current - start).length() > 2.0 {
                        let id = tab.next_annot_id();
                        let mut ann = Annotation::new(
                            id,
                            page,
                            AnnotKind::Line {
                                start,
                                end: current,
                                start_ending: quark_core::annot::LineEnding::None,
                                end_ending: if tool == Tool::Arrow {
                                    quark_core::annot::LineEnding::OpenArrow
                                } else {
                                    quark_core::annot::LineEnding::None
                                },
                                measure: None,
                            },
                            Rect::new(start, current).normalize(),
                            &quark_core::tools::whoami(),
                        );
                        ann.recompute_rect();
                        return ViewerAction::CreateAnnotation(Box::new(ann));
                    }
                }
            }
            ViewerAction::None
        }

        t if t.is_polyline() => handle_vertices(tab, response, page, page_pt, t),

        t if t.is_drag_rect() => handle_drag_rect(tab, response, page, page_pt, t),

        _ => ViewerAction::None,
    }
}

/// Tools built from a sequence of clicked vertices, ended by a double-click.
///
/// A polygon needs at least three points and a polyline at least two; anything
/// shorter is discarded rather than written out as a degenerate shape.
fn handle_vertices(
    tab: &mut Tab,
    response: &egui::Response,
    page: usize,
    page_pt: Vec2,
    tool: Tool,
) -> ViewerAction {
    // Starting on a different page abandons the shape in progress: a polygon
    // spanning two pages has no meaning in the file format.
    if let Some(active) = tab.drafting.page() {
        if active != page {
            tab.drafting = Drafting::None;
        }
    }

    if let Drafting::Vertices { cursor, .. } = &mut tab.drafting {
        *cursor = page_pt;
    }

    if response.double_clicked() {
        let Drafting::Vertices { points, .. } = tab.drafting.clone() else {
            return ViewerAction::None;
        };
        tab.drafting = Drafting::None;

        let min_points = if tool == Tool::Polygon { 3 } else { 2 };
        if points.len() < min_points {
            return ViewerAction::None;
        }

        let id = tab.next_annot_id();
        let kind = match tool {
            Tool::Polygon | Tool::MeasureArea => AnnotKind::Polygon { points },
            _ => AnnotKind::Polyline {
                points,
                start_ending: quark_core::annot::LineEnding::None,
                end_ending: quark_core::annot::LineEnding::None,
            },
        };
        let mut ann = Annotation::new(id, page, kind, Rect::ZERO, &quark_core::tools::whoami());
        ann.recompute_rect();
        return ViewerAction::CreateAnnotation(Box::new(ann));
    }

    if response.clicked() {
        match &mut tab.drafting {
            Drafting::Vertices { points, .. } => points.push(page_pt),
            _ => {
                tab.drafting = Drafting::Vertices {
                    page,
                    points: vec![page_pt],
                    cursor: page_pt,
                }
            }
        }
    }

    ViewerAction::None
}

/// Text selection and annotation picking.
fn handle_select(
    ui: &mut Ui,
    tab: &mut Tab,
    response: &egui::Response,
    page: usize,
    page_pt: Vec2,
    tol: f32,
) -> ViewerAction {
    // --- dragging a selected annotation ---
    //
    // The gesture only starts on an annotation that is already selected, so a
    // drag beginning over a highlight still selects text; moving something
    // requires clicking it first. That is what stops the markup tools from
    // fighting with text selection.
    if let Some(id) = tab.selected_annotation {
        let over = tab
            .find_annotation(id)
            .is_some_and(|a| a.page == page && a.hit_test(page_pt, tol));

        let drag_id = egui::Id::new("quark-annot-drag");
        if response.drag_started() && over {
            ui.memory_mut(|m| m.data.insert_temp(drag_id, page_pt));
        }
        let anchor: Option<Vec2> = ui.memory(|m| m.data.get_temp::<Vec2>(drag_id));
        if let Some(from) = anchor {
            if response.dragged() {
                let delta = page_pt - from;
                if delta.length() > 0.01 {
                    ui.memory_mut(|m| m.data.insert_temp(drag_id, page_pt));
                    return ViewerAction::MoveAnnotation { id, delta };
                }
                return ViewerAction::None;
            }
            if response.drag_stopped() {
                ui.memory_mut(|m| m.data.remove::<Vec2>(drag_id));
                return ViewerAction::None;
            }
        }
    }

    // An annotation under the pointer takes priority over text: clicking a
    // sticky note should open it, not select the words behind it.
    if response.clicked() {
        let hit = tab
            .annotations_on(page)
            .filter(|a| a.hit_test(page_pt, tol))
            // Topmost wins, which for equal z-order means the smallest, so a
            // note inside a big rectangle is still reachable.
            .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
            .map(|a| a.id);
        if let Some(id) = hit {
            tab.selected_annotation = Some(id);
            tab.selection = None;
            return ViewerAction::None;
        }
        tab.selected_annotation = None;

        // A link under the pointer navigates. Links are read from the file and
        // are never `dirty`, so they are not drawn — only acted on.
        if let Some(target) = tab
            .links
            .iter()
            .find(|l| l.page == page && l.rect.contains(page_pt) && l.uri.is_none())
        {
            return ViewerAction::GoToPage(target.target_page);
        }

        // A field under the pointer activates it.
        if let Some(f) = tab
            .fields
            .iter()
            .find(|f| f.page == page && f.kind.is_fillable() && !f.read_only && f.rect.contains(page_pt))
        {
            return ViewerAction::ActivateField(f.name.clone());
        }
    }

    let Some(text) = tab.page_text.get(&page) else {
        return ViewerAction::None;
    };

    if response.double_clicked() {
        if let Some(i) = text.index_at(page_pt) {
            let (a, b) = text.expand_to_word(i.saturating_sub(1));
            tab.selection = Some(TextSelection {
                page,
                start: a,
                end: b,
            });
        }
        return ViewerAction::None;
    }

    if response.drag_started() || (response.clicked() && !ui.input(|i| i.modifiers.shift)) {
        if let Some(i) = text.index_at(page_pt) {
            tab.selection = Some(TextSelection {
                page,
                start: i,
                end: i,
            });
        }
    } else if response.dragged() {
        if let Some(i) = text.index_at(page_pt) {
            match &mut tab.selection {
                // Extending across pages is not supported, so a drag that
                // leaves the page keeps the anchor page's selection.
                Some(sel) if sel.page == page => sel.end = i,
                _ => {}
            }
        }
    }

    ViewerAction::None
}

/// Builds a text-markup annotation from the current selection.
fn markup_from_selection(tab: &mut Tab, tool: Tool) -> Option<Annotation> {
    let sel = tab.selection?;
    if sel.is_empty() {
        return None;
    }
    let text = tab.page_text.get(&sel.page)?;
    let (a, b) = sel.range();
    let quads = text.quads_for_range(a, b);
    if quads.is_empty() {
        return None;
    }
    let excerpt = text.slice(a, b);

    let kind = match tool {
        Tool::Highlight => AnnotKind::Highlight { quads },
        Tool::Underline => AnnotKind::Underline { quads },
        Tool::StrikeOut => AnnotKind::StrikeOut { quads },
        Tool::Squiggly => AnnotKind::Squiggly { quads },
        _ => return None,
    };

    let id = tab.next_annot_id();
    let mut ann = Annotation::new(id, sel.page, kind, Rect::ZERO, &quark_core::tools::whoami());
    ann.recompute_rect();
    // Recording the marked text makes the comments panel useful without having
    // to go back to the page for context.
    ann.subject = excerpt.chars().take(120).collect();
    Some(ann)
}

fn handle_ink(
    tab: &mut Tab,
    response: &egui::Response,
    page: usize,
    page_pt: Vec2,
    tool: Tool,
) -> ViewerAction {
    if tool == Tool::Eraser {
        if response.dragged() || response.clicked() {
            let tol = 6.0;
            if let Some(id) = tab
                .annotations_on(page)
                .find(|a| a.hit_test(page_pt, tol))
                .map(|a| a.id)
            {
                return ViewerAction::DeleteAnnotation(id);
            }
        }
        return ViewerAction::None;
    }

    if response.drag_started() {
        tab.drafting = Drafting::Ink {
            page,
            points: vec![page_pt],
        };
    } else if response.dragged() {
        if let Drafting::Ink { points, .. } = &mut tab.drafting {
            // Drop points that barely moved: a fast drag emits one per frame
            // and the stroke ends up with thousands of collinear vertices.
            let far_enough = points
                .last()
                .map(|p| (page_pt - *p).length() > 1.0)
                .unwrap_or(true);
            if far_enough {
                points.push(page_pt);
            }
        }
    } else if response.drag_stopped() {
        if let Drafting::Ink { points, .. } = tab.drafting.clone() {
            tab.drafting = Drafting::None;
            if points.len() > 1 {
                let id = tab.next_annot_id();
                let mut ann = Annotation::new(
                    id,
                    page,
                    AnnotKind::Ink {
                        strokes: vec![quark_core::annot::InkStroke { points }],
                    },
                    Rect::ZERO,
                    &quark_core::tools::whoami(),
                );
                ann.recompute_rect();
                return ViewerAction::CreateAnnotation(Box::new(ann));
            }
        }
    }
    ViewerAction::None
}

fn handle_drag_rect(
    tab: &mut Tab,
    response: &egui::Response,
    page: usize,
    page_pt: Vec2,
    tool: Tool,
) -> ViewerAction {
    if response.drag_started() {
        tab.drafting = Drafting::Rect {
            page,
            start: page_pt,
            current: page_pt,
        };
        return ViewerAction::None;
    }
    if response.dragged() {
        if let Drafting::Rect { current, .. } = &mut tab.drafting {
            *current = page_pt;
        }
        return ViewerAction::None;
    }
    if !response.drag_stopped() {
        return ViewerAction::None;
    }

    let Some(rect) = tab.drafting.rect() else {
        return ViewerAction::None;
    };
    tab.drafting = Drafting::None;
    // A stray click produces a zero-area rectangle; creating an invisible
    // annotation from it would be baffling.
    if rect.width() < 3.0 || rect.height() < 3.0 {
        return ViewerAction::None;
    }

    match tool {
        Tool::ZoomArea => ViewerAction::ZoomToRect { page, rect },
        Tool::Snapshot => ViewerAction::Snapshot { page, rect },
        Tool::Crop => ViewerAction::Crop { page, rect },
        Tool::Rectangle | Tool::Ellipse | Tool::Redact | Tool::FreeText | Tool::MeasureArea => {
            let id = tab.next_annot_id();
            let kind = match tool {
                Tool::Rectangle | Tool::MeasureArea => AnnotKind::Square,
                Tool::Ellipse => AnnotKind::Circle,
                Tool::Redact => AnnotKind::Redact {
                    overlay_text: None,
                    fill: QColor::BLACK,
                },
                Tool::FreeText => AnnotKind::FreeText {
                    text: String::new(),
                    font_size: 12.0,
                    font: "Helvetica".into(),
                    align: quark_core::annot::TextAlign::Left,
                    callout: None,
                },
                _ => unreachable!(),
            };
            let ann = Annotation::new(id, page, kind, rect, &quark_core::tools::whoami());
            ViewerAction::CreateAnnotation(Box::new(ann))
        }
        _ => ViewerAction::None,
    }
}

/// Applies a marquee zoom: fits `rect` to the viewport.
pub fn zoom_to_rect(tab: &mut Tab, page: usize, rect: Rect) {
    let Some(pb) = tab.layout.page(page) else { return };
    let t = PageTransform::new(pb.rect, pb.size, tab.rotation);
    let content = t.page_rect_to_content(rect);
    if content.width() < 1.0 || content.height() < 1.0 {
        return;
    }
    // The rect is in content pixels at the current scale, so the factor needed
    // is the ratio of the viewport to it.
    let factor = (tab.viewport.x / content.width()).min(tab.viewport.y / content.height());
    let new_scale = (tab.layout.scale * factor).clamp(layout::MIN_ZOOM, layout::MAX_ZOOM);

    // The centre of the marquee, as a fraction of the old content size.
    let centre = content.center();
    tab.push_history();
    tab.zoom = ZoomMode::Custom(new_scale);
    tab.invalidate_layout();
    tab.ensure_layout();
    let ratio = new_scale / (tab.layout.scale / factor).max(0.0001);
    let _ = ratio;
    // Recompute the centre at the new scale and put it in the middle.
    let scaled_centre = centre * (new_scale / (new_scale / factor));
    tab.scroll = scaled_centre - tab.viewport * 0.5;
    tab.clamp_scroll();
    tab.bump_token();
}


/// Length and position of a scrollbar thumb, in pixels along its track.
///
/// Pure so the arithmetic can be tested: a thumb that runs off the end of its
/// track, or that never reaches the end at full scroll, is hard to spot by eye
/// and obvious in a test.
fn thumb(track_len: f32, view: f32, content: f32, offset: f32) -> (f32, f32) {
    if content <= 0.0 || track_len <= 0.0 {
        return (track_len.max(0.0), 0.0);
    }
    // Proportional to how much of the content is visible, with a floor so a
    // thousand-page document still has something grabbable.
    let len = (track_len * (view / content)).clamp(MIN_THUMB.min(track_len), track_len);
    let travel = (track_len - len).max(0.0);
    let max_offset = (content - view).max(1.0);
    let t = (offset / max_offset).clamp(0.0, 1.0);
    (len, t * travel)
}


/// Thickness of a scrollbar, and how far it sits from the canvas edge.
const BAR: f32 = 9.0;
const BAR_PAD: f32 = 3.0;
/// A thumb shorter than this is impossible to grab.
const MIN_THUMB: f32 = 28.0;

/// Draws the canvas scrollbars and handles dragging them.
///
/// The viewer scrolls a custom offset rather than living inside an
/// `egui::ScrollArea`, so it gets no scrollbars for free. Without them a page
/// zoomed past the window edge can only be reached by wheel or drag, and
/// nothing on screen says there is more to the right.
///
/// Each axis appears only when the content actually overflows it.
fn scrollbars(ui: &mut Ui, tab: &mut Tab, canvas: ERect, p: &Palette) {
    let content = tab.layout.content_size;
    let view = tab.viewport;
    let over_x = content.x > view.x + 0.5;
    let over_y = content.y > view.y + 0.5;
    if !over_x && !over_y {
        return;
    }

    // The track stops short of the other bar so the two do not overlap in the
    // corner.
    let end_inset = |other: bool| if other { BAR + BAR_PAD * 2.0 } else { 0.0 };

    if over_x {
        let track = ERect::from_min_max(
            pos2(canvas.left() + BAR_PAD, canvas.bottom() - BAR - BAR_PAD),
            pos2(canvas.right() - BAR_PAD - end_inset(over_y), canvas.bottom() - BAR_PAD),
        );
        if let Some(offset) = bar(ui, "hscroll", track, true, tab.scroll.x, view.x, content.x, p) {
            tab.scroll.x = offset;
            tab.clamp_scroll();
        }
    }
    if over_y {
        let track = ERect::from_min_max(
            pos2(canvas.right() - BAR - BAR_PAD, canvas.top() + BAR_PAD),
            pos2(canvas.right() - BAR_PAD, canvas.bottom() - BAR_PAD - end_inset(over_x)),
        );
        if let Some(offset) = bar(ui, "vscroll", track, false, tab.scroll.y, view.y, content.y, p) {
            tab.scroll.y = offset;
            tab.clamp_scroll();
        }
    }
}

/// One scrollbar. Returns a new scroll offset when the user moved it.
#[allow(clippy::too_many_arguments)]
fn bar(
    ui: &mut Ui,
    id: &str,
    track: ERect,
    horizontal: bool,
    offset: f32,
    view: f32,
    content: f32,
    p: &Palette,
) -> Option<f32> {
    let track_len = if horizontal {
        track.width()
    } else {
        track.height()
    };
    let max_offset = (content - view).max(1.0);

    let (thumb_len, thumb_start) = thumb(track_len, view, content, offset);
    let travel = (track_len - thumb_len).max(0.0);

    let thumb = if horizontal {
        ERect::from_min_size(
            pos2(track.left() + thumb_start, track.top()),
            egui::vec2(thumb_len, track.height()),
        )
    } else {
        ERect::from_min_size(
            pos2(track.left(), track.top() + thumb_start),
            egui::vec2(track.width(), thumb_len),
        )
    };

    let response = ui.interact(
        track,
        ui.id().with(id),
        Sense::click_and_drag(),
    );
    let hovered = response.hovered() || response.dragged();

    let painter = ui.painter();
    let radius = CornerRadius::same((BAR / 2.0) as u8);
    // The track only appears on hover: a permanently visible one is a heavy
    // frame around a document that is meant to be the focus.
    if hovered {
        painter.rect_filled(track, radius, p.card_hover);
    }
    painter.rect_filled(
        thumb,
        radius,
        if hovered { p.accent } else { p.text_faint },
    );

    if response.dragged() || response.clicked() {
        let pointer = response.interact_pointer_pos()?;
        let along = if horizontal {
            pointer.x - track.left()
        } else {
            pointer.y - track.top()
        };
        // Centre the thumb on the pointer, so a click jumps to roughly where
        // the user pointed rather than to its top-left corner.
        let want = (along - thumb_len / 2.0).clamp(0.0, travel.max(0.0));
        if travel <= 0.0 {
            return None;
        }
        return Some((want / travel) * max_offset);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scroll_thumb_spans_the_track_when_nothing_overflows() {
        // A full-length thumb is the signal that there is nowhere to scroll.
        let (len, start) = thumb(200.0, 500.0, 500.0, 0.0);
        assert!((len - 200.0).abs() < 0.01, "len {len}");
        assert_eq!(start, 0.0);
    }

    #[test]
    fn the_thumb_reaches_the_end_of_the_track_at_full_scroll() {
        // Off-by-one here leaves a gap at the end that looks like the document
        // has more content the user cannot reach.
        let (len, start) = thumb(300.0, 400.0, 1200.0, 800.0);
        assert!((start + len - 300.0).abs() < 0.01, "start {start} len {len}");
    }

    #[test]
    fn the_thumb_starts_at_zero_when_scrolled_to_the_top() {
        let (_, start) = thumb(300.0, 400.0, 1200.0, 0.0);
        assert_eq!(start, 0.0);
    }

    #[test]
    fn the_thumb_is_proportional_to_how_much_is_visible() {
        // A quarter visible is a quarter-length thumb.
        let (len, _) = thumb(400.0, 250.0, 1000.0, 0.0);
        assert!((len - 100.0).abs() < 0.01, "len {len}");
    }

    #[test]
    fn a_very_long_document_still_has_a_grabbable_thumb() {
        // Strictly proportional, a 5000-page document would give a thumb under
        // a pixel tall.
        let (len, _) = thumb(400.0, 500.0, 5_000_000.0, 0.0);
        assert!(len >= 20.0, "thumb too small to grab: {len}");
    }

    #[test]
    fn the_thumb_never_runs_past_its_track() {
        // Scroll offsets can briefly exceed the maximum while a zoom is being
        // applied, before the clamp runs.
        for offset in [0.0, 500.0, 1_000.0, 99_999.0] {
            let (len, start) = thumb(300.0, 400.0, 1200.0, offset);
            assert!(
                start + len <= 300.01,
                "offset {offset} put the thumb at {start}+{len}"
            );
            assert!(start >= 0.0, "offset {offset} gave a negative start");
        }
    }

    #[test]
    fn degenerate_sizes_do_not_divide_by_zero() {
        // A document still loading reports zero content.
        let (len, start) = thumb(300.0, 0.0, 0.0, 0.0);
        assert!(len.is_finite() && start.is_finite(), "{len} {start}");
        let (len, start) = thumb(0.0, 100.0, 200.0, 10.0);
        assert!(len.is_finite() && start.is_finite(), "{len} {start}");
    }
    use quark_core::annot::AnnotId;

    #[test]
    fn opacity_is_folded_into_the_drawn_alpha() {
        let c = QColor::rgba(255, 0, 0, 255);
        assert_eq!(with_opacity(c, 0.5).a(), 127);
        assert_eq!(with_opacity(c, 1.0).a(), 255);
        assert_eq!(with_opacity(c, 0.0).a(), 0);
    }

    #[test]
    fn opacity_outside_the_valid_range_is_clamped() {
        let c = QColor::rgba(255, 0, 0, 255);
        assert_eq!(with_opacity(c, 5.0).a(), 255);
        assert_eq!(with_opacity(c, -1.0).a(), 0);
    }

    #[test]
    fn an_opaque_colour_converts_exactly() {
        let c = QColor::rgba(12, 34, 56, 255);
        let e = to_egui(c);
        assert_eq!((e.r(), e.g(), e.b(), e.a()), (12, 34, 56, 255));
    }

    #[test]
    fn a_translucent_colour_is_premultiplied_for_egui() {
        // `Color32` stores premultiplied components, which is what the painter
        // expects; handing it unmultiplied values makes every translucent
        // annotation draw too bright.
        let c = QColor::rgba(200, 100, 50, 128);
        let e = to_egui(c);
        assert_eq!(e.a(), 128, "alpha must survive");
        assert!(e.r() < 200, "red should be scaled by alpha, got {}", e.r());
        // Approximately c * a/255.
        let expected = (200.0_f32 * 128.0 / 255.0).round() as i32;
        assert!((e.r() as i32 - expected).abs() <= 1, "got {}", e.r());
    }

    #[test]
    fn a_viewer_action_defaults_to_doing_nothing() {
        assert_eq!(ViewerAction::None, ViewerAction::None);
        assert_ne!(
            ViewerAction::None,
            ViewerAction::DeleteAnnotation(AnnotId(1))
        );
    }
}
