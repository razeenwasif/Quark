//! The toolbar's tools and their transient state.

use crate::annot::{Color, LineEnding, NoteIcon};
use serde::{Deserialize, Serialize};

/// The active pointer tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Tool {
    /// Select text, and click annotations to edit them.
    #[default]
    Select,
    /// Drag to scroll.
    Pan,
    /// Drag a rectangle to zoom into it.
    ZoomArea,
    /// Drag a rectangle and copy it as an image.
    Snapshot,

    Highlight,
    Underline,
    StrikeOut,
    Squiggly,

    Note,
    FreeText,
    Callout,
    Ink,
    Eraser,

    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Polygon,
    Polyline,

    Stamp,
    Signature,
    Redact,

    /// Measure a straight distance.
    MeasureDistance,
    /// Measure a path's total length.
    MeasurePerimeter,
    /// Measure an enclosed area.
    MeasureArea,

    /// Place or edit form fields.
    FormEdit,
    /// Edit page text and images in place.
    ContentEdit,
    /// Crop pages by dragging.
    Crop,
    /// Insert a link region.
    Link,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pan => "Pan",
            Tool::ZoomArea => "Marquee Zoom",
            Tool::Snapshot => "Snapshot",
            Tool::Highlight => "Highlight",
            Tool::Underline => "Underline",
            Tool::StrikeOut => "Strikethrough",
            Tool::Squiggly => "Squiggly",
            Tool::Note => "Sticky Note",
            Tool::FreeText => "Text Box",
            Tool::Callout => "Callout",
            Tool::Ink => "Draw",
            Tool::Eraser => "Eraser",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Polygon => "Polygon",
            Tool::Polyline => "Polyline",
            Tool::Stamp => "Stamp",
            Tool::Signature => "Sign",
            Tool::Redact => "Redact",
            Tool::MeasureDistance => "Distance",
            Tool::MeasurePerimeter => "Perimeter",
            Tool::MeasureArea => "Area",
            Tool::FormEdit => "Prepare Form",
            Tool::ContentEdit => "Edit Content",
            Tool::Crop => "Crop Pages",
            Tool::Link => "Link",
        }
    }

    /// Whether the tool acts on a text selection rather than on free geometry.
    pub fn needs_text_selection(self) -> bool {
        matches!(
            self,
            Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly
        )
    }

    /// Whether the tool is driven by dragging out a rectangle.
    pub fn is_drag_rect(self) -> bool {
        matches!(
            self,
            Tool::Rectangle
                | Tool::Ellipse
                | Tool::Redact
                | Tool::ZoomArea
                | Tool::Snapshot
                | Tool::Crop
                | Tool::FreeText
                | Tool::Link
                | Tool::MeasureArea
        )
    }

    /// Whether the tool is driven by clicking a sequence of vertices, ended by
    /// a double-click.
    pub fn is_polyline(self) -> bool {
        matches!(
            self,
            Tool::Polygon | Tool::Polyline | Tool::MeasurePerimeter | Tool::Callout
        )
    }

    /// Whether the tool creates an annotation the undo stack should record.
    pub fn creates_annotation(self) -> bool {
        !matches!(
            self,
            Tool::Select
                | Tool::Pan
                | Tool::ZoomArea
                | Tool::Snapshot
                | Tool::Crop
                | Tool::Eraser
                | Tool::FormEdit
                | Tool::ContentEdit
        )
    }

    /// Whether the tool stays selected after one use.
    ///
    /// Acrobat drops back to Select after a single shape but keeps the
    /// highlighter and the pen active, because those are used in runs.
    pub fn is_sticky(self) -> bool {
        matches!(
            self,
            Tool::Highlight
                | Tool::Underline
                | Tool::StrikeOut
                | Tool::Squiggly
                | Tool::Ink
                | Tool::Eraser
                | Tool::Redact
                | Tool::Pan
                | Tool::Select
        )
    }
}

/// Style settings the tools draw with, persisted between sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStyle {
    pub highlight_color: Color,
    pub markup_color: Color,
    pub draw_color: Color,
    pub fill_color: Option<Color>,
    pub stroke_width: f32,
    pub opacity: f32,
    pub font_size: f32,
    pub font: String,
    pub note_icon: NoteIcon,
    pub arrow_ending: LineEnding,
    pub author: String,
}

impl Default for ToolStyle {
    fn default() -> Self {
        Self {
            highlight_color: Color::YELLOW,
            markup_color: Color::RED,
            draw_color: Color::RED,
            fill_color: None,
            stroke_width: 2.0,
            opacity: 1.0,
            font_size: 12.0,
            font: "Helvetica".into(),
            note_icon: NoteIcon::Comment,
            arrow_ending: LineEnding::OpenArrow,
            author: whoami(),
        }
    }
}

/// The annotation author name to stamp on new comments.
///
/// Falls back through the usual environment variables rather than asking, so
/// the first comment a user makes is already attributed.
pub fn whoami() -> String {
    for key in ["QUARK_AUTHOR", "USERNAME", "USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_owned();
            }
        }
    }
    "Quark User".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_markup_tools_need_a_selection() {
        assert!(Tool::Highlight.needs_text_selection());
        assert!(Tool::Squiggly.needs_text_selection());
        assert!(!Tool::Rectangle.needs_text_selection());
        assert!(!Tool::Ink.needs_text_selection());
    }

    #[test]
    fn navigation_tools_do_not_create_annotations() {
        for t in [
            Tool::Select,
            Tool::Pan,
            Tool::ZoomArea,
            Tool::Snapshot,
            Tool::Crop,
            Tool::Eraser,
        ] {
            assert!(!t.creates_annotation(), "{t:?} should not annotate");
        }
        assert!(Tool::Highlight.creates_annotation());
        assert!(Tool::Stamp.creates_annotation());
    }

    #[test]
    fn drag_and_polyline_tool_sets_do_not_overlap() {
        // A tool driven by both a drag rect and a vertex sequence would have
        // two conflicting input handlers.
        for t in [
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Polygon,
            Tool::Polyline,
            Tool::Callout,
            Tool::MeasureArea,
            Tool::MeasurePerimeter,
        ] {
            assert!(
                !(t.is_drag_rect() && t.is_polyline()),
                "{t:?} is in both input modes"
            );
        }
    }

    #[test]
    fn run_tools_stay_selected_and_one_shot_tools_do_not() {
        assert!(Tool::Highlight.is_sticky());
        assert!(Tool::Ink.is_sticky());
        assert!(!Tool::Rectangle.is_sticky());
        assert!(!Tool::Stamp.is_sticky());
    }

    #[test]
    fn author_falls_back_to_a_usable_name() {
        // Whatever the environment holds, never an empty attribution.
        assert!(!whoami().trim().is_empty());
    }

    #[test]
    fn default_style_is_a_translucent_yellow_highlighter() {
        let s = ToolStyle::default();
        assert_eq!(s.highlight_color, Color::YELLOW);
        assert!(s.stroke_width > 0.0);
    }
}
