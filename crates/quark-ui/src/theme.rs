//! Quark's design system: Velvet.
//!
//! Soft, not flat. Every surface is a rounded island floating on a deep
//! ground, with a diffuse shadow under it and a one-pixel sheen along its top
//! edge — the light a velvet nap catches. Nothing has a square corner: pages,
//! thumbnails, controls, panels and the window itself are all rounded, on one
//! scale ([`RADIUS_PAGE`] up to [`RADIUS_DIALOG`]).
//!
//! Depth reads in three steps: the **canvas** is recessed below the ground,
//! **islands** (tools, pages, dock) sit on it, and **floating** surfaces
//! (menus, dialogs, toasts, the page capsule) hover above everything.
//!
//! The **page is paper, not chrome**. It stays white in both themes and its
//! own colours never follow the palette, or a dark theme would print light
//! text onto white paper.
//!
//! Contrast is checked against the panel fill a token actually sits on.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn toggled(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }
}

/// Semantic colour tokens, named by role rather than by appearance.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// The tool dock. An island like the others now, so it matches `card`.
    pub rail: Color32,
    /// Inactive icon in the tool dock.
    pub rail_icon: Color32,
    /// Window ground, behind every island.
    pub ground: Color32,
    /// The recessed well the pages sit in — the darkest surface, so a white
    /// page is unmistakably the brightest thing on screen.
    pub canvas: Color32,
    /// Island fill — the tool dock, the pages panel, the dock, home.
    pub card: Color32,
    /// Hover, and the raised segment of a segmented control.
    pub card_hover: Color32,
    /// Menus, popovers, dialogs, toasts and the active tab: one step above an
    /// island.
    pub popover: Color32,
    /// Hairline around an island. Translucent white, so it reads as an edge
    /// catching light rather than as a drawn outline.
    pub border: Color32,
    /// The sheen along an island's top edge.
    pub highlight: Color32,
    /// Fill for recessed controls: segment tracks, text fields, the composer.
    pub recess: Color32,

    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    /// A control that cannot be used right now. Deliberately below the text
    /// contrast floor: it is not meant to be read, only seen to be absent.
    pub text_disabled: Color32,

    /// Selection, the active tool, primary buttons.
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,
    /// Text and icons drawn on a solid `accent` fill.
    pub on_accent: Color32,

    pub warning: Color32,
    pub error: Color32,
    pub success: Color32,

    /// Drop shadow beneath a rendered page.
    pub page_shadow: Color32,
    /// Hairline around a rendered page, so a white page has an edge against a
    /// light canvas.
    pub page_border: Color32,
    /// Text selection wash drawn over the page.
    pub selection: Color32,
    /// The current search hit, distinct from the others.
    pub search_current: Color32,
    pub search_other: Color32,
    /// Tint over interactive form fields when field highlighting is on.
    pub field_highlight: Color32,
    pub field_required: Color32,
}

