//! One split of the code editor, drawn: the gutter, the lines in their
//! colours, search matches, the selection and the cursor. The text and
//! where things are come from the session (`quark_modal`); this only draws,
//! and turns a pointer back into a line and a column.
//!
//! Lines are laid out in a monospaced font, one character to a cell, with a
//! tab drawn as spaces to its next stop. Columns are characters, as the
//! buffer counts them; cells are what is drawn. The two differ only by tabs.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use quark_modal::session::Session;
use quark_modal::vim::VimMode;
use quark_syntax::{Scope, Span, Syntax};
use quark_ui::theme::Palette;

/// The editor's text size, in points.
pub(crate) const TEXT_SIZE: f32 = 13.5;
/// Space between lines, as a share of the text size.
const LEADING: f32 = 1.55;
/// Room either side of the line numbers.
const GUTTER_PAD: f32 = 14.0;
/// Room before the text.
const TEXT_PAD: f32 = 10.0;

pub(crate) fn font() -> FontId {
    FontId::monospace(TEXT_SIZE)
}

/// A cell's size, and where the text and the gutter start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Metrics {
    pub char_w: f32,
    pub line_h: f32,
    pub gutter_w: f32,
}

impl Metrics {
    /// For a file of `lines` lines, in cells `char_w` wide.
    pub(crate) fn new(char_w: f32, lines: usize) -> Metrics {
        Metrics { char_w, line_h: (TEXT_SIZE * LEADING).round(), gutter_w: gutter_width(char_w, lines) }
    }

    /// Where the text starts, from the split's left.
    pub(crate) fn text_left(&self) -> f32 {
        self.gutter_w + TEXT_PAD
    }

    /// How many whole lines fit in `height`.
    pub(crate) fn lines_in(&self, height: f32) -> usize {
        ((height / self.line_h).floor() as usize).max(1)
    }
}

/// The gutter: room for the widest line number, three digits at least.
pub(crate) fn gutter_width(char_w: f32, lines: usize) -> f32 {
    let digits = lines.max(1).to_string().len().max(3);
    digits as f32 * char_w + GUTTER_PAD * 2.0
}

/// The cell column a character column is drawn at, tabs to their stops.
pub(crate) fn cell_of(line: &str, column: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut cell = 0;
    for c in line.chars().take(column) {
        cell = if c == '\t' { (cell / tab + 1) * tab } else { cell + 1 };
    }
    // Past the end of the line, a cell a column.
    cell + column.saturating_sub(line.chars().count())
}

/// The character column under cell `cell`: a click in a tab's cells is the
/// tab; a click past the end of the line, its end.
pub(crate) fn column_at(line: &str, cell: f32, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut at = 0.0;
    for (i, c) in line.chars().enumerate() {
        let next = if c == '\t' { ((at / tab as f32).floor() + 1.0) * tab as f32 } else { at + 1.0 };
        // Nearer this character's left edge than its right: before it.
        if cell < (at + next) / 2.0 {
            return i;
        }
        at = next;
    }
    line.chars().count()
}

/// A line as drawn: its tabs as spaces.
pub(crate) fn expanded(line: &str, tab: usize) -> String {
    let tab = tab.max(1);
    let mut out = String::with_capacity(line.len());
    let mut cell = 0;
    for c in line.chars() {
        if c == '\t' {
            let to = (cell / tab + 1) * tab;
            out.extend(std::iter::repeat_n(' ', to - cell));
            cell = to;
        } else {
            out.push(c);
            cell += 1;
        }
    }
    out
}

/// What is selected on one line, as a half-open range of character
/// columns; `None` where nothing is.
pub(crate) fn selected_on(
    mode: VimMode,
    anchor: (usize, usize),
    head: (usize, usize),
    line: usize,
    line_len: usize,
) -> Option<(usize, usize)> {
    let (first, last) = if anchor <= head { (anchor, head) } else { (head, anchor) };
    if line < first.0 || line > last.0 {
        return None;
    }
    match mode {
        VimMode::Visual => {
            let from = if line == first.0 { first.1 } else { 0 };
            // Inclusive of the character under the head, and the line break
            // after a line wholly inside.
            let to = if line == last.0 { (last.1 + 1).min(line_len.max(last.1 + 1)) } else { line_len + 1 };
            Some((from, to))
        }
        VimMode::VisualLine => Some((0, line_len + 1)),
        VimMode::VisualBlock => {
            let (a, b) = (anchor.1.min(head.1), anchor.1.max(head.1));
            Some((a, b + 1))
        }
        _ => None,
    }
}

