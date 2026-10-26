//! A PDF as the PDF mode changes it: opened from its bytes, changed by
//! operations — each undoable, the log kept across saves — and saved as an
//! update appended to the file, so its first bytes are kept; or written
//! whole, atomically, when a change cannot be appended.
//!
//! Pure: lopdf over bytes, no processes and no network. Everything lopdf is
//! asked is asked inside `catch_unwind`, because a PDF is whatever its maker
//! wrote, and one bad file must cost a tab, not the window.

pub mod annot;
pub mod clean;
pub mod combine;
pub mod doc;
#[cfg(any(test, feature = "fixture"))]
pub mod fixture;
pub mod forms;
pub mod library;
pub mod ocr;
pub mod ops;
pub mod pages;
pub mod save;
pub mod security;
pub mod text;

pub use doc::{Doc, Opened};
pub use ops::Op;

/// Run `f`, a panic in it said as an error.
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(panic) => {
            let said = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(format!("{what}: the PDF could not be read ({said})"))
        }
    }
}
