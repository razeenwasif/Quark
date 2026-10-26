//! Edits and cursors.
//!
//! Everything here counts in **character offsets**, which is ropey's native
//! unit. Bytes and UTF-16 code units are conversions done at the edges —
//! `Buffer::position` for the front end, `Buffer::utf16_of` for LSP — because
//! doing them anywhere else is how off-by-one bugs get into a rope.

use serde::{Deserialize, Serialize};

/// A splice: replace `start..end` with `text`. An insertion has
/// `start == end`; a deletion has an empty `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            start: at,
            end: at,
            text: text.into(),
        }
    }

    pub fn delete(range: Range) -> Self {
        Self {
            start: range.start,
            end: range.end,
            text: String::new(),
        }
    }

    pub fn replace(range: Range, text: impl Into<String>) -> Self {
        Self {
            start: range.start,
            end: range.end,
            text: text.into(),
        }
    }

    /// How many characters this edit leaves behind.
    pub fn inserted_len(&self) -> usize {
        self.text.chars().count()
    }
}

/// A half-open character range, always normalised so `start <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    pub fn new(a: usize, b: usize) -> Self {
        Self {
            start: a.min(b),
            end: a.max(b),
        }
    }

    pub fn empty(at: usize) -> Self {
        Self { start: at, end: at }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Where the cursor is, and where its selection started.
///
/// `anchor == offset` means no selection, which is the normal-mode state.
/// Visual mode simply stops moving the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub offset: usize,
    pub anchor: usize,
    /// The column `j`/`k` try to return to. Moving through a short line must
    /// not forget how far right you were — that is the difference between a
    /// vim motion and a text-area caret.
    #[serde(default)]
    pub goal_column: Option<usize>,
}

impl Cursor {
    pub fn at(offset: usize) -> Self {
        Self {
            offset,
            anchor: offset,
            goal_column: None,
        }
    }

    pub fn selection(&self) -> Range {
        Range::new(self.anchor, self.offset)
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.offset
    }

    /// Collapse to a caret at `offset`, forgetting the goal column.
    pub fn moved_to(self, offset: usize) -> Self {
        Self {
            offset,
            anchor: offset,
            goal_column: None,
        }
    }

    /// Move the head, leaving the anchor — how visual mode extends.
    pub fn extended_to(self, offset: usize) -> Self {
        Self {
            offset,
            anchor: self.anchor,
            goal_column: None,
        }
    }
}

/// A line and column, for the statusline and for anything outside the core.
/// Both are zero-based; the UI adds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}
