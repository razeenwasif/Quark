//! The programs Quark runs for its documents: TeX — TinyTeX made with the
//! person's say-so, and builds in a sandbox — and OCR, by tesseract.
//!
//! This crate starts processes and talks to no model. The app decides when;
//! the person decides whether.

pub mod disk;
pub mod github;
pub mod job;
pub mod ocr;
pub mod run;
pub mod sandbox;
pub mod tex;

pub use run::State as RunState;
pub use sandbox::Kind as Sandbox;
pub use tex::{TexEnv, TexPlan, TexState};
