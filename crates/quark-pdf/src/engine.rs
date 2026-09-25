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

    // 3. A vendored copy in the source tree, so a `cargo test` or a freshly
    //    built `target/release/quark.exe` works with no setup.
    let vendor = if cfg!(windows) {
        "vendor/pdfium"
    } else {
        "vendor/pdfium-linux"
    };
    out.push(PathBuf::from(vendor).join(name));

    // Walk up from a starting directory looking for `vendor/`.
    //
    // Done from the executable as well as the working directory. Tests run
    // from inside a crate directory, so the working directory finds it — but a
    // binary in `target/release` is launched from wherever the user happens to
    // be, and walking up from *there* leaves the repository entirely. That is
    // how opening a PDF failed with the vendored library sitting in the source
    // tree three levels above the executable the whole time.
    let mut walk_up = |start: &Path| {
        let mut dir = Some(start);
        while let Some(d) = dir {
            out.push(d.join(vendor).join(name));
            dir = d.parent();
        }
    };
    if let Ok(cwd) = std::env::current_dir() {
        walk_up(&cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            walk_up(dir);
        }
    }

    // The two walks overlap whenever Quark is run from inside its own tree,
    // and a candidate list that repeats itself makes the "tried:" message in
    // the error harder to read than it needs to be.
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));

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
    fn the_vendor_tree_is_searched_from_the_executable_not_just_the_cwd() {
        // The regression this pins: the vendor walk used to start only at the
        // working directory, so `target/release/quark.exe` launched from
        // somewhere else walked up that other tree and never saw the library
        // sitting three levels above itself.
        let _g = crate::testutil::pdfium_guard();
        let paths = search_paths();

        let exe = std::env::current_exe().expect("a test binary has a path");
        let mut ancestor = exe.parent();
        let mut found = false;
        while let Some(dir) = ancestor {
            let candidate = dir.join(if cfg!(windows) {
                "vendor/pdfium"
            } else {
                "vendor/pdfium-linux"
            });
            if paths.iter().any(|p| p.starts_with(&candidate)) {
                found = true;
                break;
            }
            ancestor = dir.parent();
        }
        assert!(
            found,
            "no candidate walks up from the executable at {}",
            exe.display()
        );
    }

    #[test]
    fn the_candidate_list_does_not_repeat_itself() {
        // Running from inside the tree makes the two walks overlap, and the
        // duplicates end up in the user-facing "tried:" error message.
        let _g = crate::testutil::pdfium_guard();
        let paths = search_paths();
        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len(), "duplicates in {paths:?}");
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
