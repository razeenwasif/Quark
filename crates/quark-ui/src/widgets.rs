//! Shared widgets.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2, vec2};

use crate::icons::{self, Icon};
use crate::theme::{self, Palette, RADIUS_CONTROL};

/// A square icon button, the size the header rows and composers use.
///
/// `active` draws the accent state, which is how a toggled control is shown.
pub fn tool_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    tooltip: &str,
    active: bool,
    enabled: bool,
) -> Response {
    icon_button(ui, p, icon, tooltip, 32.0, active, enabled)
}

/// An icon button at any size. The icon is half the button, so a larger
/// button gets a larger target without a heavier glyph.
pub fn icon_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    tooltip: &str,
    size: f32,
    active: bool,
    enabled: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let radius = CornerRadius::same((size * 0.36).round() as u8);

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && enabled;
        let painter = ui.painter();

        if active {
            painter.rect_filled(rect, radius, p.wash(0x12));
        } else if hovered {
            painter.rect_filled(rect, radius, p.wash(0x0d));
        }

        let color = if !enabled {
            // Dimmer rather than a different hue: a greyed control should read
            // as unavailable, not as a different kind of control.
            p.text_disabled
        } else if active || hovered {
            p.text
        } else {
            p.text_muted
        };
        let glyph = (size * 0.52).round();
        icons::draw(painter, Rect::from_center_size(rect.center(), Vec2::splat(glyph)), icon, color);
    }

    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
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

/// A secondary button: a soft wash at rest, accent when `active`.
pub fn text_button(ui: &mut Ui, p: &Palette, label: &str, active: bool) -> Response {
    let (fill, hover, colour) = if active {
        (p.accent_soft, p.accent_alpha(0x38), p.accent_text)
    } else {
        (p.wash(0x10), p.wash(0x18), p.text)
    };
    labelled_button(ui, label, 30.0, 14.0, fill, hover, colour, true)
}

/// A button with no fill until it is hovered, for low-emphasis actions.
pub fn ghost_button(ui: &mut Ui, p: &Palette, label: &str) -> Response {
    labelled_button(ui, label, 28.0, 10.0, Color32::TRANSPARENT, p.wash(0x0d), p.text_muted, true)
}

/// The one emphasised action in a group: a solid accent fill.
pub fn primary_button(ui: &mut Ui, p: &Palette, label: &str, enabled: bool) -> Response {
    let (fill, colour) = if enabled {
        (p.accent, p.on_accent)
    } else {
        (p.wash(0x0a), p.text_disabled)
    };
    let hover = if enabled {
        theme::composite(Color32::from_white_alpha(0x1a), p.accent)
    } else {
        fill
    };
    labelled_button(ui, label, 34.0, 18.0, fill, hover, colour, enabled)
}

/// An irreversible action, filled with the error colour.
pub fn danger_button(ui: &mut Ui, p: &Palette, label: &str) -> Response {
    let hover = theme::composite(Color32::from_white_alpha(0x1a), p.error);
    let colour = if p.is_dark() {
        Color32::from_rgb(0x2a, 0x0d, 0x14)
    } else {
        Color32::WHITE
    };
    labelled_button(ui, label, 34.0, 18.0, p.error, hover, colour, true)
}

#[allow(clippy::too_many_arguments)]
fn labelled_button(
    ui: &mut Ui,
    label: &str,
    height: f32,
    pad: f32,
    fill: Color32,
    hover: Color32,
    colour: Color32,
    enabled: bool,
) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::strong(12.5), colour);
    let size = vec2(galley.size().x + pad * 2.0, height);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(RADIUS_CONTROL);
        let bg = if response.hovered() && enabled { hover } else { fill };
        if bg.a() > 0 {
            painter.rect_filled(rect, radius, bg);
        }
        let pos = rect.center() - galley.size() / 2.0;
        painter.galley(pos, galley, colour);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A thin vertical rule between toolbar groups.
