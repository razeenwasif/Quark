//! Undo/redo.
//!
//! Quark records *inverse edits* rather than whole-document snapshots. A
//! snapshot stack is far simpler, but a 200MB scanned document would make every
//! highlight cost a 200MB copy, and the undo depth users expect from Acrobat
//! is effectively unlimited.
//!
//! Each [`Edit`] therefore knows how to undo itself, and the two stacks hold
//! only the deltas.

use crate::annot::Annotation;
use crate::geom::Rot;

/// A single reversible change.
#[derive(Debug, Clone)]
pub enum Edit {
    AddAnnotation(Box<Annotation>),
    /// Holds the removed annotation so it can be put back.
    RemoveAnnotation(Box<Annotation>),
    /// Before and after, so the edit reverses in either direction.
    ModifyAnnotation {
        before: Box<Annotation>,
        after: Box<Annotation>,
    },
    /// Pages inserted at `at`, in order.
    InsertPages {
        at: usize,
        count: usize,
    },
    /// Pages deleted. Their content lives in the document's trash list, keyed
    /// by this token, because holding page bytes here would defeat the point.
    DeletePages {
        indices: Vec<usize>,
        restore_token: u64,
    },
    MovePage {
        from: usize,
        to: usize,
    },
    RotatePages {
        indices: Vec<usize>,
        by: Rot,
    },
    /// A form field's value changed.
    SetFieldValue {
        page: usize,
        field: String,
        before: String,
        after: String,
    },
    /// Several edits applied together and undone together.
    Group(Vec<Edit>),
}

impl Edit {
    /// A short description, shown on the Undo menu item.
    pub fn label(&self) -> String {
        match self {
            Edit::AddAnnotation(a) => format!("Add {}", a.kind.subtype_name()),
            Edit::RemoveAnnotation(a) => format!("Delete {}", a.kind.subtype_name()),
            Edit::ModifyAnnotation { after, .. } => {
                format!("Edit {}", after.kind.subtype_name())
            }
            Edit::InsertPages { count, .. } => {
                format!("Insert {count} Page{}", plural(*count))
            }
            Edit::DeletePages { indices, .. } => {
                format!("Delete {} Page{}", indices.len(), plural(indices.len()))
            }
            Edit::MovePage { .. } => "Move Page".into(),
            Edit::RotatePages { indices, .. } => {
                format!("Rotate {} Page{}", indices.len(), plural(indices.len()))
            }
            Edit::SetFieldValue { field, .. } => format!("Fill \"{field}\""),
            // A group is named for what it mostly did, which is more useful
            // than "Group of 12".
            Edit::Group(edits) => edits
                .first()
                .map(|e| e.label())
                .unwrap_or_else(|| "Edit".into()),
        }
    }

    /// The edit that reverses this one.
    pub fn inverse(&self) -> Edit {
        match self {
            Edit::AddAnnotation(a) => Edit::RemoveAnnotation(a.clone()),
            Edit::RemoveAnnotation(a) => Edit::AddAnnotation(a.clone()),
            Edit::ModifyAnnotation { before, after } => Edit::ModifyAnnotation {
                before: after.clone(),
                after: before.clone(),
            },
            Edit::InsertPages { at, count } => Edit::DeletePages {
                indices: (*at..*at + *count).collect(),
                restore_token: 0,
            },
            Edit::DeletePages {
                indices,
                restore_token,
            } => Edit::InsertPages {
                at: indices.first().copied().unwrap_or(0),
                count: indices.len(),
            }
            .tagged(*restore_token),
            Edit::MovePage { from, to } => Edit::MovePage {
                from: *to,
                to: *from,
            },
            Edit::RotatePages { indices, by } => Edit::RotatePages {
                indices: indices.clone(),
                // Undoing a rotation is rotating back by the complement.
                by: Rot::from_degrees(-by.degrees()),
            },
            Edit::SetFieldValue {
                page,
                field,
                before,
                after,
            } => Edit::SetFieldValue {
                page: *page,
                field: field.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            // A group reverses in the opposite order, or later edits would be
            // undone against state their predecessors had not yet restored.
            Edit::Group(edits) => Edit::Group(edits.iter().rev().map(Edit::inverse).collect()),
        }
    }

    fn tagged(self, _token: u64) -> Edit {
        self
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Bounded undo and redo stacks.
#[derive(Debug)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    limit: usize,
    /// Position of the last save, used to answer "are there unsaved changes?".
    ///
    /// `None` means the saved state is no longer reachable — it was dropped off
    /// the bottom of the bounded stack, so we can never again be sure the
    /// document matches the file.
    saved_depth: Option<usize>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(500)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            saved_depth: Some(0),
        }
    }

