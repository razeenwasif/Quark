//! Splits: several views onto the buffers.
//!
//! A split is a **view**, not a copy. It knows which buffer it is showing,
//! where its cursor is and where it is scrolled to — so two splits on one file
//! scroll independently and keep their own cursors, which is the whole reason
//! to open the same file twice.
//!
//! ## Where the cursor lives
//!
//! The cursor is a field on the [`Buffer`](quark_text::Buffer), and every
//! motion and operator in `editor.rs` reaches for it there. Moving it onto the
//! view would mean threading it through all of them.
//!
//! It does not have to be. **Only the focused view receives commands**, so its
//! is the only cursor that can move; every other view's is frozen for as long
//! as it is unfocused. So the buffer holds the focused view's cursor, each
//! other view holds its own, and they are exchanged when the focus moves. One
//! swap per focus change, and nothing below has to know splits exist.
//!
//! ## One row, not a tree
//!
//! Vim nests: a window can be split, and the halves split again, and the
//! layout is a tree. This is a single row or column. That covers what people
//! actually do — two or three panes side by side — and a tree is a great deal
//! of layout code for the case it does not cover. The shape here is a `Vec`
//! and a direction, which a tree can replace later without the rest of the
//! core noticing.

use quark_text::Cursor;
use serde::{Deserialize, Serialize};

/// How many splits are worth having. Past this they are slivers, and the
/// answer to "I need another one" is a second window.
const LIMIT: usize = 4;

/// One view onto a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    /// Which buffer it is showing, as an index into the editor's list.
    pub buffer: usize,
    /// Where this view's cursor is.
    ///
    /// Only meaningful while the view is *not* focused — the focused view's
    /// cursor lives on the buffer, where every operator can reach it. See the
    /// module note.
    pub cursor: Cursor,
    /// The top line on screen.
    pub first_line: usize,
    /// How many lines it can show. Reported by the front end, which is the
    /// only thing that knows how tall it is.
    pub lines: usize,
    /// Where this view is in a document that has no lines — the cell, in a
    /// notebook.
    ///
    /// Same rule as `cursor`, for the same reason: only meaningful while the
    /// view is *not* focused. The focused view's cell lives on `Data`, where
    /// every verb can reach it without knowing splits exist.
    pub cell: usize,
}

impl Default for View {
    fn default() -> Self {
        View {
            buffer: 0,
            cursor: Cursor::default(),
            first_line: 0,
            // Enough to be useful before the front end reports its real height.
            lines: 80,
            cell: 0,
        }
    }
}

/// Which way to move focus. `⌃w h` and friends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
    /// The next one round, wrapping — `⌃w w`.
    Next,
    /// The one before, wrapping — `⌃w W`.
    Previous,
}

#[derive(Debug)]
pub struct Splits {
    views: Vec<View>,
    focused: usize,
    /// Side by side rather than stacked.
    vertical: bool,
}

impl Default for Splits {
    fn default() -> Self {
        Splits {
            views: vec![View::default()],
            focused: 0,
            vertical: true,
        }
    }
}

impl Splits {
    pub fn views(&self) -> &[View] {
        &self.views
    }

    pub fn focused(&self) -> usize {
        self.focused
    }

    pub fn is_vertical(&self) -> bool {
        self.vertical
    }

