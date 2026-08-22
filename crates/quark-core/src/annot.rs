//! The annotation model.
//!
//! This mirrors the PDF annotation subtypes from ISO 32000-1 §12.5 closely
//! enough to round-trip through a file, while staying independent of PDFium so
//! the editing and undo logic can be tested on its own.
//!
//! Coordinates are always **page space** (PDF points, y-up). Storing screen
//! coordinates would mean every annotation shifted when the zoom changed.

use crate::geom::{Rect, Vec2};
use serde::{Deserialize, Serialize};

/// An RGBA colour, 0-255 per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const RED: Self = Self::rgb(0xE5, 0x3E, 0x3E);
    pub const YELLOW: Self = Self::rgb(0xFF, 0xD4, 0x3B);
    pub const GREEN: Self = Self::rgb(0x3E, 0xC4, 0x6B);
    pub const BLUE: Self = Self::rgb(0x3B, 0x82, 0xF6);
    pub const PURPLE: Self = Self::rgb(0xA8, 0x55, 0xF7);
    pub const ORANGE: Self = Self::rgb(0xF9, 0x73, 0x16);
    pub const PINK: Self = Self::rgb(0xEC, 0x48, 0x99);

    /// The highlighter palette offered in the toolbar.
    pub const HIGHLIGHT_PALETTE: [Color; 6] = [
        Self::YELLOW,
        Self::GREEN,
        Self::BLUE,
        Self::PINK,
        Self::ORANGE,
        Self::PURPLE,
    ];

    /// The drawing palette for ink and shapes.
    pub const DRAW_PALETTE: [Color; 8] = [
        Self::RED,
        Self::ORANGE,
        Self::YELLOW,
        Self::GREEN,
        Self::BLUE,
        Self::PURPLE,
        Self::BLACK,
        Self::WHITE,
    ];

    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Normalised components, as PDF colour operators want them.
    pub fn to_f32(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }

    pub fn from_f32(c: [f32; 4]) -> Self {
        Self {
            r: (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            g: (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            b: (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            a: (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        }
    }

    /// Perceived luminance, used to pick readable text over a fill.
    pub fn luminance(self) -> f32 {
        (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32) / 255.0
    }

    pub fn contrasting_text(self) -> Color {
        if self.luminance() > 0.55 {
            Color::BLACK
        } else {
            Color::WHITE
        }
    }
}

/// Line ending styles for lines and polylines (PDF `/LE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineEnding {
    #[default]
    None,
    OpenArrow,
    ClosedArrow,
    Circle,
    Square,
    Diamond,
    Butt,
    Slash,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::None => "None",
            LineEnding::OpenArrow => "Open Arrow",
            LineEnding::ClosedArrow => "Closed Arrow",
            LineEnding::Circle => "Circle",
            LineEnding::Square => "Square",
            LineEnding::Diamond => "Diamond",
            LineEnding::Butt => "Butt",
            LineEnding::Slash => "Slash",
        }
    }
}

/// The icon shown for a collapsed sticky note (PDF `/Name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NoteIcon {
    #[default]
    Comment,
    Note,
    Help,
    Key,
    NewParagraph,
    Paragraph,
    Insert,
}

impl NoteIcon {
    pub fn label(self) -> &'static str {
        match self {
            NoteIcon::Comment => "Comment",
            NoteIcon::Note => "Note",
            NoteIcon::Help => "Help",
            NoteIcon::Key => "Key",
            NoteIcon::NewParagraph => "New Paragraph",
            NoteIcon::Paragraph => "Paragraph",
            NoteIcon::Insert => "Insert",
        }
    }
}

/// Review status a reviewer can set on a comment, as in Acrobat's comment list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReviewStatus {
    #[default]
    None,
    Accepted,
    Rejected,
    Cancelled,
    Completed,
}

impl ReviewStatus {
    pub fn label(self) -> &'static str {
        match self {
            ReviewStatus::None => "None",
            ReviewStatus::Accepted => "Accepted",
            ReviewStatus::Rejected => "Rejected",
            ReviewStatus::Cancelled => "Cancelled",
            ReviewStatus::Completed => "Completed",
        }
    }

    pub const ALL: [ReviewStatus; 5] = [
        ReviewStatus::None,
        ReviewStatus::Accepted,
        ReviewStatus::Rejected,
        ReviewStatus::Cancelled,
        ReviewStatus::Completed,
    ];
}

