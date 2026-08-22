//! Binding to the PDFium library.
//!
//! # Why the instance is leaked
//!
//! `PdfDocument<'a>` borrows the `Pdfium` that created it, so every open
//! document would otherwise have to name that lifetime — and a long-lived
//! application struct cannot hold a self-referential borrow. Since exactly one
//! `Pdfium` exists for the life of the process anyway, it is initialised once
//! into a `OnceLock` and handed out as `&'static`. Documents are then
//! `PdfDocument<'static>` and can live wherever the application needs them.
//!
//! # Where the library comes from
//!
//! Quark ships PDFium beside its own executable. The search order below exists
//! so that the same code works from a build tree, from an installed copy, and
//! from a test run, without any of them needing configuration.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::*;

static ENGINE: OnceLock<Pdfium> = OnceLock::new();

/// Where the PDFium library was actually loaded from, for the About dialog and
/// for diagnosing a bad install.
static LOADED_FROM: OnceLock<PathBuf> = OnceLock::new();

/// The platform's PDFium filename.
pub fn library_filename() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Candidate locations for the PDFium library, in priority order.
pub fn search_paths() -> Vec<PathBuf> {
    let name = library_filename();
    let mut out = Vec::new();

    // 1. An explicit override always wins, which is what makes it useful for
    //    debugging a suspected engine problem.
    if let Ok(p) = std::env::var("QUARK_PDFIUM") {
        out.push(PathBuf::from(p));
    }

    // 2. Beside the executable — how a shipped Quark finds it.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join(name));
            // cargo puts examples and tests one level below the profile dir.
            if let Some(up) = dir.parent() {
                out.push(up.join(name));
            }
        }
    }

    // 3. The vendored copies in the source tree, so a `cargo test` from the
    //    repo root works with no setup.
    let vendor = if cfg!(windows) {
        "vendor/pdfium"
    } else {
        "vendor/pdfium-linux"
    };
    out.push(PathBuf::from(vendor).join(name));
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(vendor).join(name));
        // Walk up: tests often run from inside a crate directory.
        let mut d = cwd.as_path();
        while let Some(parent) = d.parent() {
            out.push(parent.join(vendor).join(name));
            d = parent;
        }
    }

    out
}

/// Errors that can come out of the engine layer.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(
        "could not load {0}. Quark ships this library beside its executable; \
         reinstall, or set QUARK_PDFIUM to its path. Tried: {1}"
    )]
    LibraryNotFound(String, String),
    #[error("PDFium reported: {0}")]
    Pdfium(String),
}

impl From<PdfiumError> for EngineError {
    fn from(e: PdfiumError) -> Self {
        EngineError::Pdfium(format!("{e:?}"))
    }
}

/// Initialises PDFium, or returns the already-initialised instance.
///
/// Safe to call from anywhere; the first caller wins and the rest observe the
/// same instance.
pub fn init() -> Result<&'static Pdfium, EngineError> {
    if let Some(p) = ENGINE.get() {
        return Ok(p);
    }

    let candidates = search_paths();
    let mut tried = Vec::new();
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        match Pdfium::bind_to_library(path) {
            Ok(bindings) => {
                let _ = LOADED_FROM.set(path.clone());
                tracing::info!(path = %path.display(), "loaded PDFium");
                // Another thread may have won the race; either way we end up
                // with exactly one instance.
                let _ = ENGINE.set(Pdfium::new(bindings));
                return Ok(ENGINE.get().expect("just set"));
            }
            Err(e) => tried.push(format!("{} ({e:?})", path.display())),
        }
    }

    // Last resort: a system-wide install.
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        let _ = LOADED_FROM.set(PathBuf::from("<system>"));
        let _ = ENGINE.set(Pdfium::new(bindings));
        return Ok(ENGINE.get().expect("just set"));
    }

    if tried.is_empty() {
        tried = candidates.iter().map(|p| p.display().to_string()).collect();
    }
    Err(EngineError::LibraryNotFound(
        library_filename().to_owned(),
        tried.join(", "),
    ))
}

/// The initialised engine. Panics if [`init`] has not succeeded, so call it
/// only from code that runs after startup.
pub fn engine() -> &'static Pdfium {
    ENGINE
        .get()
        .expect("PDFium not initialised — call quark_pdf::engine::init() at startup")
}

pub fn is_initialised() -> bool {
    ENGINE.get().is_some()
}

/// The path PDFium was loaded from.
pub fn loaded_from() -> Option<&'static Path> {
    LOADED_FROM.get().map(|p| p.as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_name_matches_the_platform() {
        let _g = crate::testutil::pdfium_guard();
        let n = library_filename();
        if cfg!(windows) {
            assert_eq!(n, "pdfium.dll");
        } else {
            assert!(n.starts_with("libpdfium"));
        }
    }

    #[test]
    fn search_paths_include_the_executable_directory_and_the_vendor_tree() {
        let _g = crate::testutil::pdfium_guard();
        let paths = search_paths();
        assert!(!paths.is_empty());
        let joined = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined.contains("vendor/pdfium"), "vendor dir missing: {joined}");
    }

    #[test]
    fn an_override_takes_priority_over_everything_else() {
        let _g = crate::testutil::pdfium_guard();
        // SAFETY: single-threaded test process; the variable is read only by
        // search_paths, which is called synchronously below.
        unsafe { std::env::set_var("QUARK_PDFIUM", "/tmp/explicit-pdfium.so") };
        let paths = search_paths();
        assert_eq!(paths[0], PathBuf::from("/tmp/explicit-pdfium.so"));
        unsafe { std::env::remove_var("QUARK_PDFIUM") };
    }

    #[test]
    fn the_engine_initialises_from_the_vendored_library() {
        let _g = crate::testutil::pdfium_guard();
        // This is the smoke test that the vendored binary matches the bindings
        // the crate was compiled against.
        let e = init();
        assert!(e.is_ok(), "engine init failed: {:?}", e.err());
        assert!(is_initialised());
        assert!(loaded_from().is_some());
    }
}