    pub fn len(&self) -> usize {
        self.views.len()
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    pub fn view(&self) -> &View {
        &self.views[self.focused]
    }

    pub fn view_mut(&mut self) -> &mut View {
        &mut self.views[self.focused]
    }

    pub fn at(&self, index: usize) -> Option<&View> {
        self.views.get(index)
    }

    pub fn at_mut(&mut self, index: usize) -> Option<&mut View> {
        self.views.get_mut(index)
    }

    /// Every view showing `buffer`, focused or not.
    ///
    /// What tells an edit which other panes have to be redrawn: two views on
    /// one file must not disagree about what is in it, even for a frame.
    pub fn showing(&self, buffer: usize) -> Vec<usize> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.buffer == buffer)
            .map(|(index, _)| index)
            .collect()
    }

    /// Split the focused view in two, and focus the new one.
    ///
    /// The new one starts as a copy: same buffer, same cursor, same scroll.
    /// That is what `:vsplit` does, and it is the only sensible starting point
    /// — you split because you want two of what you are looking at.
    ///
    /// `None` when there are already as many as are worth having.
    pub fn split(&mut self, vertical: bool) -> Option<usize> {
        if self.views.len() >= LIMIT {
            return None;
        }
        // The direction is a property of the row, not of one split, because
        // the layout is one row. Splitting the other way turns the whole thing.
        self.vertical = vertical;
        let copy = self.views[self.focused];
        self.views.insert(self.focused + 1, copy);
        self.focused += 1;
        Some(self.focused)
    }

    /// Close one. `None` when it is the last, which stays.
    pub fn close(&mut self, index: usize) -> Option<usize> {
        if index >= self.views.len() || self.views.len() == 1 {
            return None;
        }
        self.views.remove(index);
        if index < self.focused || self.focused >= self.views.len() {
            self.focused = self.focused.saturating_sub(1);
        }
        Some(self.focused)
    }

    /// Close every split but the focused one — `:only`.
    pub fn only(&mut self) {
        let keep = self.views[self.focused];
        self.views = vec![keep];
        self.focused = 0;
    }

    /// Put a stored layout back.
    ///
    /// Everything is clamped rather than trusted. The file it came from was
    /// written by a previous run and describes a world that has since changed:
    /// a buffer it names may not have opened, and a pane count from a build
    /// with a different limit would otherwise leave a view pointing at
    /// nothing. A layout that cannot be honoured exactly is worth honouring
    /// approximately — the alternative is opening to one empty pane.
    pub fn restore(&mut self, views: Vec<View>, focused: usize, vertical: bool, buffers: usize) {
        let kept: Vec<View> = views
            .into_iter()
            .take(LIMIT)
            .map(|mut view| {
                view.buffer = view.buffer.min(buffers.saturating_sub(1));
                view
            })
            .collect();
        if kept.is_empty() {
            return;
        }
        self.focused = focused.min(kept.len() - 1);
        self.views = kept;
        self.vertical = vertical;
    }

    /// Focus one outright. `false` if there is no such split.
    pub fn focus(&mut self, index: usize) -> bool {
        if index >= self.views.len() {
            return false;
        }
        self.focused = index;
        true
    }

    /// Move the focus. `None` when there is nowhere to go, so a caller can
    /// tell "already at the edge" from "moved".
    ///
    /// The layout is one row or one column, so half the directions are along
    /// it and half are across it — and across it there is nothing. Vim does
    /// the same thing in the same layout.
    pub fn step(&mut self, direction: Direction) -> Option<usize> {
        let count = self.views.len();
        if count < 2 {
            return None;
        }
        let along = |back: bool| -> Option<usize> {
            if back {
                self.focused.checked_sub(1)
            } else {
                (self.focused + 1 < count).then_some(self.focused + 1)
            }
        };

        let next = match direction {
            // Wrapping, because "the next one" has to have somewhere to go.
            Direction::Next => Some((self.focused + 1) % count),
            Direction::Previous => Some((self.focused + count - 1) % count),
            Direction::Left if self.vertical => along(true),
            Direction::Right if self.vertical => along(false),
            Direction::Up if !self.vertical => along(true),
            Direction::Down if !self.vertical => along(false),
            // Across the grain of the layout: there is nothing there.
            _ => None,
        }?;
        self.focused = next;
        Some(next)
    }

    /// Point every view at a buffer that still exists.
    ///
    /// Called after a buffer closes, when the indexes below it have all
    /// shifted down. A view left pointing past the end would panic the next
    /// time it drew.
    pub fn buffer_closed(&mut self, closed: usize, remaining: usize) {
        for view in &mut self.views {
            if view.buffer > closed {
                view.buffer -= 1;
            } else if view.buffer == closed {
                // Its file is gone; show whatever took its place.
                view.buffer = closed.min(remaining.saturating_sub(1));
                view.cursor = Cursor::default();
                view.first_line = 0;
            }
            view.buffer = view.buffer.min(remaining.saturating_sub(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_always_one() {
        let splits = Splits::default();
        assert_eq!(splits.len(), 1);
        assert_eq!(splits.focused(), 0);
    }

    #[test]
    fn splitting_copies_what_you_were_looking_at_and_focuses_it() {
        // You split because you want two of what you are looking at.
        let mut splits = Splits::default();
        splits.view_mut().buffer = 3;
        splits.view_mut().first_line = 40;

        assert_eq!(splits.split(true), Some(1));
        assert_eq!(splits.len(), 2);
        assert_eq!(splits.focused(), 1, "the new one takes the focus");
        assert_eq!(splits.view().buffer, 3);
        assert_eq!(splits.view().first_line, 40, "scrolled where you were");
    }

    #[test]
    fn there_is_a_limit_and_it_is_refused_rather_than_ignored() {
        let mut splits = Splits::default();
        for _ in 1..4 {
            assert!(splits.split(true).is_some());
        }
        assert_eq!(splits.len(), 4);
        assert_eq!(splits.split(true), None, "the fifth is slivers");
    }

    #[test]
    fn the_last_split_cannot_be_closed() {
        // There is no editor with no view in it.
        let mut splits = Splits::default();
        assert_eq!(splits.close(0), None);
        assert_eq!(splits.len(), 1);
    }

    #[test]
    fn closing_one_before_you_keeps_you_in_the_same_view() {
        let mut splits = Splits::default();
        splits.split(true);
        splits.split(true);
        splits.views[0].buffer = 10;
        splits.views[1].buffer = 11;
        splits.views[2].buffer = 12;
        assert_eq!(splits.focused(), 2);

        splits.close(0);
        assert_eq!(splits.view().buffer, 12, "still looking at the same one");
    }

    #[test]
    fn closing_the_focused_one_lands_on_a_neighbour() {
        let mut splits = Splits::default();
        splits.split(true);
        splits.views[0].buffer = 10;
        splits.views[1].buffer = 11;

        splits.close(1);
        assert_eq!(splits.len(), 1);
        assert_eq!(splits.view().buffer, 10);
    }

    #[test]
    fn only_keeps_the_one_you_are_in() {
        let mut splits = Splits::default();
        splits.split(true);
        splits.split(true);
        splits.view_mut().buffer = 7;

        splits.only();
        assert_eq!(splits.len(), 1);
        assert_eq!(splits.view().buffer, 7);
    }

    #[test]
    fn focus_moves_along_the_layout_and_stops_at_the_edge() {
        let mut splits = Splits::default();
        splits.split(true); // vertical: a row, so left/right
        splits.split(true);
        assert_eq!(splits.focused(), 2);

        assert_eq!(splits.step(Direction::Right), None, "already at the end");
        assert_eq!(splits.step(Direction::Left), Some(1));
        assert_eq!(splits.step(Direction::Left), Some(0));
        assert_eq!(splits.step(Direction::Left), None, "and at the start");
    }

    #[test]
    fn focus_does_not_move_across_the_grain() {
        // A row has no up and down. Vim does the same in the same layout.
        let mut splits = Splits::default();
        splits.split(true);
        assert_eq!(splits.step(Direction::Up), None);
        assert_eq!(splits.step(Direction::Down), None);

        let mut stacked = Splits::default();
        stacked.split(false);
        assert_eq!(stacked.step(Direction::Left), None);
        assert_eq!(stacked.step(Direction::Up), Some(0));
    }

    #[test]
    fn next_wraps_because_it_has_to_go_somewhere() {
        let mut splits = Splits::default();
        splits.split(true);
        assert_eq!(splits.focused(), 1);
        assert_eq!(splits.step(Direction::Next), Some(0));
        assert_eq!(splits.step(Direction::Previous), Some(1));
    }

    #[test]
    fn a_closed_buffer_does_not_leave_a_view_pointing_past_the_end() {
        // The indexes below it all shifted down, and a view still pointing at
        // the old one would panic the next time it drew.
        let mut splits = Splits::default();
        splits.split(true);
        splits.split(true);
        splits.views[0].buffer = 0;
        splits.views[1].buffer = 1;
        splits.views[2].buffer = 2;

        // Buffer 1 closed; two remain.
        splits.buffer_closed(1, 2);
        assert_eq!(splits.views[0].buffer, 0, "below it, unmoved");
        assert_eq!(splits.views[1].buffer, 1, "it was showing the closed one");
        assert_eq!(splits.views[2].buffer, 1, "above it, shifted down");
        assert!(splits.views.iter().all(|v| v.buffer < 2));
    }

    #[test]
    fn every_view_showing_a_buffer_can_be_found() {
        // Two views on one file must not disagree about what is in it, so an
        // edit has to know which other panes to redraw.
        let mut splits = Splits::default();
        splits.split(true);
        splits.split(true);
        splits.views[0].buffer = 5;
        splits.views[1].buffer = 9;
        splits.views[2].buffer = 5;

        assert_eq!(splits.showing(5), vec![0, 2]);
        assert_eq!(splits.showing(9), vec![1]);
        assert!(splits.showing(11).is_empty());
    }
}
