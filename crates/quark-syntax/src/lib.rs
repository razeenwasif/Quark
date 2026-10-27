//! Syntax highlighting.
//!
//! A grammar table, a scope mapping, and a highlighter that parses the whole
//! document and queries only the part of it you can see. Nothing here draws
//! anything: the output is character columns and a colour name, and what those
//! look like is the theme's business.

pub mod highlight;
pub mod language;
pub mod scope;

pub use highlight::{Span, Syntax};
pub use language::{Language, LANGUAGES};
pub use scope::Scope;