/// A single stroke of a freehand drawing: a run of connected points.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InkStroke {
    pub points: Vec<Vec2>,
}

impl InkStroke {
    /// Bounding box of the stroke, or `None` when it has no points.
    pub fn bounds(&self) -> Option<Rect> {
        let mut it = self.points.iter();
        let first = *it.next()?;
        let mut r = Rect::new(first, first);
        for p in it {
            r = r.union(&Rect::new(*p, *p));
        }
        Some(r)
    }
}

/// The geometry and style specific to each annotation subtype.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnnotKind {
    /// Text markup bound to selected text. `quads` are the per-line rectangles
    /// the selection covered, which is what PDF's `/QuadPoints` stores.
    Highlight {
        quads: Vec<Rect>,
    },
    Underline {
        quads: Vec<Rect>,
    },
    StrikeOut {
        quads: Vec<Rect>,
    },
    Squiggly {
        quads: Vec<Rect>,
    },
    /// A collapsed sticky note anchored at a point.
    Note {
        icon: NoteIcon,
    },
    /// Free-standing text drawn on the page.
    FreeText {
        text: String,
        font_size: f32,
        font: String,
        align: TextAlign,
        /// Optional leader line to a point being annotated (a callout).
        callout: Option<[Vec2; 3]>,
    },
    Ink {
        strokes: Vec<InkStroke>,
    },
    Line {
        start: Vec2,
        end: Vec2,
        start_ending: LineEnding,
        end_ending: LineEnding,
        /// When set, the line reports its length in the given unit.
        measure: Option<Measure>,
    },
    Square,
    Circle,
    Polygon {
        points: Vec<Vec2>,
    },
    Polyline {
        points: Vec<Vec2>,
        start_ending: LineEnding,
        end_ending: LineEnding,
    },
    /// A rubber stamp: either a named standard stamp or an embedded image.
    Stamp {
        name: String,
        image: Option<Vec<u8>>,
    },
    /// A region marked for redaction. Applying it destroys the content beneath.
    Redact {
        /// Text drawn over the blacked-out area.
        overlay_text: Option<String>,
        fill: Color,
    },
    /// An embedded file attached at a point on the page.
    FileAttachment {
        filename: String,
    },
    /// Anything Quark parsed but does not model, kept so that saving does not
    /// silently drop it.
    Unsupported {
        subtype: String,
    },
}

impl AnnotKind {
    pub fn subtype_name(&self) -> &'static str {
        match self {
            AnnotKind::Highlight { .. } => "Highlight",
            AnnotKind::Underline { .. } => "Underline",
            AnnotKind::StrikeOut { .. } => "StrikeOut",
            AnnotKind::Squiggly { .. } => "Squiggly",
            AnnotKind::Note { .. } => "Text",
            AnnotKind::FreeText { .. } => "FreeText",
            AnnotKind::Ink { .. } => "Ink",
            AnnotKind::Line { .. } => "Line",
            AnnotKind::Square => "Square",
            AnnotKind::Circle => "Circle",
            AnnotKind::Polygon { .. } => "Polygon",
            AnnotKind::Polyline { .. } => "PolyLine",
            AnnotKind::Stamp { .. } => "Stamp",
            AnnotKind::Redact { .. } => "Redact",
            AnnotKind::FileAttachment { .. } => "FileAttachment",
            AnnotKind::Unsupported { .. } => "Unsupported",
        }
    }

    /// Whether this kind is anchored to a run of selected text.
    pub fn is_text_markup(&self) -> bool {
        matches!(
            self,
            AnnotKind::Highlight { .. }
                | AnnotKind::Underline { .. }
                | AnnotKind::StrikeOut { .. }
                | AnnotKind::Squiggly { .. }
        )
    }

    /// The quads of a text-markup annotation.
    pub fn quads(&self) -> Option<&[Rect]> {
        match self {
            AnnotKind::Highlight { quads }
            | AnnotKind::Underline { quads }
            | AnnotKind::StrikeOut { quads }
            | AnnotKind::Squiggly { quads } => Some(quads),
            _ => None,
        }
    }
}

