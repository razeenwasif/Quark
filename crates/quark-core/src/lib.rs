//! Domain types and pure logic for Quark.
//!
//! Nothing in this crate touches PDFium, Win32 or egui, so all of it builds and
//! tests natively on Linux. That is deliberate: the viewer's geometry, the
//! annotation model and the undo stack are where the subtle bugs live, and
//! testing them should not require a window or a document.

pub mod annot;
pub mod command;
pub mod geom;
pub mod history;
pub mod layout;
pub mod prefs;
pub mod search;
pub mod tools;

pub use geom::{PageSize, PageTransform, Rect, Rot, Vec2};
pub use layout::{Layout, LayoutParams, PageMode, ZoomMode};
