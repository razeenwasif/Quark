//! Vim for Quark's editor, copied from Proton's core: the modal engine
//! (`vim`, `key`, `action`), what its actions do to a buffer (`editor`,
//! `block`), several views onto the buffers (`split`), the `:` line (`ex`)
//! and what typing does (`typing`) — and, Quark's own, the `session` that
//! joins them for one editor pane.

pub mod action;
pub mod block;
pub mod editor;
pub mod ex;
pub mod key;
pub mod root;
pub mod split;
pub mod typing;
pub mod vim;
pub mod session;

pub use action::Action;
pub use key::Key;
pub use vim::{Vim, VimMode};
