//! Undo as a tree, not a stack.
//!
//! A stack loses work: undo three times, type one character, and the three
//! things you undid are gone forever. A tree keeps them as a sibling branch —
//! the edit you just made becomes a *new child* of where you were, and the old
//! branch is still reachable.
//!
//! Redo follows the most recently taken branch, which is what makes the tree
//! behave exactly like a stack until the moment you actually need it not to.

use crate::edit::{Cursor, Edit};

#[derive(Debug)]
struct Node {
    parent: Option<usize>,
    children: Vec<usize>,
    /// The branch redo takes from here — the child visited most recently.
    last_child: Option<usize>,
    /// Edits that move parent's state to this node's state.
    redo: Vec<Edit>,
    /// Edits that move this node's state back to the parent's.
    undo: Vec<Edit>,
    cursor_before: Cursor,
    cursor_after: Cursor,
}

/// What a step through the history asks the buffer to do.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub edits: Vec<Edit>,
    pub cursor: Cursor,
}

#[derive(Debug)]
pub struct History {
    nodes: Vec<Node>,
    current: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        // The root is the empty state the buffer opened in. It is never undone
        // past, which is what makes `undo` at the start a no-op rather than a
        // panic.
        Self {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                last_child: None,
                redo: Vec::new(),
                undo: Vec::new(),
                cursor_before: Cursor::default(),
                cursor_after: Cursor::default(),
            }],
            current: 0,
        }
    }

    /// Record a change. `undo` must be the inverse of `redo`, in an order that
    /// applies cleanly back-to-front.
    pub fn commit(&mut self, redo: Vec<Edit>, undo: Vec<Edit>, before: Cursor, after: Cursor) {
        self.commit_inner(redo, undo, before, after, false)
    }

    /// Fold a change into the node that is already current, so an insert
    /// session undoes as one step. Typing a word after `c` is one thing the
    /// user did, and needing three presses of `u` to take it back is the
    /// wrong answer.
    pub fn join(&mut self, redo: Vec<Edit>, undo: Vec<Edit>, after: Cursor) {
        self.commit_inner(redo, undo, Cursor::default(), after, true)
    }

    fn commit_inner(
        &mut self,
        redo: Vec<Edit>,
        mut undo: Vec<Edit>,
        before: Cursor,
        after: Cursor,
        join: bool,
    ) {
        if redo.is_empty() {
            return;
        }
        // Joining is only possible into a real change; the root is the state
        // the buffer opened in and has no edits of its own.
        if join && self.current != 0 {
            let node = &mut self.nodes[self.current];
            node.redo.extend(redo);
            // Undo runs newest first, so the new inverse goes to the front.
            undo.extend(std::mem::take(&mut node.undo));
            node.undo = undo;
            node.cursor_after = after;
            return;
        }
        let node = self.nodes.len();
        self.nodes.push(Node {
            parent: Some(self.current),
            children: Vec::new(),
            last_child: None,
            redo,
            undo,
            cursor_before: before,
            cursor_after: after,
        });
        let parent = &mut self.nodes[self.current];
        parent.children.push(node);
        parent.last_child = Some(node);
        self.current = node;
    }

    pub fn undo(&mut self) -> Option<Step> {
        let node = &self.nodes[self.current];
        let parent = node.parent?;
        let step = Step {
            edits: node.undo.clone(),
            cursor: node.cursor_before,
        };
        self.current = parent;
        Some(step)
    }

    pub fn redo(&mut self) -> Option<Step> {
        let child = self.nodes[self.current].last_child?;
        let node = &self.nodes[child];
        let step = Step {
            edits: node.redo.clone(),
            cursor: node.cursor_after,
        };
        self.current = child;
        Some(step)
    }

    /// How many branches lead out of the current state. More than one means an
    /// undo took you back past a fork — worth surfacing, because the whole
    /// point of a tree is that the other branch is still there.
    pub fn branches(&self) -> usize {
        self.nodes[self.current].children.len()
    }

    /// A monotonic id for the current state, so a buffer can tell whether it
    /// is back at the last save.
    pub fn revision(&self) -> usize {
        self.current
    }
}