/// Horizontal alignment for free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Units a measurement annotation reports in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MeasureUnit {
    #[default]
    Points,
    Inches,
    Millimetres,
    Centimetres,
}

impl MeasureUnit {
    pub fn suffix(self) -> &'static str {
        match self {
            MeasureUnit::Points => "pt",
            MeasureUnit::Inches => "in",
            MeasureUnit::Millimetres => "mm",
            MeasureUnit::Centimetres => "cm",
        }
    }

    /// How many of this unit make up one PDF point.
    pub fn per_point(self) -> f32 {
        match self {
            MeasureUnit::Points => 1.0,
            MeasureUnit::Inches => 1.0 / 72.0,
            MeasureUnit::Millimetres => 25.4 / 72.0,
            MeasureUnit::Centimetres => 2.54 / 72.0,
        }
    }

    pub const ALL: [MeasureUnit; 4] = [
        MeasureUnit::Points,
        MeasureUnit::Inches,
        MeasureUnit::Millimetres,
        MeasureUnit::Centimetres,
    ];
}

/// A distance scale for measurement tools, e.g. 1 inch on the page = 10 feet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub unit: MeasureUnit,
    /// Multiplier applied after unit conversion, for scaled drawings.
    pub scale: f32,
}

impl Default for Measure {
    fn default() -> Self {
        Self {
            unit: MeasureUnit::Inches,
            scale: 1.0,
        }
    }
}

impl Measure {
    /// Formats a length given in PDF points.
    pub fn format(&self, points: f32) -> String {
        let v = points * self.unit.per_point() * self.scale;
        format!("{v:.2} {}", self.unit.suffix())
    }
}

/// Border dash pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BorderStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// A reply in a comment thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub author: String,
    pub contents: String,
    /// RFC 3339 timestamp.
    pub created: String,
}

/// A stable identifier for an annotation within a session.
///
/// Deliberately not the PDF object number: annotations created in this session
/// do not have one yet, and object numbers change when a document is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AnnotId(pub u64);

/// One annotation on one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: AnnotId,
    pub page: usize,
    pub kind: AnnotKind,
    /// Bounding rectangle in page space (PDF `/Rect`).
    pub rect: Rect,
    pub color: Color,
    /// Fill colour for shapes (PDF `/IC`), when the shape is filled.
    pub interior_color: Option<Color>,
    pub border_width: f32,
    pub border_style: BorderStyle,
    /// 0.0 – 1.0 (PDF `/CA`).
    pub opacity: f32,
    pub author: String,
    pub contents: String,
    pub subject: String,
    /// RFC 3339 timestamp.
    pub created: String,
    pub modified: String,
    pub replies: Vec<Reply>,
    pub status: ReviewStatus,
    /// Locked annotations are drawn but not editable (PDF flag bit 8).
    pub locked: bool,
    pub hidden: bool,
    /// True once the annotation differs from what is in the file.
    #[serde(default)]
    pub dirty: bool,
}

impl Annotation {
    /// A new annotation with sane defaults for its kind.
    pub fn new(id: AnnotId, page: usize, kind: AnnotKind, rect: Rect, author: &str) -> Self {
        let now = now_rfc3339();
        // Highlights are multiplied over the text and need to be translucent to
        // stay readable; everything else defaults to opaque.
        let opacity = if matches!(kind, AnnotKind::Highlight { .. }) {
            0.4
        } else {
            1.0
        };
        let color = match &kind {
            AnnotKind::Highlight { .. } => Color::YELLOW,
            AnnotKind::Redact { .. } => Color::BLACK,
            _ => Color::RED,
        };
        Self {
            id,
            page,
            kind,
            rect: rect.normalize(),
            color,
            interior_color: None,
            border_width: 2.0,
            border_style: BorderStyle::Solid,
            opacity,
            author: author.to_owned(),
            contents: String::new(),
            subject: String::new(),
            created: now.clone(),
            modified: now,
            replies: Vec::new(),
            status: ReviewStatus::None,
            locked: false,
            hidden: false,
            dirty: true,
        }
    }