/// The colour of a scope, in this theme.
pub(crate) fn colour(scope: Scope, p: &Palette) -> Color32 {
    let dark = p.is_dark();
    let pick = |d: [u8; 3], l: [u8; 3]| {
        let [r, g, b] = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    };
    match scope {
        Scope::Keyword => pick([0xc6, 0x9b, 0xff], [0x7a, 0x3c, 0xd6]),
        Scope::Function => pick([0x82, 0xb8, 0xff], [0x1f, 0x5f, 0xc4]),
        Scope::Type => pick([0x6e, 0xd6, 0xc8], [0x0f, 0x7f, 0x73]),
        Scope::String => pick([0xf0, 0xc0, 0x74], [0xa0, 0x5a, 0x00]),
        Scope::Number | Scope::Constant => pick([0xf3, 0x9c, 0x7a], [0xb5, 0x45, 0x20]),
        Scope::Comment => pick([0x7d, 0x80, 0x8c], [0x8a, 0x8c, 0x96]),
        Scope::Heading => pick([0xf4, 0xf4, 0xf6], [0x15, 0x15, 0x1a]),
        Scope::Attribute => pick([0x9e, 0xd8, 0x8a], [0x2f, 0x7d, 0x1f]),
        Scope::Property => pick([0xe6, 0xa5, 0xd6], [0xa3, 0x2f, 0x8a]),
        _ => p.text,
    }
}

/// How far a view is scrolled sideways, in cells, moved as little as it
/// can be so the cursor's cell is in the `width` cells shown, with a little
/// room past it.
pub(crate) fn sideways(left: usize, width: usize, cursor: usize) -> usize {
    let room = 4.min(width / 4);
    if cursor < left + room {
        cursor.saturating_sub(room)
    } else if cursor + room >= left + width {
        cursor + room + 1 - width.max(1)
    } else {
        left
    }
}

/// What a split needs to know to be drawn.
pub(crate) struct ViewDraw<'a> {
    pub view: usize,
    pub focused: bool,
    /// The editor has the keyboard.
    pub active: bool,
    pub syntax: Option<&'a mut Syntax>,
    pub tab: usize,
    /// How far it is scrolled sideways, in cells: kept by the caller, moved
    /// here to follow the cursor.
    pub left: &'a mut usize,
}

/// What a pointer did in a split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Pointer {
    None,
    /// Pressed here: the cursor goes here.
    Click { view: usize, line: usize, column: usize },
    /// Dragged to here: the selection runs here.
    Drag { line: usize, column: usize },
    /// The wheel, by lines.
    Scroll { view: usize, lines: i64 },
}

