//! Quark's documents: what the docs editor edits.
//!
//! A [`Document`] is a list of [`Block`]s — paragraphs, headings, list items,
//! quotes, code — each holding text with a [`Style`] on every character. An
//! [`Editor`] owns one, with the selection and the undo history, and every
//! change made to it goes through the editor. [`markdown`] reads and writes
//! the file, which is plain Markdown.
//!
//! Nothing here draws or does input; `quark-app` does that.

pub mod block;
pub mod doc;
pub mod editor;
pub mod markdown;
pub mod motion;
pub mod survey;

pub use block::{Block, BlockKind, Marks, Style};
pub use doc::{Author, Comment, Document, Note, Pos, Selection, Suggestion};
pub use editor::Editor;