impl Palette {
    pub const DARK: Palette = Palette {
        rail: Color32::from_rgb(0x17, 0x15, 0x1d),
        rail_icon: Color32::from_rgb(0xaa, 0xa4, 0xb6),
        // Near-black with a violet cast; the cast is what makes the darks read
        // as plush rather than as a dead grey.
        ground: Color32::from_rgb(0x0e, 0x0d, 0x12),
        canvas: Color32::from_rgb(0x09, 0x08, 0x0c),

        card: Color32::from_rgb(0x17, 0x15, 0x1d),
        card_hover: Color32::from_rgb(0x24, 0x21, 0x2d),
        popover: Color32::from_rgb(0x1e, 0x1b, 0x26),
        border: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0x0f),
        highlight: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0x0c),
        recess: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 0x47),

        text: Color32::from_rgb(0xed, 0xe9, 0xf3),
        text_muted: Color32::from_rgb(0xaa, 0xa4, 0xb6),
        text_faint: Color32::from_rgb(0x85, 0x7f, 0x92),
        text_disabled: Color32::from_rgb(0x4c, 0x47, 0x57),

        // Amethyst. Solid fills carry dark text: white on this lilac is only
        // 3:1, the near-black is over 6:1.
        accent: Color32::from_rgb(0x9d, 0x7b, 0xff),
        accent_soft: Color32::from_rgba_unmultiplied_const(0x9d, 0x7b, 0xff, 0x29),
        accent_text: Color32::from_rgb(0xc6, 0xb2, 0xff),
        on_accent: Color32::from_rgb(0x12, 0x0e, 0x1c),

        warning: Color32::from_rgb(0xf1, 0xbd, 0x6a),
        error: Color32::from_rgb(0xf3, 0x8b, 0xa0),
        success: Color32::from_rgb(0x7a, 0xd7, 0xa8),

        page_shadow: Color32::from_black_alpha(0xa6),
        page_border: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 0x66),
        selection: Color32::from_rgba_unmultiplied_const(0x7c, 0x5c, 0xff, 0x4d),
        search_current: Color32::from_rgba_unmultiplied_const(0xf9, 0x73, 0x16, 0x99),
        search_other: Color32::from_rgba_unmultiplied_const(0xfb, 0xbf, 0x24, 0x66),
        field_highlight: Color32::from_rgba_unmultiplied_const(0x9d, 0x7b, 0xff, 0x33),
        field_required: Color32::from_rgba_unmultiplied_const(0xf3, 0x8b, 0xa0, 0x3d),
    };

    pub const LIGHT: Palette = Palette {
        rail: Color32::from_rgb(0xff, 0xff, 0xff),
        rail_icon: Color32::from_rgb(0x5f, 0x5a, 0x6b),
        ground: Color32::from_rgb(0xee, 0xed, 0xf2),
        // A shade darker than the ground, recessed the same way the dark well
        // is, and still far enough below white for a page to stand off it.
        canvas: Color32::from_rgb(0xe3, 0xe1, 0xe9),
        card: Color32::from_rgb(0xff, 0xff, 0xff),
        card_hover: Color32::from_rgb(0xf1, 0xef, 0xf6),
        popover: Color32::from_rgb(0xff, 0xff, 0xff),
        border: Color32::from_rgba_unmultiplied_const(0x2a, 0x1b, 0x50, 0x1a),
        highlight: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0x00),
        recess: Color32::from_rgba_unmultiplied_const(0x2a, 0x1b, 0x50, 0x0f),

        text: Color32::from_rgb(0x18, 0x16, 0x1f),
        text_muted: Color32::from_rgb(0x5f, 0x5a, 0x6b),
        text_faint: Color32::from_rgb(0x72, 0x6d, 0x7e),
        text_disabled: Color32::from_rgb(0xbd, 0xb9, 0xc6),

        accent: Color32::from_rgb(0x7c, 0x5c, 0xe0),
        accent_soft: Color32::from_rgba_unmultiplied_const(0x7c, 0x5c, 0xe0, 0x24),
        accent_text: Color32::from_rgb(0x63, 0x42, 0xc9),
        on_accent: Color32::from_rgb(0xff, 0xff, 0xff),

        warning: Color32::from_rgb(0xb4, 0x53, 0x09),
        error: Color32::from_rgb(0xbe, 0x12, 0x3c),
        success: Color32::from_rgb(0x15, 0x80, 0x3d),

        page_shadow: Color32::from_black_alpha(0x2e),
        page_border: Color32::from_rgba_unmultiplied_const(0x2a, 0x1b, 0x40, 0x2b),
        selection: Color32::from_rgba_unmultiplied_const(0x7c, 0x5c, 0xe0, 0x40),
        search_current: Color32::from_rgba_unmultiplied_const(0xf9, 0x73, 0x16, 0x99),
        search_other: Color32::from_rgba_unmultiplied_const(0xfb, 0xbf, 0x24, 0x80),
        field_highlight: Color32::from_rgba_unmultiplied_const(0x7c, 0x5c, 0xe0, 0x24),
        field_required: Color32::from_rgba_unmultiplied_const(0xbe, 0x12, 0x3c, 0x2e),
    };

    pub const fn for_mode(mode: ThemeMode) -> Palette {
        match mode {
            ThemeMode::Dark => Palette::DARK,
            ThemeMode::Light => Palette::LIGHT,
        }
    }

    pub fn is_dark(&self) -> bool {
        luminance(self.ground) < 0.2
    }

    /// The colour a translucent card actually composites to over the canvas.
    ///
    /// Contrast has to be measured against this, not against the card's own
    /// nominal fill — a panel at 85% alpha is meaningfully different from the
    /// colour written in the table.
    pub fn card_over_canvas(&self) -> Color32 {
        composite(self.card, self.canvas)
    }

    /// The accent at a given opacity, for rings and glows.
    pub fn accent_alpha(&self, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(self.accent.r(), self.accent.g(), self.accent.b(), alpha)
    }

    /// A translucent wash of the text colour: white in the dark theme, ink in
    /// the light one. Hover fills and secondary buttons are built from it so
    /// they lighten a dark surface and darken a light one.
    pub fn wash(&self, alpha: u8) -> Color32 {
        if self.is_dark() {
            Color32::from_white_alpha(alpha)
        } else {
            Color32::from_rgba_unmultiplied(0x2a, 0x1b, 0x50, alpha / 2)
        }
    }
}