pub fn separator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 18.0), Sense::hover());
    let x = rect.center().x.round();
    ui.painter().rect_filled(
        Rect::from_min_max(egui::pos2(x - 0.5, rect.top()), egui::pos2(x + 0.5, rect.bottom())),
        CornerRadius::same(1),
        p.wash(0x16),
    );
}

/// A small header above a group inside a side panel.
pub fn panel_header(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(micro_caps(title))
            .font(theme::strong(10.5))
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
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let c = rect.center();
        painter.circle_filled(c, 8.0, color);
        // The ring sits outside a gap, so a dark colour still shows a visible
        // selection against a dark panel.
        if selected {
            painter.circle_stroke(c, 11.0, Stroke::new(1.5, p.text_muted));
        } else if response.hovered() {
            painter.circle_stroke(c, 11.0, Stroke::new(1.0, p.wash(0x20)));
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row in a list panel, with hover and selection states.
pub fn list_row(ui: &mut Ui, p: &Palette, height: f32, selected: bool) -> (Rect, Response) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(RADIUS_CONTROL);
        if selected {
            painter.rect_filled(rect, radius, p.accent_soft);
        } else if response.hovered() {
            painter.rect_filled(rect, radius, p.wash(0x0d));
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

/// A tool button in the tool dock.
///
/// The active tool glows faintly in the accent: it is the one piece of state
/// on screen that changes what a click on the page does.
pub fn rail_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    tooltip: &str,
    active: bool,
    enabled: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && enabled;
        let painter = ui.painter();
        let radius = CornerRadius::same(14);
        if active {
            painter.add(
                egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: p.accent_alpha(0x40),
                }
                .as_shape(rect, radius),
            );
            painter.rect_filled(rect, radius, theme::composite(p.accent_soft, p.rail));
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, p.accent_alpha(0x80)),
                egui::StrokeKind::Inside,
            );
        } else if hovered {
            painter.rect_filled(rect, radius, p.wash(0x10));
        }
        let color = if !enabled {
            p.text_disabled
        } else if active {
            p.accent_text
        } else if hovered {
            p.text
        } else {
            p.rail_icon
        };
        icons::draw(painter, Rect::from_center_size(rect.center(), Vec2::splat(20.0)), icon, color);
    }
    if enabled {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tooltip)
    } else {
        response
    }
}

