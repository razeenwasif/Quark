//! The library: the books in the folders the person keeps them in —
//! `~/Documents/Books` until they name others — shelved by the folder each
//! is in, each with its title and author as the book says them, how far it
//! was read, and its cover.
//!
//! Walked and read on a worker, and kept between sessions, so the library is
//! on screen at once while it is walked again. A book whose file is unchanged
//! is not read again. A book whose file is in iCloud is listed and never
//! read, so nothing is brought down until the person opens it. Covers are
//! drawn by the service ([`Request::Cover`](crate::service::Request::Cover))
//! and kept under a name for the file as it is, so a book changed is drawn
//! again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The folder a library starts with.
pub const DEFAULT_FOLDER: &str = "~/Documents/Books";
/// The most books one library lists.
const MOST: usize = 5_000;
/// The most files and folders its walk looks at.
const WALKED: usize = 50_000;
/// Folders that hold other people's code, not books.
const PASSED: [&str; 6] = ["node_modules", "target", "build", "dist", "__pycache__", "venv"];
/// How long where a book was left waits before it is written down.
const KEEP_AFTER: Duration = Duration::from_secs(5);

/// A book, as the library shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    pub path: PathBuf,
    /// Its file's name.
    pub file: String,
    pub title: String,
    pub author: String,
    pub pages: u32,
    pub size: u64,
    /// The folder it is in under the library's — its shelf; empty for the
    /// library folder's own.
    pub shelf: String,
    /// Which of the library's folders it is in, as typed: `~/Documents/Books`.
    pub folder: String,
    /// Its file is in iCloud: opening it brings it down.
    pub in_cloud: bool,
    /// The page it was left at, from 0.
    pub page: Option<u32>,
    /// When it was last read, in seconds since the epoch.
    pub read: Option<u64>,
    /// Its title or author is the person's, not the book's own.
    pub renamed: bool,
}

impl Book {
    /// The name its shelf goes by: its folder under the library's, or the
    /// library folder's own name.
    pub fn shelf_name(&self) -> String {
        if !self.shelf.is_empty() {
            return self.shelf.clone();
        }
        self.folder.trim_end_matches('/').rsplit('/').next().unwrap_or(&self.folder).to_string()
    }

    /// Whether every word typed is in its title, author, file or shelf.
    pub fn named_by(&self, query: &str) -> bool {
        let text = format!("{} {} {} {}", self.title, self.author, self.file, self.shelf).to_lowercase();
        query.to_lowercase().split_whitespace().all(|w| text.contains(w))
    }

    /// How far through it was read, 0 to 1, once it was left somewhere.
    pub fn progress(&self) -> Option<f32> {
        let page = self.page?;
        (self.pages > 0).then(|| ((page + 1) as f32 / self.pages as f32).min(1.0))
    }
}

/// A book as walked and read: kept between sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Shelved {
    path: PathBuf,
    /// The root it was found under, resolved.
    root: PathBuf,
    shelf: String,
    size: u64,
    mtime: u64,
    in_cloud: bool,
    title: String,
    author: String,
    pages: u32,
}

/// What the person calls a book, in place of what it calls itself: kept apart
/// from what the walk reads, so reading it again changes neither. An empty
/// one is the book's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Named {
    title: String,
    author: String,
}

/// `library.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    books: Vec<Shelved>,
    #[serde(default)]
    names: Vec<(PathBuf, Named)>,
    /// Where each book was left, and when.
    #[serde(default)]
    read: Vec<(PathBuf, u32, u64)>,
}

/// The library.
#[derive(Default)]
pub struct Library {
    /// The folders as typed — `~/Documents/Books`.
    typed: Vec<String>,
    /// As last walked.
    shelved: Vec<Shelved>,
    walking: Option<Receiver<Vec<Shelved>>>,
    /// Walked since the session began.
    walked: bool,
    /// `library.json`, and the covers' folder; none in memory.
    store: Option<PathBuf>,
    covers_dir: Option<PathBuf>,
    names: HashMap<PathBuf, Named>,
    read: HashMap<PathBuf, (u32, u64)>,
    /// Where a book was left changed, not yet written down: since when.
    unkept: Option<Instant>,
    /// The last file and page told to [`left_at`](Self::left_at), so a frame
    /// that changes neither costs nothing.
    last_told: Option<(PathBuf, usize)>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("typed", &self.typed)
            .field("books", &self.shelved.len())
            .field("walking", &self.walking.is_some())
            .finish()
    }
}