/// Composites a translucent foreground over an opaque background.
///
/// `Color32` stores premultiplied components, so the foreground term is added
/// as-is rather than being scaled by alpha a second time. Every translucent
/// token above is therefore built with
/// [`Color32::from_rgba_unmultiplied_const`], which does that multiplication —
/// writing the plain RGB into `from_rgba_premultiplied` instead produces a
/// colour brighter than its own alpha allows, and it renders as an additive
/// glow rather than as a tint.
pub fn composite(fg: Color32, bg: Color32) -> Color32 {
    let a = fg.a() as f32 / 255.0;
    let mix = |f: u8, b: u8| ((f as f32) + (b as f32) * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    // `Color32` is premultiplied, so the foreground term is not scaled again.
    Color32::from_rgb(mix(fg.r(), bg.r()), mix(fg.g(), bg.g()), mix(fg.b(), bg.b()))
}

/// Relative luminance, per WCAG 2.1.
pub fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio between two opaque colours, from 1.0 to 21.0.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// --- metrics ---

/// The row across the top of the window: brand, tabs, command search.
pub const TITLE_BAR_HEIGHT: f32 = 52.0;
/// Space left for the macOS traffic lights, which sit over the title bar once
/// the content is drawn under a transparent titlebar.
pub const TRAFFIC_LIGHT_INSET: f32 = 78.0;
/// The header row at the top of the canvas well: file identity and actions.
pub const TOOLBAR_HEIGHT: f32 = 52.0;
pub const ROW_HEIGHT: f32 = 30.0;
pub const GUTTER: f32 = 12.0;

/// Gap between islands, and between an island and the window edge.
pub const GAP: f32 = 10.0;

// The radius scale. Every corner in Quark is one of these.
/// Pages and keyboard keys — rounded, but only just, so paper still reads as
/// paper.
pub const RADIUS_PAGE: u8 = 6;
pub const RADIUS_SMALL: u8 = 8;
pub const RADIUS_THUMB: u8 = 10;
/// Buttons, tabs, menu rows.
pub const RADIUS_CONTROL: u8 = 12;
/// Cards, list rows, text fields, menus.
pub const RADIUS_CARD: u8 = 16;
/// The islands.
pub const RADIUS_ISLAND: u8 = 20;
/// Dialogs and the command palette.
pub const RADIUS_DIALOG: u8 = 24;

/// Gap between pages in the document canvas.
pub const PAGE_GAP: f32 = 16.0;
/// Margin around the whole document.
pub const PAGE_MARGIN: f32 = 24.0;

/// Width of the tool dock.
pub const RAIL_WIDTH: f32 = 56.0;
/// Default width of the pages island, margins included.
pub const LIST_WIDTH: f32 = 258.0;
/// Default width of the right-hand dock that holds Comments and the Assistant,
/// margins included.
pub const DOCK_WIDTH: f32 = 330.0;
/// The dock is resizable. Below this the comment text wraps to two or three
/// words a line and stops being readable; above it the document loses more
/// width than a side panel is worth.
pub const DOCK_MIN_WIDTH: f32 = 280.0;
pub const DOCK_MAX_WIDTH: f32 = 520.0;

// --- type ---

/// Manrope, the one UI typeface, shipped as its variable font.
const MANROPE: &[u8] = include_bytes!("../assets/fonts/Manrope-Variable.ttf");

/// Family name of the semibold cut, for labels, tabs and titles.
pub const STRONG: &str = "manrope-strong";
/// Family name of the bold cut, for headings.
pub const HEAVY: &str = "manrope-heavy";

/// Body text: Manrope Medium.
pub fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Labels, tabs, buttons: Manrope SemiBold.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// Headings: Manrope Bold.
pub fn heavy(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(HEAVY.into()))
}