/// One entry in a navigation list: home's sections.
pub fn nav_item(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    active: bool,
    count: Option<usize>,
) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(13);
        if active {
            raised(painter, rect, radius, p);
        } else if response.hovered() {
            painter.rect_filled(rect, radius, p.wash(0x0d));
        }
        let (icon_colour, text_colour) = if active {
            (p.accent_text, p.text)
        } else {
            (p.text_muted, p.text_muted)
        };
        let icon_rect = Rect::from_center_size(rect.left_center() + vec2(22.0, 0.0), Vec2::splat(18.0));
        icons::draw(painter, icon_rect, icon, icon_colour);
        painter.text(
            rect.left_center() + vec2(42.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            theme::strong(13.0),
            text_colour,
        );
        if let Some(n) = count {
            painter.text(
                rect.right_center() - vec2(14.0, 0.0),
                Align2::RIGHT_CENTER,
                n.to_string(),
                theme::strong(11.0),
                p.text_faint,
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The raised plate under a selected segment or nav item.
pub fn raised(painter: &egui::Painter, rect: Rect, radius: CornerRadius, p: &Palette) {
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: if p.is_dark() {
                Color32::from_black_alpha(0x66)
            } else {
                Color32::from_rgba_unmultiplied(0x2a, 0x1b, 0x50, 0x1a)
            },
        }
        .as_shape(rect, radius),
    );
    painter.rect_filled(
        rect,
        radius,
        if p.is_dark() { p.card_hover } else { p.card },
    );
    theme::paint_sheen(painter, rect, radius.nw as f32, p);
}

/// The document identity at the left of the canvas header: a type badge, the
/// filename, and the folder it came from underneath.
///
/// The folder is the disambiguator — two documents called `invoice.pdf` are
/// only told apart by where they live.
pub fn file_chip(ui: &mut Ui, p: &Palette, name: &str, folder: &str) {
    let painter = ui.painter();
    let name_g = painter.layout_no_wrap(name.to_owned(), theme::strong(13.0), p.text);
    let folder_g = painter.layout_no_wrap(folder.to_owned(), theme::body(11.0), p.text_faint);
    let width = 42.0 + name_g.size().x.max(folder_g.size().x);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let badge = Rect::from_min_size(rect.left_top() + vec2(0.0, 1.0), Vec2::splat(32.0));
    painter.rect_filled(badge, CornerRadius::same(11), p.accent_soft);
    icons::draw(
        painter,
        Rect::from_center_size(badge.center(), Vec2::splat(17.0)),
        Icon::Document,
        p.accent_text,
    );
    // Two lines when there is a folder, one centred line when there is not.
    let x = rect.left() + 42.0;
    if folder.is_empty() {
        painter.galley(
            egui::pos2(x, rect.center().y - name_g.size().y / 2.0),
            name_g,
            p.text,
        );
    } else {
        painter.galley(egui::pos2(x, rect.top() + 1.0), name_g, p.text);
        painter.galley(egui::pos2(x, rect.top() + 19.0), folder_g, p.text_faint);
    }
}

/// The floating capsule the page and zoom controls sit in.
pub fn capsule<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.popover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(18))
        .inner_margin(egui::Margin::same(5))
        .shadow(theme::float_shadow(p))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| add(ui)).inner
        })
        .inner
}