impl Library {
    /// Kept in `dir`: the books as last walked, what the person calls them,
    /// where each was left, and the covers.
    pub fn kept_in(dir: &Path) -> Library {
        let store = dir.join("library.json");
        let kept: Kept = std::fs::read_to_string(&store)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Library {
            shelved: kept.books,
            names: kept.names.into_iter().collect(),
            read: kept.read.into_iter().map(|(p, page, when)| (p, (page, when))).collect(),
            store: Some(store),
            covers_dir: Some(dir.join("covers")),
            ..Library::default()
        }
    }

    /// The folders, as typed. Kept books from folders no longer named go.
    pub fn set_folders(&mut self, typed: Vec<String>) {
        let roots: Vec<PathBuf> = typed.iter().map(|t| resolved(t)).collect();
        self.shelved.retain(|s| roots.contains(&s.root));
        if typed != self.typed {
            self.typed = typed;
            self.keep();
        }
    }

    pub fn folders(&self) -> &[String] {
        &self.typed
    }

    /// The folders named that are not there.
    pub fn missing(&self) -> Vec<String> {
        self.typed.iter().filter(|t| !expand(t).is_dir()).cloned().collect()
    }

    /// Whether the session has walked it yet.
    pub fn walked(&self) -> bool {
        self.walked
    }

    pub fn walking(&self) -> bool {
        self.walking.is_some()
    }

    /// Walk the folders again, on a worker: what is kept of a book whose
    /// file is unchanged is not read again.
    pub fn walk(&mut self) {
        self.walked = true;
        let roots: Vec<PathBuf> = self.typed.iter().map(|t| resolved(t)).collect();
        let known: HashMap<PathBuf, Shelved> = self.shelved.iter().map(|s| (s.path.clone(), s.clone())).collect();
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("quark-library".into()).spawn(move || {
            let _ = send.send(shelve(&roots, &known));
        });
        if spawned.is_ok() {
            self.walking = Some(receive);
        }
    }

    /// What has come of the walk: whether the library changed. Where a book
    /// was left is written down here too, a few seconds after it changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(walking) = &self.walking {
            match walking.try_recv() {
                Ok(shelved) => {
                    self.shelved = shelved;
                    self.walking = None;
                    self.last_told = None;
                    self.keep();
                    changed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.walking = None;
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.unkept.is_some_and(|since| since.elapsed() >= KEEP_AFTER) {
            self.keep();
        }
        changed
    }

    /// The books, each where it was left, in shelf then title order.
    pub fn books(&self) -> Vec<Book> {
        self.shelved
            .iter()
            .map(|s| {
                let left = self.read.get(&s.path);
                let named = self.names.get(&s.path);
                let title = named.map(|n| n.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| s.title.clone());
                let author = named.map(|n| n.author.clone()).filter(|a| !a.is_empty()).unwrap_or_else(|| s.author.clone());
                let folder = self
                    .typed
                    .iter()
                    .find(|t| resolved(t) == s.root)
                    .cloned()
                    .unwrap_or_else(|| tilde(&s.root));
                Book {
                    path: s.path.clone(),
                    file: s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    title,
                    author,
                    pages: s.pages,
                    size: s.size,
                    shelf: s.shelf.clone(),
                    folder,
                    in_cloud: s.in_cloud,
                    page: left.map(|l| l.0),
                    read: left.map(|l| l.1),
                    renamed: named.is_some(),
                }
            })
            .collect()
    }

    /// Whether `path` is a book of the library.
    pub fn has(&self, path: &Path) -> bool {
        let path = normalised(path);
        self.shelved.iter().any(|s| s.path == path)
    }

    /// A book left at `page`: kept, if it is one of the library's, and if it
    /// moved. Cheap enough to call every frame.
    pub fn left_at(&mut self, path: &Path, page: usize) {
        if self.last_told.as_ref().is_some_and(|(p, n)| p == path && *n == page) {
            return;
        }
        self.last_told = Some((path.to_path_buf(), page));
        let page = page as u32;
        let path = normalised(path);
        if self.read.get(&path).is_some_and(|l| l.0 == page) || !self.shelved.iter().any(|s| s.path == path) {
            return;
        }
        self.read.insert(path, (page, now()));
        self.unkept.get_or_insert_with(Instant::now);
    }

    /// The page a book was left at.
    pub fn page_left(&self, path: &Path) -> Option<usize> {
        self.read.get(&normalised(path)).map(|l| l.0 as usize)
    }

    /// Call a book of the library by another title and author, in the library
    /// only: its file and what it says of itself are left as they are. Both
    /// empty, it is called by its own again.
    pub fn rename(&mut self, path: &Path, title: &str, author: &str) -> Result<(), String> {
        let path = normalised(path);
        let book = self
            .shelved
            .iter()
            .find(|s| s.path == path)
            .ok_or_else(|| format!("{} is not a book of the library", path.display()))?;
        let (title, author) = (title.trim(), author.trim());
        if title.chars().count() > 300 || author.chars().count() > 300 {
            return Err("A title or an author is three hundred letters at most.".into());
        }
        // What it calls itself already is no renaming.
        let title = if title == book.title { "" } else { title };
        let author = if author == book.author { "" } else { author };
        if title.is_empty() && author.is_empty() {
            self.names.remove(&path);
        } else {
            self.names.insert(path, Named { title: title.to_string(), author: author.to_string() });
        }
        self.keep();
        Ok(())
    }

    /// Where a book's cover is kept, for its file as it is; none for a file
    /// that is not a book, or one in iCloud, which is not to be drawn.
    pub fn cover_file(&self, path: &Path) -> Option<PathBuf> {
        let book = self.shelved.iter().find(|s| s.path == path)?;
        if book.in_cloud {
            return None;
        }
        Some(cover_file(self.covers_dir.as_ref()?, &book.path, book.size, book.mtime))
    }

    /// Everything written down now: on quitting.
    pub fn flush(&mut self) {
        if self.unkept.is_some() {
            self.keep();
        }
    }

    /// Written down, for the next session.
    fn keep(&mut self) {
        self.unkept = None;
        let Some(file) = &self.store else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut names: Vec<(PathBuf, Named)> = self.names.iter().map(|(p, n)| (p.clone(), n.clone())).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let mut read: Vec<(PathBuf, u32, u64)> = self.read.iter().map(|(p, l)| (p.clone(), l.0, l.1)).collect();
        read.sort_by(|a, b| a.0.cmp(&b.0));
        let kept = Kept { books: self.shelved.clone(), names, read };
        if let Ok(text) = serde_json::to_string(&kept) {
            let _ = write_atomically(file, text.as_bytes());
        }
    }
}

/// A file written whole or not at all: beside it, then moved over it.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut beside = path.as_os_str().to_owned();
    beside.push(format!(".{}.part", std::process::id()));
    let beside = PathBuf::from(beside);
    std::fs::write(&beside, bytes)?;
    std::fs::rename(&beside, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&beside);
    })
}