/// Installs Manrope as the proportional font, plus its heavier cuts.
///
/// One variable font, instanced three times at different weights: egui picks
/// a font per family rather than per weight, so each weight is its own family.
/// egui's own fonts stay behind each as fallbacks for the glyphs Manrope lacks
/// (symbols, CJK, emoji).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let at = |weight: f32| {
        egui::FontData::from_static(MANROPE).tweak(egui::FontTweak {
            coords: egui::epaint::text::VariationCoords::new([(b"wght", weight)]),
            ..Default::default()
        })
    };
    fonts.font_data.insert("manrope-500".into(), Arc::new(at(500.0)));
    fonts.font_data.insert("manrope-620".into(), Arc::new(at(620.0)));
    fonts.font_data.insert("manrope-720".into(), Arc::new(at(720.0)));

    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let family = |first: &str| {
        std::iter::once(first.to_string())
            .chain(fallbacks.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts.families.insert(FontFamily::Proportional, family("manrope-500"));
    fonts.families.insert(FontFamily::Name(STRONG.into()), family("manrope-620"));
    fonts.families.insert(FontFamily::Name(HEAVY.into()), family("manrope-720"));
    ctx.set_fonts(fonts);
}

// --- surfaces ---

/// Shadow under an island: wide and soft, so the edge dissolves rather than
/// casting.
pub fn island_shadow(p: &Palette) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: if p.is_dark() {
            Color32::from_black_alpha(0x5a)
        } else {
            Color32::from_rgba_unmultiplied(0x2a, 0x1b, 0x50, 0x14)
        },
    }
}

/// Shadow under a floating surface — menus, dialogs, toasts, the capsule.
pub fn float_shadow(p: &Palette) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 18],
        blur: 48,
        spread: 0,
        color: if p.is_dark() {
            Color32::from_black_alpha(0x99)
        } else {
            Color32::from_rgba_unmultiplied(0x2a, 0x1b, 0x50, 0x26)
        },
    }
}

/// Kept for callers that predate the island split.
pub fn card_shadow(p: &Palette) -> egui::epaint::Shadow {
    island_shadow(p)
}

/// An island: the rounded, shadowed slab every panel is drawn as.
///
/// The caller adds the outer margin, which is what spaces islands apart.
pub fn island(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_ISLAND))
        .shadow(island_shadow(p))
        .inner_margin(egui::Margin::same(10))
}

/// A bordered surface inside an island — a comment, a card in a list.
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.wash(0x0a))
        .stroke(Stroke::new(1.0, p.wash(0x0a)))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(egui::Margin::same(12))
}

