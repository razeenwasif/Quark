//! The literature: papers asked of arXiv, Semantic Scholar and OpenAlex,
//! names checked with Crossref and the DOI system, and open PDFs kept —
//! each host asked no oftener than it asks, on a worker the core polls.
//!
//! What a paper is, as the sources describe it, is `model`'s; the rest
//! only asks.

pub mod arxiv;
pub mod crossref;
pub mod fetch;
pub mod http;
pub mod keys;
pub mod merge;
pub mod model;
pub mod openalex;
pub mod pdf;
pub mod s2;

pub use fetch::{Context, Download, Fetch, Job, Reply, SourceFailure};
pub use http::{Failure, Http};
pub use keys::{KeyState, Keys};
