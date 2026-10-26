//! The LaTeX mode's understanding of TeX, with no processes and no network:
//! what a build's log says, which file of a project is its main one and
//! which engine typesets it, and SyncTeX's map between the two.

pub mod bib;
pub mod blocks;
pub mod log;
pub mod markdown;
pub mod outline;
pub mod project;
pub mod synctex;
pub mod templates;
