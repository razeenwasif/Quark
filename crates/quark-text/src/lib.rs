//! Quark's text core: rope-backed buffers, an undo tree, and the motions and
//! text objects the modal engine resolves its actions against.
//!
//! No UI, no I/O beyond opening and saving a file, no knowledge of what a mode
//! is. See `docs/ARCHITECTURE.md` §3.

pub mod buffer;
pub mod edit;
pub mod history;
pub mod motion;
pub mod search;

pub use buffer::{Buffer, Change, Point};
pub use edit::{Cursor, Edit, Position, Range};
pub use history::History;
pub use motion::Class;
pub use search::find_in;
