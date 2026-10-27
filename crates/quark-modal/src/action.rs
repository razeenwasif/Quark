//! Resolved actions — the boundary the whole architecture rests on.
//!
//! The modal engine turns keys into an [`Action`], and a mode turns an
//! [`Action`] into an edit. `2dd` resolves to `Delete · Line · 2` here, and
//! Codex splices a rope with it while Video ripple-deletes two clips. Neither
//! mode ever sees a keystroke.

use serde::{Deserialize, Serialize};

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verb {
    Move,
    Delete,
    Yank,
    Change,
    /// Enter insert mode. The object says where the cursor lands first.
    Insert,
    Paste,
    Undo,
    Redo,
    /// Follow a reference to where it is defined. Not an edit, but it is a
    /// verb the modal engine resolves like any other, which is what lets `gd`
    /// mean the same shape of thing in a mode that has no text in it.
    Goto,
    /// Ask what the thing under the cursor *is* — vim's `K`. Not a motion and
    /// not an edit: the answer appears beside the cursor and the buffer is
    /// untouched. A verb rather than a bare command so that a mode with no
    /// text in it can answer it about a layer or a cell.
    Explain,
    /// Repeat the last change — vim's `.`.
    Repeat,
    /// Name the cursor's position, for `m`.
    SetMark,
    /// `>` — shift lines right by one indent.
    Indent,
    /// `<` — shift lines left by one indent.
    Outdent,
    /// `gu`, and `u` on a selection.
    Lowercase,
    /// `gU`, and `U` on a selection.
    Uppercase,
    /// `g~` and `~`.
    ToggleCase,
    /// `J` — the lines into one, a space between.
    Join,
    /// `r` — overwrite characters with the one in [`Object::Char`].
    Replace,
}

/// What to do it to. A mode maps these onto its own nouns: `Line` is a line of
/// text in Codex, a layer in Design, a clip in Video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Object {
    Line,
    LineDown,
    LineUp,
    CharLeft,
    CharRight,
    WordForward,
    WordBackward,
    WordEnd,
    /// `W`, `B`, `E` — every non-blank counts as one class.
    BigWordForward,
    BigWordBackward,
    BigWordEnd,
    LineStart,
    /// `^` — the first non-blank, which is also where `I` inserts.
    FirstNonBlank,
    LineEnd,
    FileStart,
    FileEnd,
    /// The current selection, in visual mode.
    Selection,
    /// Where the symbol under the cursor is defined.
    Definition,
    /// Everywhere the symbol under the cursor is used.
    References,

    /* ── marks ────────────────────────────────────────────────────
    A mark is a place, so it is an object like any other — which is what
    makes `d`a` and `y'a` fall out without a line of extra code. */
    /// The mark to set, for `m`.
    Mark {
        name: char,
    },
    /// Exactly where a mark is — ``a`.
    MarkExact {
        name: char,
    },
    /// The first non-blank of a mark's line — `'a`. Linewise, so `d'a`
    /// takes whole lines.
    MarkLine {
        name: char,
    },
    /// No object — the verb stands alone (`u`, `.`).
    None,
    /// Cursor stays put; used by `i` and `a` to say which side.
    Here,
    After,
    /// Open a new line and insert on it.
    LineBelow,
    LineAbove,

    /* ── visual block ─────────────────────────────────────────────
    `I` and `A` in a block insert on every line of it at once, which is
    the reason the mode exists at all. */
    /// The block's left edge.
    BlockStart,
    /// The block's right edge, padding short lines out to reach it.
    BlockEnd,

    /* ── motions that carry their argument ───────────────────────── */
    /// `f`, `t`, `F` and `T`: the `count`th `ch` on the line, ahead or behind,
    /// or the character just short of it.
    FindChar {
        ch: char,
        forward: bool,
        till: bool,
    },
    /// `%` — the bracket that matches the one under or after the cursor.
    MatchPair,
    /// `}` and `{` — the next and previous blank line.
    ParagraphForward,
    ParagraphBackward,
    /// `5G` and `5gg` — a line by number, counting from one as vim does.
    LineNumber {
        line: u32,
    },
    /// What `r` writes.
    Char {
        ch: char,
    },

    /* ── text objects ─────────────────────────────────────────────
    These are the reason the engine resolves to objects rather than
    to offsets: `ciw` means the same thing in every mode that has a
    notion of "the thing under the cursor". */
    InnerWord,
    AroundWord,
    InnerBigWord,
    AroundBigWord,
    /// `i(`, `i{`, `i"` … carried by the *opening* delimiter.
    InnerPair {
        open: char,
    },
    AroundPair {
        open: char,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub verb: Verb,
    pub object: Object,
    /// Always at least 1. `2d3w` multiplies to 6.
    pub count: u32,
    /// The register a yank or delete writes to, from `"a`.
    pub register: Option<String>,
}

impl Verb {
    /// Whether this verb changes the buffer. `.` repeats changes, not motions.
    pub fn modifies(self) -> bool {
        matches!(
            self,
            Verb::Delete
                | Verb::Change
                | Verb::Insert
                | Verb::Paste
                | Verb::Repeat
                | Verb::Indent
                | Verb::Outdent
                | Verb::Lowercase
                | Verb::Uppercase
                | Verb::ToggleCase
                | Verb::Join
                | Verb::Replace
        )
    }
}

impl Action {
    pub fn new(verb: Verb, object: Object, count: u32) -> Self {
        Self {
            verb,
            object,
            count: count.max(1),
            register: None,
        }
    }

    /// A mode reports this when it has no meaning for the action, so the
    /// statusline can say so. Silently doing nothing is the failure users
    /// cannot debug.
    pub fn describe(&self) -> String {
        let count = if self.count > 1 {
            format!("{} ", self.count)
        } else {
            String::new()
        };
        format!("{count}{:?} · {:?}", self.verb, self.object)
    }
}