    /// Recomputes `rect` from the annotation's own geometry.
    ///
    /// Callers move points around freely; this is what keeps `/Rect` — which
    /// readers use for hit-testing and for deciding what to repaint — in step.
    /// Shapes keep their rect as the primary geometry and are left alone.
    pub fn recompute_rect(&mut self) {
        let bounds = match &self.kind {
            AnnotKind::Highlight { quads }
            | AnnotKind::Underline { quads }
            | AnnotKind::StrikeOut { quads }
            | AnnotKind::Squiggly { quads } => union_all(quads.iter().copied()),
            AnnotKind::Ink { strokes } => {
                let b = union_all(strokes.iter().filter_map(|s| s.bounds()));
                // Ink is stroked, so half the pen width falls outside the path.
                b.map(|r| r.expand(self.border_width * 0.5))
            }
            AnnotKind::Line { start, end, .. } => {
                Some(Rect::new(*start, *end).normalize().expand(self.border_width))
            }
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points, .. } => {
                union_all(points.iter().map(|p| Rect::new(*p, *p)))
                    .map(|r| r.expand(self.border_width))
            }
            _ => None,
        };
        if let Some(b) = bounds {
            self.rect = b;
        }
    }

    pub fn touch(&mut self) {
        self.modified = now_rfc3339();
        self.dirty = true;
    }

    /// Whether a page-space point hits this annotation.
    ///
    /// Text markup is tested against its quads rather than its bounding box: a
    /// selection spanning three lines has a bounding box covering the full
    /// paragraph, and clicking the whitespace beside a short last line should
    /// not select it.
    pub fn hit_test(&self, p: Vec2, tolerance: f32) -> bool {
        if self.hidden {
            return false;
        }
        match &self.kind {
            k if k.is_text_markup() => k
                .quads()
                .map(|q| q.iter().any(|r| r.expand(tolerance).contains(p)))
                .unwrap_or(false),
            AnnotKind::Ink { strokes } => strokes.iter().any(|s| {
                s.points
                    .windows(2)
                    .any(|w| dist_to_segment(p, w[0], w[1]) <= tolerance + self.border_width)
            }),
            AnnotKind::Line { start, end, .. } => {
                dist_to_segment(p, *start, *end) <= tolerance + self.border_width
            }
            AnnotKind::Polyline { points, .. } => points
                .windows(2)
                .any(|w| dist_to_segment(p, w[0], w[1]) <= tolerance + self.border_width),
            _ => self.rect.expand(tolerance).contains(p),
        }
    }

    /// Moves the annotation by a page-space delta, including its geometry.
    pub fn translate(&mut self, d: Vec2) {
        self.rect = self.rect.translate(d);
        match &mut self.kind {
            AnnotKind::Highlight { quads }
            | AnnotKind::Underline { quads }
            | AnnotKind::StrikeOut { quads }
            | AnnotKind::Squiggly { quads } => {
                for q in quads.iter_mut() {
                    *q = q.translate(d);
                }
            }
            AnnotKind::Ink { strokes } => {
                for s in strokes.iter_mut() {
                    for p in s.points.iter_mut() {
                        *p = *p + d;
                    }
                }
            }
            AnnotKind::Line { start, end, .. } => {
                *start = *start + d;
                *end = *end + d;
            }
            AnnotKind::Polygon { points } | AnnotKind::Polyline { points, .. } => {
                for p in points.iter_mut() {
                    *p = *p + d;
                }
            }
            AnnotKind::FreeText { callout, .. } => {
                if let Some(c) = callout {
                    for p in c.iter_mut() {
                        *p = *p + d;
                    }
                }
            }
            _ => {}
        }
        self.touch();
    }

    /// A one-line description for the comments panel.
    pub fn summary(&self) -> String {
        if !self.contents.trim().is_empty() {
            let first = self.contents.lines().next().unwrap_or_default();
            return first.chars().take(80).collect();
        }
        match &self.kind {
            AnnotKind::FreeText { text, .. } => text.chars().take(80).collect(),
            AnnotKind::Stamp { name, .. } => name.clone(),
            k => k.subtype_name().to_owned(),
        }
    }

    /// Total comments in the thread, counting the annotation itself.
    pub fn thread_len(&self) -> usize {
        1 + self.replies.len()
    }
}

fn union_all(mut it: impl Iterator<Item = Rect>) -> Option<Rect> {
    let first = it.next()?;
    Some(it.fold(first, |a, b| a.union(&b)))
}