    /// Records an edit that has already been applied.
    pub fn push(&mut self, edit: Edit) {
        // Any new edit invalidates the redo branch.
        self.redo.clear();
        self.undo.push(edit);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
            match &mut self.saved_depth {
                // The save point shifts down with everything else, and is lost
                // entirely once it falls off the bottom.
                Some(0) => self.saved_depth = None,
                Some(d) => *d -= 1,
                None => {}
            }
        }
    }

    /// Pops the next edit to undo. The caller applies its inverse.
    pub fn undo(&mut self) -> Option<Edit> {
        let e = self.undo.pop()?;
        self.redo.push(e.clone());
        Some(e)
    }

    /// Pops the next edit to redo. The caller re-applies it.
    pub fn redo(&mut self) -> Option<Edit> {
        let e = self.redo.pop()?;
        self.undo.push(e.clone());
        Some(e)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<String> {
        self.undo.last().map(|e| e.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|e| e.label())
    }

    /// Marks the current position as saved.
    pub fn mark_saved(&mut self) {
        self.saved_depth = Some(self.undo.len());
    }

    /// Whether the document differs from the last save.
    pub fn is_dirty(&self) -> bool {
        match self.saved_depth {
            Some(d) => self.undo.len() != d,
            // Unknown: assume dirty rather than risk losing work silently.
            None => true,
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.saved_depth = Some(0);
    }

    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annot::{AnnotId, AnnotKind, Annotation};
    use crate::geom::Rect;

    fn ann(id: u64) -> Box<Annotation> {
        Box::new(Annotation::new(
            AnnotId(id),
            0,
            AnnotKind::Square,
            Rect::ZERO,
            "t",
        ))
    }

    #[test]
    fn undo_then_redo_returns_the_same_edit() {
        let mut h = History::default();
        h.push(Edit::AddAnnotation(ann(1)));
        let e = h.undo().unwrap();
        assert!(matches!(e, Edit::AddAnnotation(_)));
        assert!(!h.can_undo());
        assert!(h.can_redo());
        let e = h.redo().unwrap();
        assert!(matches!(e, Edit::AddAnnotation(_)));
        assert!(h.can_undo());
    }

    #[test]
    fn a_new_edit_discards_the_redo_branch() {
        let mut h = History::default();
        h.push(Edit::AddAnnotation(ann(1)));
        h.undo();
        assert!(h.can_redo());
        h.push(Edit::AddAnnotation(ann(2)));
        assert!(!h.can_redo(), "redo must not survive a divergent edit");
    }

    #[test]
    fn add_and_remove_are_inverses() {
        let e = Edit::AddAnnotation(ann(7));
        match e.inverse() {
            Edit::RemoveAnnotation(a) => assert_eq!(a.id, AnnotId(7)),
            other => panic!("wrong inverse: {other:?}"),
        }
    }

    #[test]
    fn modify_inverse_swaps_before_and_after() {
        let mut before = ann(1);
        before.contents = "old".into();
        let mut after = ann(1);
        after.contents = "new".into();
        let e = Edit::ModifyAnnotation {
            before: before.clone(),
            after: after.clone(),
        };
        match e.inverse() {
            Edit::ModifyAnnotation { before: b, after: a } => {
                assert_eq!(b.contents, "new");
                assert_eq!(a.contents, "old");
            }
            other => panic!("wrong inverse: {other:?}"),
        }
    }

    #[test]
    fn rotation_inverse_turns_the_other_way() {
        let e = Edit::RotatePages {
            indices: vec![0, 1],
            by: Rot::D90,
        };
        match e.inverse() {
            Edit::RotatePages { by, .. } => assert_eq!(by, Rot::D270),
            other => panic!("wrong inverse: {other:?}"),
        }
        // And 180 is its own inverse.
        let e = Edit::RotatePages {
            indices: vec![0],
            by: Rot::D180,
        };
        match e.inverse() {
            Edit::RotatePages { by, .. } => assert_eq!(by, Rot::D180),
            other => panic!("wrong inverse: {other:?}"),
        }
    }

    #[test]
    fn group_inverse_reverses_the_order() {
        // Undoing "insert then rotate" must undo the rotate first, or the
        // rotate is undone against pages that no longer exist.
        let g = Edit::Group(vec![
            Edit::InsertPages { at: 0, count: 1 },
            Edit::RotatePages {
                indices: vec![0],
                by: Rot::D90,
            },
        ]);
        match g.inverse() {
            Edit::Group(items) => {
                assert!(matches!(items[0], Edit::RotatePages { .. }));
                assert!(matches!(items[1], Edit::DeletePages { .. }));
            }
            other => panic!("wrong inverse: {other:?}"),
        }
    }

    #[test]
    fn move_page_inverse_moves_it_back() {
        match (Edit::MovePage { from: 2, to: 9 }).inverse() {
            Edit::MovePage { from, to } => {
                assert_eq!((from, to), (9, 2));
            }
            other => panic!("wrong inverse: {other:?}"),
        }
    }

    #[test]
    fn dirty_tracks_the_save_point_in_both_directions() {
        let mut h = History::default();
        assert!(!h.is_dirty());
        h.push(Edit::AddAnnotation(ann(1)));
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        // Undoing past the save point is also a difference from the file.
        h.undo();
        assert!(h.is_dirty());
        h.redo();
        assert!(!h.is_dirty(), "returning to the save point is clean again");
    }

    #[test]
    fn the_stack_is_bounded_and_drops_the_oldest_edits() {
        let mut h = History::new(3);
        for i in 0..5 {
            h.push(Edit::AddAnnotation(ann(i)));
        }
        assert_eq!(h.depth(), 3);
    }

    #[test]
    fn losing_the_save_point_off_the_stack_reports_dirty() {
        // If we can no longer prove the document matches the file, the safe
        // answer is that it does not.
        let mut h = History::new(2);
        h.mark_saved();
        for i in 0..4 {
            h.push(Edit::AddAnnotation(ann(i)));
        }
        for _ in 0..2 {
            h.undo();
        }
        assert!(h.is_dirty());
    }

    #[test]
    fn labels_pluralise_correctly() {
        assert_eq!(
            Edit::InsertPages { at: 0, count: 1 }.label(),
            "Insert 1 Page"
        );
        assert_eq!(
            Edit::InsertPages { at: 0, count: 4 }.label(),
            "Insert 4 Pages"
        );
    }
}