/// A cover's file, named for its book's file as it is: a book changed is
/// drawn again.
fn cover_file(dir: &Path, path: &Path, size: u64, mtime: u64) -> PathBuf {
    // FNV-1a: stable from one build to the next, as std's hasher is not
    // promised to be.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().bytes().chain(size.to_le_bytes()).chain(mtime.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    dir.join(format!("{hash:016x}.png"))
}

/// `~` and all, as the person typed it, resolved.
fn resolved(typed: &str) -> PathBuf {
    normalised(&expand(typed))
}

/// A path as an opened file's is: the same book on a shelf and open in a tab
/// are one.
fn normalised(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `~/x` as the home folder's `x`.
pub fn expand(typed: &str) -> PathBuf {
    let typed = typed.trim();
    match (typed.strip_prefix("~"), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(typed),
    }
}

/// A path as the person would write it, the home folder as `~`.
pub fn tilde(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
        }
    }
    path.display().to_string()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether a file or folder is in iCloud only, its contents not on this Mac:
/// reading it would bring it down.
fn in_cloud(meta: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        /// `SF_DATALESS`, from `<sys/stat.h>`.
        const DATALESS: u32 = 0x4000_0000;
        meta.st_flags() & DATALESS != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = meta;
        false
    }
}

/// Every PDF in the folders, shelved by the folder it is in, each read for
/// its title, author and pages unless `known` has it as its file still is.
fn shelve(roots: &[PathBuf], known: &HashMap<PathBuf, Shelved>) -> Vec<Shelved> {
    let mut out = Vec::new();
    let mut walked = 0;
    'roots: for root in roots {
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                walked += 1;
                if walked > WALKED || out.len() >= MOST {
                    break 'roots;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
                if meta.file_type().is_symlink() {
                    continue;
                }
                if meta.is_dir() {
                    // A folder whose listing is in iCloud is not opened: that
                    // would bring it down.
                    if !PASSED.contains(&name.as_str()) && !in_cloud(&meta) {
                        pending.push(entry.path());
                    }
                    continue;
                }
                let path = entry.path();
                if !meta.is_file() || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                    continue;
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                let cloud = in_cloud(&meta);
                let shelf = path
                    .parent()
                    .and_then(|p| p.strip_prefix(root).ok())
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                let found = Shelved {
                    path: path.clone(),
                    root: root.clone(),
                    shelf,
                    size: meta.len(),
                    mtime,
                    in_cloud: cloud,
                    title: String::new(),
                    author: String::new(),
                    pages: 0,
                };
                let read = match known.get(&path) {
                    Some(k) if (k.size, k.mtime, k.in_cloud) == (found.size, found.mtime, found.in_cloud) && !k.title.is_empty() => {
                        Shelved { title: k.title.clone(), author: k.author.clone(), pages: k.pages, ..found }
                    }
                    // Its file is in iCloud: what its name says, and no more.
                    _ if cloud => {
                        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                        let (title, by) = quark_edit::library::from_name(&stem);
                        Shelved { title, author: by.unwrap_or_default(), ..found }
                    }
                    _ => {
                        let about = quark_edit::library::about(&path);
                        Shelved { title: about.title, author: about.author, pages: about.pages, ..found }
                    }
                };
                out.push(read);
            }
        }
    }
    out.sort_by(|a, b| {
        (a.shelf.to_lowercase(), a.title.to_lowercase()).cmp(&(b.shelf.to_lowercase(), b.title.to_lowercase()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library folder of the test's own: two books at its top, one on a
    /// shelf, one on a shelf inside that, and what is not a book.
    fn books(name: &str) -> PathBuf {
        let root = crate::testutil::tmp_path(name);
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["", "ML", "ML/Deep", "node_modules", ".hidden"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let pdf = |title: &str| {
            let mut doc = lopdf::Document::load_mem(&quark_edit::fixture::fixture(&["One", "Two", "Three"])).unwrap();
            let info = doc.add_object(lopdf::dictionary! { "Title" => lopdf::Object::string_literal(title) });
            doc.trailer.set("Info", info);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            bytes
        };
        std::fs::write(root.join("zebra-stripes.pdf"), pdf("")).unwrap();
        std::fs::write(root.join("algebra.pdf"), pdf("Linear Algebra Done Right")).unwrap();
        std::fs::write(root.join("ML/mml.pdf"), pdf("Mathematics for Machine Learning")).unwrap();
        std::fs::write(root.join("ML/Deep/dl.pdf"), pdf("Deep Learning")).unwrap();
        std::fs::write(root.join("node_modules/not-a-book.pdf"), pdf("Package docs")).unwrap();
        std::fs::write(root.join(".hidden/secret.pdf"), pdf("Hidden")).unwrap();
        std::fs::write(root.join("notes.txt"), "not a book").unwrap();
        root
    }

    fn walked(lib: &mut Library) {
        lib.walk();
        let started = Instant::now();
        while lib.walking() {
            assert!(started.elapsed() < Duration::from_secs(20), "the walk never ended");
            lib.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn books_are_shelved_by_their_folder_with_their_own_titles_and_pages() {
        let root = books("lib-shelves");
        let mut lib = Library::default();
        lib.set_folders(vec![root.display().to_string()]);
        walked(&mut lib);
        let books = lib.books();
        let seen: Vec<(&str, &str, u32)> = books.iter().map(|b| (b.shelf.as_str(), b.title.as_str(), b.pages)).collect();
        assert_eq!(
            seen,
            [
                ("", "Linear Algebra Done Right", 3),
                // No title of its own: its file's name, as words.
                ("", "Zebra Stripes", 3),
                ("ML", "Mathematics for Machine Learning", 3),
                ("ML/Deep", "Deep Learning", 3),
            ],
            "code's folders, hidden ones and what is not a PDF are passed over"
        );
        let top = books.iter().find(|b| b.shelf.is_empty()).unwrap();
        assert_eq!(top.shelf_name(), root.file_name().unwrap().to_string_lossy());
        assert!(books.iter().all(|b| !b.in_cloud && b.page.is_none() && !b.renamed));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_library_is_kept_with_its_names_and_where_each_book_was_left() {
        let root = books("lib-kept");
        let store = crate::testutil::tmp_path("lib-kept-store");
        let _ = std::fs::remove_dir_all(&store);
        let mut lib = Library::kept_in(&store);
        lib.set_folders(vec![root.display().to_string()]);
        walked(&mut lib);
        let mml = lib.books().into_iter().find(|b| b.shelf == "ML").unwrap();

        lib.rename(&mml.path, "MML", "Deisenroth").unwrap();
        lib.left_at(&mml.path, 41);
        assert!(lib.page_left(&mml.path) == Some(41));
        // Not a book of the library: not kept.
        lib.left_at(&root.join("notes.txt"), 3);
        lib.flush();

        let again = Library::kept_in(&store);
        let book = again.books().into_iter().find(|b| b.path == mml.path).unwrap();
        assert_eq!((book.title.as_str(), book.author.as_str(), book.renamed), ("MML", "Deisenroth", true));
        assert_eq!(book.page, Some(41));
        assert!(book.read.is_some());
        assert_eq!(again.page_left(&root.join("notes.txt")), None);
        // On screen at once, before it is walked again.
        assert_eq!(again.books().len(), 4);

        // Its own again: both emptied, or said as the book says them.
        let mut again = again;
        again.rename(&mml.path, "", "").unwrap();
        let book = again.books().into_iter().find(|b| b.path == mml.path).unwrap();
        assert_eq!((book.title.as_str(), book.renamed), ("Mathematics for Machine Learning", false));
        again.rename(&mml.path, "Mathematics for Machine Learning", "").unwrap();
        assert!(!again.books().into_iter().find(|b| b.path == mml.path).unwrap().renamed);
        assert!(again.rename(&root.join("notes.txt"), "x", "").is_err(), "only a book is renamed");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(store);
    }

    #[test]
    fn a_book_unchanged_is_not_read_again_and_one_gone_leaves_the_shelf() {
        let root = books("lib-again");
        let mut lib = Library::default();
        lib.set_folders(vec![root.display().to_string()]);
        walked(&mut lib);
        // What was read is what is kept: changed by hand, it stays so while
        // the file is as it was.
        for s in &mut lib.shelved {
            s.title = format!("Kept {}", s.title);
        }
        std::fs::remove_file(root.join("ML/Deep/dl.pdf")).unwrap();
        walked(&mut lib);
        let titles: Vec<String> = lib.books().into_iter().map(|b| b.title).collect();
        assert_eq!(titles.len(), 3);
        assert!(titles.iter().all(|t| t.starts_with("Kept ")), "{titles:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_no_longer_named_takes_its_books_and_a_missing_one_is_said() {
        let root = books("lib-folders");
        let mut lib = Library::default();
        lib.set_folders(vec![root.display().to_string()]);
        walked(&mut lib);
        assert_eq!(lib.books().len(), 4);
        let gone = root.join("not-there").display().to_string();
        lib.set_folders(vec![gone.clone()]);
        assert!(lib.books().is_empty());
        assert_eq!(lib.missing(), [gone]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_cover_is_named_for_its_file_as_it_is() {
        let dir = Path::new("/covers");
        let a = cover_file(dir, Path::new("/b/x.pdf"), 10, 20);
        assert_eq!(a, cover_file(dir, Path::new("/b/x.pdf"), 10, 20), "the same file, the same cover");
        assert_ne!(a, cover_file(dir, Path::new("/b/x.pdf"), 11, 20), "a book changed is drawn again");
        assert_ne!(a, cover_file(dir, Path::new("/b/x.pdf"), 10, 21));
        assert_ne!(a, cover_file(dir, Path::new("/b/y.pdf"), 10, 20));
        assert!(a.starts_with(dir) && a.extension().is_some_and(|e| e == "png"));
    }

    #[test]
    fn words_typed_narrow_to_the_books_they_name() {
        let b = Book {
            path: "/x/mml.pdf".into(),
            file: "mml.pdf".into(),
            title: "Mathematics for Machine Learning".into(),
            author: "Deisenroth".into(),
            pages: 400,
            size: 1,
            shelf: "ML".into(),
            folder: "~/Documents/Books".into(),
            in_cloud: false,
            page: Some(99),
            read: Some(1),
            renamed: false,
        };
        assert!(b.named_by("machine deis"));
        assert!(b.named_by("ml"));
        assert!(b.named_by(""));
        assert!(!b.named_by("physics"));
        assert_eq!(b.progress(), Some(0.25));
        assert_eq!(Book { page: None, ..b.clone() }.progress(), None);
        assert_eq!(Book { shelf: String::new(), ..b }.shelf_name(), "Books");
    }

    #[test]
    fn the_home_folder_is_written_and_read_as_a_tilde() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand("~/Documents/Books"), home.join("Documents/Books"));
            assert_eq!(expand("~"), home);
            assert_eq!(tilde(&home.join("Documents/Books")), "~/Documents/Books");
        }
        assert_eq!(expand("/srv/books"), PathBuf::from("/srv/books"));
        assert_eq!(expand("~other/x"), PathBuf::from("~other/x"));
    }
}
