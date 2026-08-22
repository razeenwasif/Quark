//! Quark's design system.
//!
//! The chrome is the same purple glass as Neutron, so the two read as one
//! family. The **document canvas is deliberately different**: a page is a large
//! white rectangle, and floating it over drifting coloured light makes the
//! light appear to bleed into the paper. So the canvas is a flat, neutral,
//! slightly cool grey — the colour a lightbox has — and the glass is kept to
//! the toolbars, panels and overlays around it.
//!
//! Contrast is checked against the canvas rather than the ground, because that
//! is what most of the chrome actually sits next to.

use egui::{Color32, CornerRadius, Stroke, Visuals};

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
    /// Window ground, behind everything.
    pub ground: Color32,
    /// The scrolling document canvas that pages sit on.
    pub canvas: Color32,
    /// Translucent panel fill — toolbars, side panels, the status bar.
    pub card: Color32,
    pub card_hover: Color32,
    /// Menus, popovers and dialogs, one step above a card.
    pub popover: Color32,
    /// Hairline between and around panels.
    pub border: Color32,
    /// Inset highlight along a panel's top edge.
    pub highlight: Color32,

    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,

    /// The brand purple, used for selection and active state.
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,

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
        ground: Color32::from_rgb(0x0c, 0x07, 0x14),
        // Distinctly lighter than the ground so a page reads as lit, but far
        // enough from white that the page itself is still the brightest thing
        // on screen.
        canvas: Color32::from_rgb(0x1a, 0x16, 0x24),
        card: Color32::from_rgba_unmultiplied_const(0x28, 0x1f, 0x3c, 0xd8),
        card_hover: Color32::from_rgba_unmultiplied_const(0x33, 0x28, 0x4a, 0xe8),
        popover: Color32::from_rgb(0x25, 0x1d, 0x36),
        border: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0x1c),
        highlight: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0x24),

        text: Color32::from_rgb(0xf2, 0xee, 0xfa),
        text_muted: Color32::from_rgb(0xb0, 0xa5, 0xc4),
        text_faint: Color32::from_rgb(0x7d, 0x72, 0x91),

        accent: Color32::from_rgb(0xa8, 0x55, 0xf7),
        accent_soft: Color32::from_rgba_unmultiplied_const(0xa8, 0x55, 0xf7, 0x3a),
        accent_text: Color32::from_rgb(0xc0, 0x84, 0xfc),

        warning: Color32::from_rgb(0xfb, 0xbf, 0x24),
        error: Color32::from_rgb(0xfb, 0x71, 0x85),
        success: Color32::from_rgb(0x4a, 0xde, 0x80),

        page_shadow: Color32::from_black_alpha(0x8c),
        page_border: Color32::from_rgba_unmultiplied_const(0x00, 0x00, 0x00, 0x60),
        selection: Color32::from_rgba_unmultiplied_const(0x3b, 0x82, 0xf6, 0x59),
        search_current: Color32::from_rgba_unmultiplied_const(0xf9, 0x73, 0x16, 0x99),
        search_other: Color32::from_rgba_unmultiplied_const(0xfb, 0xbf, 0x24, 0x66),
        field_highlight: Color32::from_rgba_unmultiplied_const(0x3b, 0x82, 0xf6, 0x33),
        field_required: Color32::from_rgba_unmultiplied_const(0xfb, 0x71, 0x85, 0x3d),
    };

    pub const LIGHT: Palette = Palette {
        ground: Color32::from_rgb(0xef, 0xea, 0xf6),
        // A shade darker than the page, so a white page still stands off it.
        canvas: Color32::from_rgb(0xd6, 0xd1, 0xe0),
        card: Color32::from_rgba_unmultiplied_const(0xfc, 0xfa, 0xff, 0xe6),
        card_hover: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0xf2),
        popover: Color32::from_rgb(0xff, 0xff, 0xff),
        border: Color32::from_rgba_unmultiplied_const(0x2a, 0x1b, 0x40, 0x24),
        highlight: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0xcc),

        text: Color32::from_rgb(0x1e, 0x16, 0x2c),
        text_muted: Color32::from_rgb(0x5c, 0x51, 0x70),
        text_faint: Color32::from_rgb(0x8b, 0x81, 0x9c),

        accent: Color32::from_rgb(0x93, 0x33, 0xea),
        accent_soft: Color32::from_rgba_unmultiplied_const(0x93, 0x33, 0xea, 0x2e),
        accent_text: Color32::from_rgb(0x7e, 0x22, 0xce),

        warning: Color32::from_rgb(0xb4, 0x53, 0x09),
        error: Color32::from_rgb(0xbe, 0x12, 0x3c),
        success: Color32::from_rgb(0x15, 0x80, 0x3d),

        page_shadow: Color32::from_black_alpha(0x33),
        page_border: Color32::from_rgba_unmultiplied_const(0x2a, 0x1b, 0x40, 0x33),
        selection: Color32::from_rgba_unmultiplied_const(0x3b, 0x82, 0xf6, 0x4d),
        search_current: Color32::from_rgba_unmultiplied_const(0xf9, 0x73, 0x16, 0x99),
        search_other: Color32::from_rgba_unmultiplied_const(0xfb, 0xbf, 0x24, 0x80),
        field_highlight: Color32::from_rgba_unmultiplied_const(0x3b, 0x82, 0xf6, 0x2e),
        field_required: Color32::from_rgba_unmultiplied_const(0xbe, 0x12, 0x3c, 0x2e),
    };

    pub const fn for_mode(mode: ThemeMode) -> Palette {
        match mode {
            ThemeMode::Dark => Palette::DARK,
            ThemeMode::Light => Palette::LIGHT,
        }
    }

    /// The colour a translucent card actually composites to over the canvas.
    ///
    /// Contrast has to be measured against this, not against the card's own
    /// nominal fill — a panel at 85% alpha is meaningfully different from the
    /// colour written in the table.
    pub fn card_over_canvas(&self) -> Color32 {
        composite(self.card, self.canvas)
    }
}