/// A recessed field: segment tracks, text inputs, the composer.
pub fn recess(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.recess)
        .stroke(Stroke::new(1.0, p.wash(0x0a)))
        .corner_radius(CornerRadius::same(15))
        .inner_margin(egui::Margin::same(4))
}

/// A menu or popover surface. Opaque, because text over a translucent menu
/// that happens to sit on a white page is unreadable.
pub fn popover(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.popover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_CARD + 2))
        .inner_margin(egui::Margin::same(6))
        .shadow(float_shadow(p))
}

/// A dialog, or the command palette.
pub fn dialog(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.popover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_DIALOG))
        .inner_margin(egui::Margin::same(22))
        .shadow(float_shadow(p))
}

/// The scrim behind a modal.
pub fn scrim(p: &Palette) -> Color32 {
    if p.is_dark() {
        Color32::from_black_alpha(0x8c)
    } else {
        Color32::from_rgba_unmultiplied(0x18, 0x10, 0x30, 0x40)
    }
}

/// Paints an island: its shadow, its fill and edge, and the sheen on top.
///
/// For surfaces whose container would clip a frame's own shadow — panels clip
/// to their rect, so a shadow drawn by the panel's frame is cut off a few
/// pixels outside the island. Pass an unclipped painter.
pub fn paint_island(painter: &egui::Painter, rect: egui::Rect, p: &Palette) {
    let radius = CornerRadius::same(RADIUS_ISLAND);
    painter.add(island_shadow(p).as_shape(rect, radius));
    painter.rect(rect, radius, p.card, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    paint_sheen(painter, rect, RADIUS_ISLAND as f32, p);
}

/// Paints the sheen along the top edge of a rounded surface.
///
/// Inset by the corner radius at each end, so it reads as light on the flat
/// of the top rather than as a line that runs off round the corners.
pub fn paint_sheen(painter: &egui::Painter, rect: egui::Rect, radius: f32, p: &Palette) {
    if p.highlight.a() == 0 {
        return;
    }
    let y = rect.top() + 1.0;
    painter.line_segment(
        [
            egui::pos2(rect.left() + radius, y),
            egui::pos2(rect.right() - radius, y),
        ],
        Stroke::new(1.0, p.highlight),
    );
}

/// Rounds the corners of a rectangular area that content was clipped to, by
/// painting `behind` into each corner outside the rounding.
///
/// egui clips to rectangles, so a page scrolled into the corner of the canvas
/// well would otherwise poke a square corner out of the rounded one. Each
/// mask is a fan from the corner point to the arc, which covers exactly the
/// region between the two edges and the curve.
pub fn mask_corners(painter: &egui::Painter, rect: egui::Rect, radius: f32, behind: Color32) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    if r <= 0.0 {
        return;
    }
    let corners = [
        (rect.left_top(), egui::pos2(rect.left() + r, rect.top() + r), PI),
        (rect.right_top(), egui::pos2(rect.right() - r, rect.top() + r), PI + FRAC_PI_2),
        (rect.right_bottom(), egui::pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (rect.left_bottom(), egui::pos2(rect.left() + r, rect.bottom() - r), FRAC_PI_2),
    ];
    const STEPS: u32 = 10;
    let mut mesh = egui::Mesh::default();
    for (corner, centre, start) in corners {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(corner, behind);
        for i in 0..=STEPS {
            let a = start + FRAC_PI_2 * (i as f32 / STEPS as f32);
            mesh.colored_vertex(centre + egui::vec2(a.cos(), a.sin()) * r, behind);
        }
        for i in 0..STEPS {
            mesh.add_triangle(base, base + 1 + i, base + 2 + i);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Paints the window ground: the fill, and the soft light that falls on it
/// from above the title bar.
pub fn paint_ground(painter: &egui::Painter, rect: egui::Rect, p: &Palette) {
    painter.rect_filled(rect, CornerRadius::ZERO, p.ground);
    // A fan from a point above the window to an ellipse round it: opaque-ish
    // at the centre, clear at the rim. Vertex colours do the falloff, so the
    // whole sheen is one mesh and costs nothing to repaint.
    let glow = if p.is_dark() {
        Color32::from_rgba_unmultiplied(0x90, 0x7b, 0xc6, 0x1c)
    } else {
        Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 0x80)
    };
    let centre = egui::pos2(rect.center().x, rect.top() - rect.height() * 0.08);
    let (rx, ry) = (rect.width() * 0.62, rect.height() * 0.62);
    const STEPS: u32 = 48;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(centre, glow);
    for i in 0..=STEPS {
        let a = std::f32::consts::TAU * (i as f32 / STEPS as f32);
        mesh.colored_vertex(
            centre + egui::vec2(a.cos() * rx, a.sin() * ry),
            Color32::TRANSPARENT,
        );
    }
    for i in 0..STEPS {
        mesh.add_triangle(0, 1 + i, 2 + i);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The velvet nap: a small tile of soft grain, laid over the ground.
///
/// Deterministic, so the texture is identical every launch, and tiled with a
/// repeat sampler so it costs one quad however large the window is.
pub fn grain_image(p: &Palette) -> egui::ColorImage {
    const N: usize = 128;
    let strength = if p.is_dark() { 9.0 } else { 6.0 };
    let mut px = Vec::with_capacity(N * N);
    let mut state = 0x9e37_79b9_u32;
    for _ in 0..N * N {
        // xorshift: cheap, stable, and plenty random for grain.
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let v = (state >> 24) as f32 / 255.0;
        let a = (v * strength).round() as u8;
        px.push(if p.is_dark() {
            Color32::from_white_alpha(a)
        } else {
            Color32::from_black_alpha(a)
        });
    }
    egui::ColorImage::new([N, N], px)
}

/// Applies the palette to an egui context.
pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let p = Palette::for_mode(mode);
    let mut v = if mode.is_dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    v.panel_fill = p.ground;
    v.window_fill = p.popover;
    v.extreme_bg_color = composite(p.recess, p.card);
    v.text_edit_bg_color = Some(composite(p.recess, p.card));
    v.faint_bg_color = p.card_hover;
    v.code_bg_color = p.card_hover;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.text_faint);
    v.hyperlink_color = p.accent_text;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent_text);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(RADIUS_DIALOG);
    v.window_shadow = float_shadow(&p);
    v.menu_corner_radius = CornerRadius::same(RADIUS_CARD + 2);
    v.popup_shadow = float_shadow(&p);
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);

    let control = CornerRadius::same(RADIUS_CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_muted);
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.corner_radius = control;

    // A button at rest is a soft wash rather than an outline; menus clear the
    // wash themselves, so menu rows still read as rows.
    w.inactive.bg_fill = p.recess;
    w.inactive.weak_bg_fill = p.wash(0x10);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.corner_radius = control;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = p.card_hover;
    w.hovered.weak_bg_fill = p.wash(0x18);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.corner_radius = control;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = p.accent_soft;
    w.active.fg_stroke = Stroke::new(1.0, p.accent_text);
    w.active.bg_stroke = Stroke::NONE;
    w.active.corner_radius = control;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.card_hover;
    w.open.weak_bg_fill = p.wash(0x14);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.bg_stroke = Stroke::NONE;
    w.open.corner_radius = control;

    ctx.set_visuals(v);

    // `all_styles_mut` rather than replacing one theme's style: egui keeps a
    // separate style per theme, and setting only the active one means the
    // spacing silently reverts the first time the user toggles light/dark.
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.window_margin = egui::Margin::same(22);
        style.spacing.menu_spacing = 4.0;
        style.spacing.combo_height = 320.0;
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating_allocated_width = 0.0;
        style.text_styles.insert(egui::TextStyle::Body, body(13.0));
        style.text_styles.insert(egui::TextStyle::Button, strong(13.0));
        style.text_styles.insert(egui::TextStyle::Heading, heavy(18.0));
        style.text_styles.insert(egui::TextStyle::Small, body(11.0));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositing_an_opaque_colour_returns_it_unchanged() {
        let c = Color32::from_rgb(10, 20, 30);
        assert_eq!(composite(c, Color32::WHITE), c);
    }

    #[test]
    fn compositing_a_fully_transparent_colour_returns_the_backdrop() {
        let clear = Color32::from_rgba_unmultiplied_const(0, 0, 0, 0);
        let bg = Color32::from_rgb(90, 40, 120);
        assert_eq!(composite(clear, bg), bg);
    }

    #[test]
    fn contrast_ratio_matches_the_known_extremes() {
        // Black on white is the maximum the scale defines.
        let r = contrast_ratio(Color32::BLACK, Color32::WHITE);
        assert!((r - 21.0).abs() < 0.1, "got {r}");
        // A colour against itself is the minimum.
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
    }

    #[test]
    fn contrast_is_symmetric() {
        let a = Color32::from_rgb(30, 30, 30);
        let b = Color32::from_rgb(200, 200, 200);
        assert!((contrast_ratio(a, b) - contrast_ratio(b, a)).abs() < 1e-4);
    }

    #[test]
    fn primary_text_meets_wcag_aa_on_a_panel() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let bg = p.card_over_canvas();
            let r = contrast_ratio(p.text, bg);
            assert!(r >= 4.5, "{mode:?} body text is only {r:.2}:1");
        }
    }

    #[test]
    fn muted_text_meets_the_large_text_threshold() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let bg = p.card_over_canvas();
            let r = contrast_ratio(p.text_muted, bg);
            assert!(r >= 3.0, "{mode:?} muted text is only {r:.2}:1");
        }
    }

    #[test]
    fn faint_text_is_still_readable_on_a_panel() {
        // Faint carries real information — timestamps, page numbers, folders —
        // so it has to clear body-text contrast on the island it sits on.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let r = contrast_ratio(p.text_faint, p.card);
            assert!(r >= 4.5, "{mode:?} faint text is only {r:.2}:1");
        }
    }

    #[test]
    fn accent_text_is_readable_on_a_panel() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let r = contrast_ratio(p.accent_text, p.card_over_canvas());
            assert!(r >= 3.0, "{mode:?} accent text is only {r:.2}:1");
        }
    }

    #[test]
    fn text_on_a_solid_accent_fill_is_readable() {
        // Primary buttons and the send button put `on_accent` on `accent`.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let r = contrast_ratio(p.on_accent, p.accent);
            assert!(r >= 4.5, "{mode:?} primary button text is only {r:.2}:1");
        }
    }

    #[test]
    fn status_colours_are_readable_on_a_panel() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let bg = p.card_over_canvas();
            for (name, c) in [
                ("warning", p.warning),
                ("error", p.error),
                ("success", p.success),
            ] {
                let r = contrast_ratio(c, bg);
                assert!(r >= 3.0, "{mode:?} {name} is only {r:.2}:1");
            }
        }
    }

    #[test]
    fn a_white_page_stands_off_the_canvas_in_both_themes() {
        // Without this the page edge disappears and the document stops looking
        // like a sheet of paper.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let r = contrast_ratio(Color32::WHITE, p.canvas);
            assert!(r >= 1.2, "{mode:?} page/canvas separation is only {r:.2}:1");
        }
    }

    #[test]
    fn the_canvas_is_recessed_below_the_islands() {
        // The well reads as sunk only if it is darker than the panels around
        // it, in both themes.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            assert!(
                luminance(p.canvas) < luminance(p.card),
                "{mode:?} canvas is not below the panels"
            );
            assert!(
                luminance(p.canvas) < luminance(Color32::WHITE),
                "{mode:?} canvas outshines the page"
            );
        }
    }

    #[test]
    fn selection_and_search_washes_stay_translucent() {
        // An opaque wash hides the text it is meant to mark.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            for (name, c) in [
                ("selection", p.selection),
                ("current hit", p.search_current),
                ("other hits", p.search_other),
                ("field highlight", p.field_highlight),
            ] {
                assert!(c.a() < 200, "{mode:?} {name} is too opaque ({})", c.a());
                assert!(c.a() > 20, "{mode:?} {name} is too faint ({})", c.a());
            }
        }
    }

    #[test]
    fn every_translucent_token_respects_the_premultiplied_invariant() {
        // In a premultiplied colour no channel may exceed the alpha, because
        // the channel is already scaled by it. A token written with plain RGB
        // into `from_rgba_premultiplied` violates this and renders as an
        // additive glow — which is what made the active toolbar button a solid
        // block with an invisible icon.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            for (name, c) in [
                ("card", p.card),
                ("card_hover", p.card_hover),
                ("border", p.border),
                ("highlight", p.highlight),
                ("recess", p.recess),
                ("accent_soft", p.accent_soft),
                ("page_shadow", p.page_shadow),
                ("page_border", p.page_border),
                ("selection", p.selection),
                ("search_current", p.search_current),
                ("search_other", p.search_other),
                ("field_highlight", p.field_highlight),
                ("field_required", p.field_required),
                ("wash", p.wash(0x10)),
                ("accent ring", p.accent_alpha(0x80)),
            ] {
                let a = c.a();
                assert!(
                    c.r() <= a && c.g() <= a && c.b() <= a,
                    "{mode:?} {name} is not premultiplied: rgb({},{},{}) exceeds alpha {a}",
                    c.r(),
                    c.g(),
                    c.b()
                );
            }
        }
    }

    #[test]
    fn the_active_toolbar_state_keeps_its_icon_readable() {
        // The active tool is drawn as `accent_text` over `accent_soft` on a
        // card; if the fill is too strong the icon disappears into it.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let fill = composite(p.accent_soft, p.card_over_canvas());
            let r = contrast_ratio(p.accent_text, fill);
            assert!(r >= 3.0, "{mode:?} active tool icon is only {r:.2}:1");
        }
    }

    #[test]
    fn the_radius_scale_only_grows() {
        // A control rounder than the card it sits in looks like a mistake.
        let scale = [
            RADIUS_PAGE,
            RADIUS_SMALL,
            RADIUS_THUMB,
            RADIUS_CONTROL,
            RADIUS_CARD,
            RADIUS_ISLAND,
            RADIUS_DIALOG,
        ];
        assert!(scale.windows(2).all(|w| w[0] < w[1]), "{scale:?}");
        assert!(scale.iter().all(|&r| r > 0), "no square corners anywhere");
    }

    #[test]
    fn the_grain_tile_is_square_and_faint() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let img = grain_image(&Palette::for_mode(mode));
            assert_eq!(img.size[0], img.size[1]);
            assert!(img.pixels.iter().all(|c| c.a() < 16), "{mode:?} grain is too strong");
        }
    }

    #[test]
    fn the_dock_default_sits_inside_its_own_bounds() {
        // A default outside the range gets silently clamped on first launch,
        // which looks like the panel resizing itself for no reason.
        assert!(DOCK_MIN_WIDTH < DOCK_MAX_WIDTH);
        assert!((DOCK_MIN_WIDTH..=DOCK_MAX_WIDTH).contains(&DOCK_WIDTH));
    }

    #[test]
    fn toggling_the_theme_twice_returns_to_the_start() {
        assert_eq!(ThemeMode::Dark.toggled().toggled(), ThemeMode::Dark);
        assert!(ThemeMode::Dark.is_dark());
        assert!(!ThemeMode::Light.is_dark());
    }
}