/// One segment of a [`segmented`] control.
#[derive(Debug, Clone, Copy)]
pub enum Segment<'a> {
    Text(&'a str),
    /// A label with a count after it.
    Counted(&'a str, usize),
    /// An icon alone; the string is its tooltip.
    Icon(Icon, &'a str),
    /// An icon before a label.
    Labelled(Icon, &'a str),
}

/// A segmented control: a recessed track with the chosen segment raised out
/// of it.
///
/// `fill` stretches the segments to the available width; otherwise each is as
/// wide as its content. Returns the index of a segment that was clicked.
pub fn segmented(
    ui: &mut Ui,
    p: &Palette,
    segments: &[Segment<'_>],
    current: usize,
    fill: bool,
    height: f32,
) -> Option<usize> {
    const PAD: f32 = 4.0;
    const GAP: f32 = 2.0;
    let font = theme::strong(if height >= 32.0 { 12.5 } else { 11.5 });
    let painter = ui.painter().clone();
    let measure = |s: &Segment<'_>| -> f32 {
        let text = |t: &str| painter.layout_no_wrap(t.to_owned(), font.clone(), p.text).size().x;
        match s {
            Segment::Text(t) => text(t) + 24.0,
            Segment::Counted(t, n) => text(t) + 24.0 + 8.0 + text(&n.to_string()).max(8.0) + 10.0,
            Segment::Icon(..) => height + 4.0,
            Segment::Labelled(_, t) => text(t) + 24.0 + 22.0,
        }
    };
    let natural: Vec<f32> = segments.iter().map(measure).collect();
    let n = segments.len().max(1) as f32;
    let width = if fill {
        ui.available_width()
    } else {
        natural.iter().sum::<f32>() + GAP * (n - 1.0) + PAD * 2.0
    };
    let (track, _) = ui.allocate_exact_size(vec2(width, height + PAD * 2.0), Sense::hover());
    let base_id = ui.id().with(("segmented", track.min.x as i32, track.min.y as i32));

    let track_radius = CornerRadius::same(((height + PAD * 2.0) * 0.36).round() as u8);
    let seg_radius = CornerRadius::same((height * 0.32).round() as u8);
    painter.rect_filled(track, track_radius, p.recess);
    painter.rect_stroke(track, track_radius, Stroke::new(1.0, p.wash(0x0a)), egui::StrokeKind::Inside);

    let inner = track.shrink(PAD);
    let mut x = inner.left();
    let even = (inner.width() - GAP * (n - 1.0)) / n;
    let mut clicked = None;
    for (i, seg) in segments.iter().enumerate() {
        let w = if fill { even } else { natural[i] };
        let rect = Rect::from_min_size(egui::pos2(x, inner.top()), vec2(w, height));
        x += w + GAP;

        let response = ui.interact(rect, base_id.with(i), Sense::click());
        let active = i == current;
        if active {
            raised(&painter, rect, seg_radius, p);
        } else if response.hovered() {
            painter.rect_filled(rect, seg_radius, p.wash(0x0a));
        }
        let colour = if active {
            p.text
        } else if response.hovered() {
            p.text_muted
        } else {
            p.text_faint
        };
        let icon_colour = if active { p.accent_text } else { colour };

        match seg {
            Segment::Text(t) => {
                painter.text(rect.center(), Align2::CENTER_CENTER, *t, font.clone(), colour);
            }
            Segment::Counted(t, count) => {
                let label = painter.layout_no_wrap(t.to_string(), font.clone(), colour);
                let num = painter.layout_no_wrap(count.to_string(), theme::strong(10.5), if active { p.accent_text } else { colour });
                let badge_w = num.size().x.max(8.0) + 10.0;
                let total = label.size().x + 7.0 + badge_w;
                let left = rect.center().x - total / 2.0;
                painter.galley(egui::pos2(left, rect.center().y - label.size().y / 2.0), label, colour);
                let badge = Rect::from_min_size(
                    egui::pos2(left + total - badge_w, rect.center().y - 9.0),
                    vec2(badge_w, 18.0),
                );
                painter.rect_filled(badge, CornerRadius::same(9), if active { p.accent_soft } else { p.wash(0x10) });
                painter.galley(badge.center() - num.size() / 2.0, num, p.text);
            }
            Segment::Icon(icon, _) => {
                icons::draw(&painter, Rect::from_center_size(rect.center(), Vec2::splat(17.0)), *icon, icon_colour);
            }
            Segment::Labelled(icon, t) => {
                let label = painter.layout_no_wrap(t.to_string(), font.clone(), colour);
                let total = 15.0 + 7.0 + label.size().x;
                let left = rect.center().x - total / 2.0;
                icons::draw(
                    &painter,
                    Rect::from_center_size(egui::pos2(left + 7.5, rect.center().y), Vec2::splat(15.0)),
                    *icon,
                    icon_colour,
                );
                painter.galley(egui::pos2(left + 22.0, rect.center().y - label.size().y / 2.0), label, colour);
            }
        }

        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let response = match seg {
            Segment::Icon(_, tip) => response.on_hover_text(*tip),
            _ => response,
        };
        if response.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// A keyboard key, for shortcut hints.
pub fn kbd(ui: &mut Ui, p: &Palette, key: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(key.to_owned(), theme::strong(10.5), p.text_muted);
    let size = vec2(galley.size().x + 12.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(7), p.wash(0x10));
        painter.galley(rect.center() - galley.size() / 2.0, galley, p.text_muted);
    }
}

/// A shortcut as this platform spells it.
///
/// The command table writes `Ctrl`, and the matcher accepts Cmd for it on a
/// Mac; showing `Ctrl` there would send the user to the wrong key.
pub fn platform_keys(spelled: &str) -> String {
    if cfg!(target_os = "macos") {
        spelled.replace("Ctrl+", "Cmd+")
    } else {
        spelled.to_owned()
    }
}

/// The keys of a spelled shortcut, one per chip.
fn keys(spelled: &str) -> Vec<String> {
    platform_keys(spelled)
        .split('+')
        .filter(|k| !k.is_empty())
        .map(|k| match k {
            "Plus" => "+".to_string(),
            "Minus" => "−".to_string(),
            k => k.to_string(),
        })
        .collect()
}

/// A run of keys from a shortcut spelled `Ctrl+Shift+P`.
pub fn shortcut(ui: &mut Ui, p: &Palette, spelled: &str) {
    ui.spacing_mut().item_spacing.x = 3.0;
    for key in keys(spelled) {
        kbd(ui, p, &key);
    }
}

/// Paints a shortcut's keys ending at `right`, for rows drawn by hand.
/// Returns the left edge of the run.
pub fn paint_keys(painter: &egui::Painter, right: f32, centre_y: f32, spelled: &str, p: &Palette) -> f32 {
    let mut x = right;
    for key in keys(spelled).iter().rev() {
        let galley = painter.layout_no_wrap(key.clone(), theme::strong(10.5), p.text_muted);
        let w = galley.size().x + 12.0;
        let chip = Rect::from_min_max(egui::pos2(x - w, centre_y - 10.0), egui::pos2(x, centre_y + 10.0));
        painter.rect_filled(chip, CornerRadius::same(7), p.wash(0x10));
        painter.galley(chip.center() - galley.size() / 2.0, galley, p.text_muted);
        x -= w + 3.0;
    }
    x + 3.0
}

/// Paints a small tag ending at `right`. Returns its left edge.
pub fn paint_tag(painter: &egui::Painter, right: f32, centre_y: f32, text: &str, p: &Palette) -> f32 {
    let galley = painter.layout_no_wrap(text.to_owned(), theme::strong(11.0), p.text_faint);
    let w = galley.size().x + 16.0;
    let tag = Rect::from_min_max(egui::pos2(right - w, centre_y - 11.0), egui::pos2(right, centre_y + 11.0));
    painter.rect_filled(tag, CornerRadius::same(8), p.wash(0x0d));
    painter.galley(tag.center() - galley.size() / 2.0, galley, p.text_faint);
    right - w
}

/// A small rounded count or label.
pub fn badge(ui: &mut Ui, p: &Palette, text: &str, accent: bool) {
    let colour = if accent { p.accent_text } else { p.text_muted };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::strong(11.0), colour);
    let size = vec2(galley.size().x.max(8.0) + 12.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            CornerRadius::same(8),
            if accent { p.accent_soft } else { p.wash(0x0d) },
        );
        painter.galley(rect.center() - galley.size() / 2.0, galley, colour);
    }
}

/// Quark's mark: three quarks bound in a ring.
pub fn brand_mark(painter: &egui::Painter, rect: Rect, p: &Palette) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    painter.circle_stroke(c, s * 0.40, Stroke::new((s * 0.07).max(1.2), p.accent_alpha(0x66)));
    let dot = s * 0.095;
    painter.circle_filled(c + vec2(0.0, -s * 0.20), dot, p.accent);
    painter.circle_filled(c + vec2(-s * 0.17, s * 0.10), dot, p.accent_text);
    painter.circle_filled(c + vec2(s * 0.17, s * 0.10), dot, p.accent_alpha(0x8c));
}

/// A round initial, for comment authors.
pub fn avatar(ui: &mut Ui, p: &Palette, name: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let initial: String = name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "?".into());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), 10.0, p.accent_soft);
        painter.text(rect.center(), Align2::CENTER_CENTER, initial, theme::heavy(9.5), p.accent_text);
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
    fn shortcuts_split_into_keys_and_name_the_symbols() {
        let k = keys("Ctrl+Shift+Plus");
        assert_eq!(k.len(), 3);
        assert_eq!(k[2], "+");
        assert_eq!(k[0], if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" });
        assert!(keys("").is_empty());
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
