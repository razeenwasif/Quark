//! A project's folder under both of its names.
//!
//! One is the name it was opened by: a path through a symlink, or on macOS
//! anything under `/tmp` and `/var`, which point into `/private`. The other
//! is the real folder behind it. The editor resolves every file it opens, so
//! those buffers are spelled through the real folder, while `:w {file}` and
//! anything resolved against the root are spelled through the name it was
//! opened by. They are files in the same folder, and matching only one
//! spelling opens one file as two buffers.

use std::path::{Path, PathBuf};

/// A path made absolute, its symlinks resolved where it exists.
pub fn normalise(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(real) => plain(real),
        // Does not exist yet, or cannot be read. Absolute is still better than
        // relative, so the error at least names something findable.
        Err(_) => match std::env::current_dir() {
            Ok(cwd) if path.is_relative() => cwd.join(path),
            _ => path.to_path_buf(),
        },
    }
}

/// Windows' canonical paths begin `\\?\`, which nothing else spells; gone.
pub(crate) fn plain(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

#[derive(Debug, Clone, Default)]
pub struct Root {
    path: PathBuf,
    /// `path` with its symlinks resolved, worked out once.
    real: PathBuf,
}

impl Root {
    pub fn new(path: impl AsRef<Path>) -> Root {
        let path = path.as_ref().to_path_buf();
        Root { real: normalise(&path), path }
    }

    /// The folder, as it was opened.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where a file is in this folder, under either name, or `None` if it is
    /// not in it.
    pub fn inside<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.path).or_else(|_| path.strip_prefix(&self.real)).ok()
    }

    /// Whether two paths are one file: spelled alike, or the same place in
    /// this folder reached through its two names.
    pub fn same(&self, a: &Path, b: &Path) -> bool {
        a == b || matches!((self.inside(a), self.inside(b)), (Some(here), Some(there)) if here == there)
    }

    /// A path as a row shows it: relative to this folder when it is in it,
    /// whole when it is not, with `/` between the parts on every platform.
    pub fn shown(&self, path: &Path) -> String {
        self.inside(path).unwrap_or(path).to_string_lossy().replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_one_file_under_either_name_of_its_folder() {
        let dir = std::env::temp_dir().join(format!("quark-modal-root-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = Root::new(&dir);
        let real = normalise(&dir);
        assert!(root.same(&dir.join("main.tex"), &real.join("main.tex")));
        assert!(!root.same(&dir.join("a.tex"), &real.join("b.tex")));
        assert_eq!(root.shown(&real.join("chapters/one.tex")), "chapters/one.tex");
        assert_eq!(root.shown(Path::new("/elsewhere/x.tex")), "/elsewhere/x.tex");
        let _ = std::fs::remove_dir_all(dir);
    }
}
