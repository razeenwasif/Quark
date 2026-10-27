//! Visual block: the selection that is a rectangle rather than a run.
//!
//! Everything else an editor selects is a range of offsets, which is why
//! everything else shares one code path. A block is not — it is the same
//! columns on several lines, and the text between those columns on different
//! lines is not contiguous. So it gets its own shape here, and the operators
//! ask for a list of ranges instead of one.

use quark_text::edit::Range;
use quark_text::{Buffer, Position};

/// A rectangle of text, in lines and columns.
///
/// `right` is exclusive and `left` is not, the way a range is — but note that
/// both are *columns*, and a line shorter than `left` contributes nothing
/// rather than being clamped into a zero-width sliver at its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub first_line: usize,
    pub last_line: usize,
    pub left: usize,
    pub right: usize,
}

impl Block {
    /// The block the cursor and its anchor describe.
    ///
    /// Either corner may be the anchor — dragging up and left has to give the
    /// same rectangle as dragging down and right — so both are normalised.
    pub fn of(buffer: &Buffer) -> Block {
        let anchor = buffer.position(buffer.cursor.anchor);
        let head = buffer.position(buffer.cursor.offset);
        Block {
            first_line: anchor.line.min(head.line),
            last_line: anchor.line.max(head.line),
            left: anchor.column.min(head.column),
            // Inclusive of the column the cursor is on, as every visual
            // selection is.
            right: anchor.column.max(head.column) + 1,
        }
    }

    pub fn lines(&self) -> std::ops::RangeInclusive<usize> {
        self.first_line..=self.last_line
    }

    /// One range per line, clipped to what that line actually has.
    ///
    /// Lines too short to reach the block give an empty range rather than
    /// being dropped, because the *shape* has to survive: a yanked block that
    /// silently loses its blank rows pastes back crooked.
    pub fn ranges(&self, buffer: &Buffer) -> Vec<Range> {
        self.lines()
            .map(|line| {
                if line >= buffer.len_lines() {
                    let end = buffer.len_chars();
                    return Range::new(end, end);
                }
                let start = buffer.offset_of(Position { line, column: 0 });
                let width = buffer.line(line).chars().count();
                let from = start + self.left.min(width);
                let to = start + self.right.min(width);
                Range::new(from, to.max(from))
            })
            .collect()
    }

    /// Where the cursor goes once the block is gone: its top-left corner.
    pub fn corner(&self, buffer: &Buffer) -> usize {
        buffer.offset_of(Position {
            line: self.first_line,
            column: self.left,
        })
    }
}

/// A block insert waiting to be replicated.
///
/// `I` and `A` type on the first line like any other insert. What makes them
/// block commands is what happens on the way out: whatever was typed is
/// copied to the same column on every other line of the block. Held until
/// then, because until then there is nothing to copy.
#[derive(Debug, Clone)]
pub struct PendingInsert {
    /// Lines *after* the first — the first one is being typed on directly.
    pub lines: Vec<usize>,
    pub column: usize,
    /// Where the typing started on the first line, so the text can be read
    /// back out of the buffer rather than accumulated separately.
    pub from: usize,
    /// `A` past the end of a short line pads it with spaces to get there.
    /// `I` never does — it inserts at a column the line already reaches, or
    /// skips the line.
    pub pad: bool,
}