/// Composites a translucent foreground over an opaque background.
///
/// `Color32` stores premultiplied components, so the foreground term is added
/// as-is rather than being scaled by alpha a second time. Every translucent
/// token below is therefore built with
/// [`Color32::from_rgba_unmultiplied_const`], which does that multiplication —
/// writing the plain RGB into `from_rgba_premultiplied` instead produces a
/// colour brighter than its own alpha allows, and it renders as an additive
/// glow rather than as tinted glass.
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

pub const TOOLBAR_HEIGHT: f32 = 44.0;
pub const TAB_HEIGHT: f32 = 34.0;
pub const STATUS_HEIGHT: f32 = 26.0;
pub const ROW_HEIGHT: f32 = 30.0;
pub const GUTTER: f32 = 12.0;
pub const RADIUS_CARD: u8 = 14;
pub const RADIUS_CONTROL: u8 = 8;
pub const RADIUS_SMALL: u8 = 6;

/// Gap between pages in the document canvas.
pub const PAGE_GAP: f32 = 14.0;
/// Margin around the whole document.
pub const PAGE_MARGIN: f32 = 18.0;

pub fn card_shadow(p: &Palette) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 6],
        blur: 22,
        spread: 0,
        color: if p.ground == Palette::DARK.ground {
            Color32::from_black_alpha(0x66)
        } else {
            Color32::from_black_alpha(0x22)
        },
    }
}

/// A translucent glass panel.
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(egui::Margin::same(10))
        .shadow(card_shadow(p))
}

/// A menu or dialog surface. Opaque, because text over a translucent menu that
/// happens to sit on a white page is unreadable.
pub fn popover(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.popover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(egui::Margin::same(8))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(0x77),
        })
}

/// Paints the inset highlight along a panel's top edge that gives it thickness.
pub fn glass_highlight(painter: &egui::Painter, rect: egui::Rect, p: &Palette) {
    let inset = rect.shrink(1.0);
    painter.line_segment(
        [
            egui::pos2(inset.left() + 8.0, inset.top() + 0.5),
            egui::pos2(inset.right() - 8.0, inset.top() + 0.5),
        ],
        Stroke::new(1.0, p.highlight),
    );
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
    v.extreme_bg_color = composite(p.card, p.ground);
    v.faint_bg_color = p.card_hover;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent_text;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent_text);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(RADIUS_CARD);
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(0x77),
    };

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_muted);
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.corner_radius = CornerRadius::same(RADIUS_CONTROL);

    w.inactive.bg_fill = Color32::TRANSPARENT;
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.corner_radius = CornerRadius::same(RADIUS_CONTROL);

    w.hovered.bg_fill = p.card_hover;
    w.hovered.weak_bg_fill = p.card_hover;
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.corner_radius = CornerRadius::same(RADIUS_CONTROL);

    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = p.accent_soft;
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.corner_radius = CornerRadius::same(RADIUS_CONTROL);

    w.open.bg_fill = p.card_hover;
    w.open.weak_bg_fill = p.card_hover;
    w.open.fg_stroke = Stroke::new(1.0, p.text);

    ctx.set_visuals(v);

    // `all_styles_mut` rather than replacing one theme's style: egui keeps a
    // separate style per theme, and setting only the active one means the
    // spacing silently reverts the first time the user toggles light/dark.
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 5.0);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.scroll.bar_width = 11.0;
        style.spacing.scroll.floating = false;
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
    fn primary_text_meets_wcag_aa_on_a_glass_panel() {
        // Measured against what the panel actually composites to, not against
        // the nominal card colour.
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
    fn accent_text_is_readable_on_a_panel() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
            let r = contrast_ratio(p.accent_text, p.card_over_canvas());
            assert!(r >= 3.0, "{mode:?} accent text is only {r:.2}:1");
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
    fn the_canvas_is_never_brighter_than_a_page() {
        // A canvas lighter than the paper inverts the visual hierarchy.
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let p = Palette::for_mode(mode);
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
                ("accent_soft", p.accent_soft),
                ("page_shadow", p.page_shadow),
                ("page_border", p.page_border),
                ("selection", p.selection),
                ("search_current", p.search_current),
                ("search_other", p.search_other),
                ("field_highlight", p.field_highlight),
                ("field_required", p.field_required),
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
    fn toggling_the_theme_twice_returns_to_the_start() {
        assert_eq!(ThemeMode::Dark.toggled().toggled(), ThemeMode::Dark);
        assert!(ThemeMode::Dark.is_dark());
        assert!(!ThemeMode::Light.is_dark());
    }
}
