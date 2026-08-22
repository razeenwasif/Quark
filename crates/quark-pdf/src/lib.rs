//! Quark's PDF engine.
//!
//! Two libraries do the work, split along a clean line:
//!
//! * **PDFium** renders pages, extracts text, and reads and writes annotations
//!   and form fields. It is the same engine Chrome uses, which is why a page
//!   looks right rather than approximately right.
//! * **lopdf** performs structural surgery on the file's object graph — page
//!   insertion and removal, metadata, encryption, watermarks. PDFium's writing
//!   API does not reach these, and lopdf has no renderer, so neither one alone
//!   is enough.
//!
//! Everything in this crate is synchronous and blocking. [`service`] wraps it
//! in a worker thread so the UI never waits on a render.

pub mod annots;
pub mod doc;
pub mod engine;
pub mod export;
pub mod forms;
pub mod organize;
pub mod outline;
pub mod render;
pub mod security;
pub mod service;
pub mod text;

#[cfg(test)]
pub mod testutil;

pub use doc::{DocInfo, Document, Metadata, OpenError, PageInfo, Permissions};
pub use engine::EngineError;