/// Perpendicular distance from `p` to the segment `a`–`b`.
///
/// A degenerate segment (a == b) falls back to point distance rather than
/// dividing by zero, which is what a single-point ink stroke produces.
pub fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 <= f32::EPSILON {
        return (p - a).length();
    }
    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / len2).clamp(0.0, 1.0);
    let proj = Vec2::new(a.x + ab.x * t, a.y + ab.y * t);
    (p - proj).length()
}

/// Current time as an RFC 3339 string.
///
/// `quark-core` has no clock dependency, so this is built from the system
/// clock directly; the PDF layer converts it to a PDF date string on save.
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_epoch(secs)
}

/// Formats seconds-since-epoch as RFC 3339 UTC.
///
/// Written out rather than pulled from a date crate because this crate stays
/// dependency-free, and the civil-from-days algorithm is short and exact.
pub fn format_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(kind: AnnotKind) -> Annotation {
        Annotation::new(AnnotId(1), 0, kind, Rect::ZERO, "tester")
    }

    #[test]
    fn highlights_default_to_translucent_yellow() {
        // An opaque highlight hides the text it marks, which is the whole point
        // of the annotation.
        let h = a(AnnotKind::Highlight { quads: vec![] });
        assert_eq!(h.color, Color::YELLOW);
        assert!(h.opacity < 1.0);

        let s = a(AnnotKind::Square);
        assert_eq!(s.opacity, 1.0);
    }

    #[test]
    fn colour_roundtrips_through_normalised_form() {
        let c = Color::rgba(18, 200, 77, 128);
        let back = Color::from_f32(c.to_f32());
        assert_eq!(c, back);
    }

    #[test]
    fn contrasting_text_flips_on_light_backgrounds() {
        assert_eq!(Color::WHITE.contrasting_text(), Color::BLACK);
        assert_eq!(Color::BLACK.contrasting_text(), Color::WHITE);
        assert_eq!(Color::YELLOW.contrasting_text(), Color::BLACK);
    }

    #[test]
    fn recompute_rect_covers_every_quad() {
        let mut ann = a(AnnotKind::Highlight {
            quads: vec![
                Rect::from_xywh(10.0, 100.0, 50.0, 12.0),
                Rect::from_xywh(10.0, 80.0, 90.0, 12.0),
            ],
        });
        ann.recompute_rect();
        assert_eq!(ann.rect.min, Vec2::new(10.0, 80.0));
        assert_eq!(ann.rect.max, Vec2::new(100.0, 112.0));
    }

    #[test]
    fn recompute_rect_pads_ink_by_the_pen_width() {
        let mut ann = a(AnnotKind::Ink {
            strokes: vec![InkStroke {
                points: vec![Vec2::new(10.0, 10.0), Vec2::new(30.0, 40.0)],
            }],
        });
        ann.border_width = 4.0;
        ann.recompute_rect();
        // Half the pen width falls outside the path on each side.
        assert_eq!(ann.rect.min, Vec2::new(8.0, 8.0));
        assert_eq!(ann.rect.max, Vec2::new(32.0, 42.0));
    }

    #[test]
    fn empty_ink_leaves_the_rect_alone() {
        let mut ann = a(AnnotKind::Ink { strokes: vec![] });
        ann.rect = Rect::from_xywh(1.0, 2.0, 3.0, 4.0);
        ann.recompute_rect();
        assert_eq!(ann.rect, Rect::from_xywh(1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn text_markup_hit_test_uses_quads_not_the_bounding_box() {
        // Two lines: a long one and a short one. The bounding box includes the
        // empty space to the right of the short line.
        let ann = a(AnnotKind::Highlight {
            quads: vec![
                Rect::from_xywh(10.0, 100.0, 200.0, 12.0),
                Rect::from_xywh(10.0, 80.0, 40.0, 12.0),
            ],
        });
        assert!(ann.hit_test(Vec2::new(150.0, 105.0), 0.0));
        // Inside the bounding box, but past the end of the short second line.
        assert!(!ann.hit_test(Vec2::new(150.0, 85.0), 0.0));
    }

    #[test]
    fn ink_hit_test_follows_the_stroke() {
        let mut ann = a(AnnotKind::Ink {
            strokes: vec![InkStroke {
                points: vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)],
            }],
        });
        ann.border_width = 1.0;
        assert!(ann.hit_test(Vec2::new(50.0, 0.5), 1.0));
        // Well off the line, though inside its bounding box after expansion.
        assert!(!ann.hit_test(Vec2::new(50.0, 40.0), 1.0));
    }

    #[test]
    fn hidden_annotations_are_never_hit() {
        let mut ann = a(AnnotKind::Square);
        ann.rect = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        assert!(ann.hit_test(Vec2::new(50.0, 50.0), 0.0));
        ann.hidden = true;
        assert!(!ann.hit_test(Vec2::new(50.0, 50.0), 0.0));
    }

    #[test]
    fn translate_moves_geometry_and_not_just_the_rect() {
        let mut ann = a(AnnotKind::Ink {
            strokes: vec![InkStroke {
                points: vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)],
            }],
        });
        ann.recompute_rect();
        let before = ann.rect;
        ann.translate(Vec2::new(5.0, -3.0));
        match &ann.kind {
            AnnotKind::Ink { strokes } => {
                assert_eq!(strokes[0].points[0], Vec2::new(5.0, -3.0));
                assert_eq!(strokes[0].points[1], Vec2::new(15.0, 7.0));
            }
            _ => unreachable!(),
        }
        assert_eq!(ann.rect.min, before.min + Vec2::new(5.0, -3.0));
    }

    #[test]
    fn translating_a_line_keeps_its_length() {
        let mut ann = a(AnnotKind::Line {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(30.0, 40.0),
            start_ending: LineEnding::None,
            end_ending: LineEnding::OpenArrow,
            measure: None,
        });
        ann.translate(Vec2::new(100.0, 100.0));
        match &ann.kind {
            AnnotKind::Line { start, end, .. } => {
                assert_eq!((*end - *start).length(), 50.0);
                assert_eq!(*start, Vec2::new(100.0, 100.0));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn distance_to_a_degenerate_segment_is_point_distance() {
        let p = Vec2::new(3.0, 4.0);
        let d = dist_to_segment(p, Vec2::ZERO, Vec2::ZERO);
        assert!((d - 5.0).abs() < 1e-5);
    }

    #[test]
    fn distance_clamps_to_the_segment_ends() {
        // Beyond the end of the segment, distance is to the endpoint, not to
        // the infinite line.
        let d = dist_to_segment(Vec2::new(20.0, 0.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
        assert!((d - 10.0).abs() < 1e-5);
    }

    #[test]
    fn measure_converts_points_to_the_chosen_unit() {
        let m = Measure {
            unit: MeasureUnit::Inches,
            scale: 1.0,
        };
        assert_eq!(m.format(144.0), "2.00 in");
        let m = Measure {
            unit: MeasureUnit::Centimetres,
            scale: 1.0,
        };
        assert_eq!(m.format(72.0), "2.54 cm");
        // A scaled drawing: 1 inch on the page is 10 real inches.
        let m = Measure {
            unit: MeasureUnit::Inches,
            scale: 10.0,
        };
        assert_eq!(m.format(72.0), "10.00 in");
    }

    #[test]
    fn summary_prefers_the_comment_then_falls_back_to_the_type() {
        let mut ann = a(AnnotKind::Square);
        assert_eq!(ann.summary(), "Square");
        ann.contents = "Check this figure\nsecond line".into();
        assert_eq!(ann.summary(), "Check this figure");
    }

    #[test]
    fn epoch_formatting_matches_known_dates() {
        assert_eq!(format_epoch(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_epoch(1_000_000_000), "2001-09-09T01:46:40Z");
        // A leap day, which is where a hand-rolled calendar usually breaks.
        assert_eq!(format_epoch(1_583_020_800), "2020-03-01T00:00:00Z");
        assert_eq!(format_epoch(1_582_934_400), "2020-02-29T00:00:00Z");
    }

    #[test]
    fn thread_length_counts_the_root_comment() {
        let mut ann = a(AnnotKind::Note {
            icon: NoteIcon::Comment,
        });
        assert_eq!(ann.thread_len(), 1);
        ann.replies.push(Reply {
            author: "b".into(),
            contents: "ok".into(),
            created: now_rfc3339(),
        });
        assert_eq!(ann.thread_len(), 2);
    }
}