/// Draw split `d.view` in `rect`, and say what the pointer did to it.
pub(crate) fn show(ui: &mut Ui, p: &Palette, session: &Session, d: ViewDraw, rect: Rect) -> Pointer {
    let Some((buffer_index, cursor, first)) = session.view_of(d.view) else { return Pointer::None };
    let Some(buffer) = session.editor().buffers().get(buffer_index) else { return Pointer::None };
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font(), 'M'));
    let m = Metrics::new(char_w, buffer.len_lines());
    let response = ui.interact(rect, ui.id().with(("code-view", d.view)), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, p.canvas);
    let gutter = Rect::from_min_size(rect.min, vec2(m.gutter_w, rect.height()));
    painter.rect_filled(gutter, CornerRadius::ZERO, p.wash(0x04));

    let lines = m.lines_in(rect.height());
    let texts = buffer.lines(first, lines);
    let spans: Vec<Vec<Span>> = match d.syntax {
        Some(s) => s.spans(buffer, first, lines),
        None => Vec::new(),
    };
    let head = buffer.position(cursor.offset);
    let anchor = buffer.position(cursor.anchor);
    let mode = if d.focused { session.mode() } else { VimMode::Normal };
    let pattern = session.pattern();
    // Sideways, the cursor kept in view.
    let width = (((rect.width() - m.text_left()) / m.char_w).floor() as usize).max(1);
    if d.focused {
        let line = buffer.line(head.line);
        let cursor_cell = cell_of(line.trim_end_matches(['\n', '\r']), head.column, d.tab);
        *d.left = sideways(*d.left, width, cursor_cell);
    }
    let left = *d.left;
    let text_x = rect.left() + m.text_left() - left as f32 * m.char_w;
    // The text is drawn right of the gutter only; the gutter over it after.
    let text_painter = painter.with_clip_rect(Rect::from_min_max(pos2(rect.left() + m.gutter_w, rect.top()), rect.max));
    let mut numbers = Vec::new();

    for (i, raw) in texts.iter().enumerate() {
        let line = first + i;
        let text = raw.trim_end_matches(['\n', '\r']);
        let y = rect.top() + i as f32 * m.line_h;
        let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), m.line_h));
        // The cursor's line, lit faintly.
        if line == head.line && d.focused {
            painter.rect_filled(row, CornerRadius::ZERO, p.wash(0x06));
        }
        numbers.push((line, y));
        let cell_x = |column: usize| text_x + cell_of(text, column, d.tab) as f32 * m.char_w;
        // Search matches.
        if let Some(pattern) = pattern {
            for (a, b) in quark_text::search::find_in(text, pattern) {
                let r = Rect::from_min_max(pos2(cell_x(a), y + 2.0), pos2(cell_x(b), y + m.line_h - 2.0));
                text_painter.rect_filled(r, CornerRadius::same(3), Color32::from_rgba_unmultiplied(0xf0, 0xc0, 0x40, 0x40));
            }
        }
        // The selection.
        let len = text.chars().count();
        if let Some((a, b)) = selected_on(mode, (anchor.line, anchor.column), (head.line, head.column), line, len) {
            let right = if b > len { cell_x(len) + m.char_w * 0.6 } else { cell_x(b) };
            let r = Rect::from_min_max(pos2(cell_x(a), y), pos2(right.max(cell_x(a) + 2.0), y + m.line_h));
            text_painter.rect_filled(r, CornerRadius::same(2), p.accent_alpha(0x55));
        }
        // The text, in its colours.
        let shown = expanded(text, d.tab);
        let mut job = egui::text::LayoutJob::default();
        let line_spans = spans.get(i).map(Vec::as_slice).unwrap_or(&[]);
        let mut at = 0;
        let chars: Vec<char> = text.chars().collect();
        let cells = |from: usize, to: usize| -> String {
            let a = cell_of(text, from, d.tab);
            let b = cell_of(text, to, d.tab);
            shown.chars().skip(a).take(b - a).collect()
        };
        for s in line_spans {
            if s.start > at {
                job.append(&cells(at, s.start), 0.0, egui::TextFormat::simple(font(), p.text));
            }
            let end = s.end.min(chars.len());
            if end > s.start {
                job.append(&cells(s.start, end), 0.0, egui::TextFormat::simple(font(), colour(s.scope, p)));
            }
            at = end.max(at);
        }
        if at < chars.len() {
            job.append(&cells(at, chars.len()), 0.0, egui::TextFormat::simple(font(), p.text));
        }
        let galley = text_painter.layout_job(job);
        let text_y = y + (m.line_h - galley.size().y) / 2.0;
        text_painter.galley(pos2(text_x, text_y), galley, p.text);

        // The cursor: a block in normal mode, a bar where text goes in.
        if line == head.line && d.focused {
            let x = cell_x(head.column);
            let w = chars.get(head.column).map_or(m.char_w, |c| if *c == '\t' { cell_x(head.column + 1) - x } else { m.char_w });
            let colour = if d.active { p.accent } else { p.text_faint };
            match session.mode() {
                VimMode::Insert => {
                    text_painter.rect_filled(Rect::from_min_size(pos2(x - 1.0, y + 3.0), vec2(2.0, m.line_h - 6.0)), CornerRadius::ZERO, colour);
                }
                VimMode::Replace => {
                    text_painter.rect_filled(Rect::from_min_size(pos2(x, y + m.line_h - 4.0), vec2(w, 2.0)), CornerRadius::ZERO, colour);
                }
                VimMode::Command => {}
                _ if d.active => {
                    let block = Rect::from_min_size(pos2(x, y + 2.0), vec2(w, m.line_h - 4.0));
                    text_painter.rect_filled(block, CornerRadius::same(2), colour.gamma_multiply(0.75));
                    if let Some(c) = chars.get(head.column).filter(|c| !c.is_whitespace()) {
                        text_painter.text(pos2(x, text_y), egui::Align2::LEFT_TOP, c.to_string(), font(), p.canvas);
                    }
                }
                _ => {
                    let block = Rect::from_min_size(pos2(x, y + 2.0), vec2(w, m.line_h - 4.0));
                    text_painter.rect_stroke(block, CornerRadius::same(2), Stroke::new(1.0, colour), egui::StrokeKind::Inside);
                }
            }
        }
    }

    // The gutter, over any text scrolled under it, and its numbers.
    painter.rect_filled(gutter, CornerRadius::ZERO, p.canvas);
    painter.rect_filled(gutter, CornerRadius::ZERO, p.wash(0x04));
    for (line, y) in numbers {
        painter.text(
            pos2(rect.left() + m.gutter_w - GUTTER_PAD, y + m.line_h / 2.0),
            egui::Align2::RIGHT_CENTER,
            (line + 1).to_string(),
            FontId::monospace(TEXT_SIZE - 1.5),
            if line == head.line { p.text_muted } else { p.text_faint },
        );
    }

    // The pointer: a press puts the cursor, a drag selects, the wheel scrolls.
    let to_line_col = |pos: Pos2| -> (usize, usize) {
        let row = ((pos.y - rect.top()) / m.line_h).floor().max(0.0) as usize;
        let line = first + row;
        let text = texts.get(row).map(|t| t.trim_end_matches(['\n', '\r'])).unwrap_or("");
        let cell = (pos.x - text_x) / m.char_w;
        (line, column_at(text, cell.max(0.0), d.tab))
    };
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel.abs() > 0.5 {
            return Pointer::Scroll { view: d.view, lines: (-wheel / m.line_h).round() as i64 };
        }
    }
    if response.drag_started() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (line, column) = to_line_col(pos);
            return Pointer::Click { view: d.view, line, column };
        }
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (line, column) = to_line_col(pos);
            return Pointer::Drag { line, column };
        }
    }
    Pointer::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_is_drawn_to_its_next_stop_and_a_click_in_it_is_the_tab() {
        assert_eq!(expanded("\tab", 4), "    ab");
        assert_eq!(expanded("x\ty", 4), "x   y");
        assert_eq!(cell_of("x\ty", 2, 4), 4, "y is drawn at cell four");
        assert_eq!(cell_of("abc", 5, 4), 5, "past the end, a cell a column");
        assert_eq!(column_at("x\ty", 2.0, 4), 1, "a click inside the tab's cells is the tab");
        assert_eq!(column_at("x\ty", 4.2, 4), 2);
        assert_eq!(column_at("abc", 0.4, 4), 0);
        assert_eq!(column_at("abc", 0.6, 4), 1, "nearer the right edge: after it");
        assert_eq!(column_at("abc", 40.0, 4), 3, "past the end: the end");
        assert_eq!(column_at("héllo", 2.0, 4), 2, "characters, not bytes");
    }

    #[test]
    fn the_gutter_holds_the_widest_line_number() {
        assert_eq!(gutter_width(8.0, 10), 3.0 * 8.0 + GUTTER_PAD * 2.0, "three digits at least");
        assert_eq!(gutter_width(8.0, 12_345), 5.0 * 8.0 + GUTTER_PAD * 2.0);
        let m = Metrics::new(8.0, 10);
        assert_eq!(m.lines_in(m.line_h * 10.5), 10, "whole lines only");
        assert_eq!(m.lines_in(1.0), 1);
    }

    #[test]
    fn a_selection_covers_what_each_visual_mode_covers() {
        // From line 1 column 3 to line 3 column 2, as Visual.
        let (a, h) = ((1, 3), (3, 2));
        assert_eq!(selected_on(VimMode::Visual, a, h, 0, 9), None);
        assert_eq!(selected_on(VimMode::Visual, a, h, 1, 9), Some((3, 10)), "to the end, and its line break");
        assert_eq!(selected_on(VimMode::Visual, a, h, 2, 4), Some((0, 5)));
        assert_eq!(selected_on(VimMode::Visual, a, h, 3, 9), Some((0, 3)), "the character under the head too");
        // Backwards is the same.
        assert_eq!(selected_on(VimMode::Visual, h, a, 3, 9), Some((0, 3)));
        assert_eq!(selected_on(VimMode::VisualLine, a, h, 2, 4), Some((0, 5)));
        assert_eq!(selected_on(VimMode::VisualBlock, a, h, 2, 9), Some((2, 4)), "the columns between");
        assert_eq!(selected_on(VimMode::Normal, a, h, 2, 9), None);
        // On one line.
        assert_eq!(selected_on(VimMode::Visual, (0, 2), (0, 2), 0, 9), Some((2, 3)));
    }

    #[test]
    fn a_long_line_scrolls_sideways_as_little_as_it_can_to_show_the_cursor() {
        assert_eq!(sideways(0, 80, 10), 0, "in view: left alone");
        assert_eq!(sideways(0, 80, 78), 3, "near the right edge: just enough, with room");
        assert_eq!(sideways(40, 80, 41), 37, "back toward the left");
        assert_eq!(sideways(40, 80, 2), 0);
        assert_eq!(sideways(0, 4, 9), 7, "a narrow view keeps less room: cells 7 to 10, one past the cursor");
    }
}
