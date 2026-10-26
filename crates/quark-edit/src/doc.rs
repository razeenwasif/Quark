//! An open PDF: the file as it is on disk, the document its changes have
//! made, and the log of those changes — each with the objects it changed as
//! they were before and after, so undo and redo put them back rather than
//! replay anything, and the log survives a save.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use lopdf::{Dictionary, Document, EncryptionState, LoadOptions, Object, ObjectId};

use crate::ops::Op;
use crate::security::{self, Protection};
use crate::{clean, guarded, save};

/// The largest PDF opened.
pub const LARGEST: usize = 128 * 1024 * 1024;

/// What a change did to the objects: the ones it changed, as they were or
/// as they became — `None` where there was none — and the trailer and last
/// number around them.
#[derive(Debug, Clone)]
struct Patch {
    objects: Vec<(ObjectId, Option<Object>)>,
    max_id: u32,
    trailer: Dictionary,
}

impl Patch {
    /// Put the objects as the patch has them.
    fn onto(&self, doc: &mut Document) {
        for (id, object) in &self.objects {
            match object {
                Some(object) => {
                    doc.objects.insert(*id, object.clone());
                }
                None => {
                    doc.objects.remove(id);
                }
            }
        }
        doc.max_id = self.max_id;
        doc.trailer = self.trailer.clone();
    }

    fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.objects.iter().map(|(id, _)| *id)
    }
}

/// A change made: what it was, and what it did, either way.
#[derive(Debug)]
struct Step {
    op: Op,
    before: Patch,
    after: Patch,
}

/// What a change did: `prev` and `next` compared, object by object.
fn patches(prev: &Document, next: &Document) -> (Patch, Patch) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (id, object) in &next.objects {
        let was = prev.objects.get(id);
        if was != Some(object) {
            before.push((*id, was.cloned()));
            after.push((*id, Some(object.clone())));
        }
    }
    for (id, object) in &prev.objects {
        if !next.objects.contains_key(id) {
            before.push((*id, Some(object.clone())));
            after.push((*id, None));
        }
    }
    (
        Patch {
            objects: before,
            max_id: prev.max_id,
            trailer: prev.trailer.clone(),
        },
        Patch {
            objects: after,
            max_id: next.max_id,
            trailer: next.trailer.clone(),
        },
    )
}

/// An open PDF.
#[derive(Debug)]
pub struct Doc {
    /// The file as it is on disk now — after a save, what was saved — what
    /// the next update is appended to, and what it needs of it.
    disk: Arc<Vec<u8>>,
    base: save::Base,
    /// What the changes kept have made of the file as it was opened.
    current: Document,
    /// Every object that may differ from the file on disk: what an update
    /// writes. Let go of when it is saved.
    differs: BTreeSet<ObjectId>,
    /// Every change since opening; the first `at` are made.
    log: Vec<Step>,
    at: usize,
    /// How far into the log the file on disk is, if it is a point in it.
    saved: Option<usize>,
    /// The file on disk is encrypted: the keys it was read with, and the
    /// password that opened it — kept in memory, never said.
    sealed: Option<(EncryptionState, String)>,
    /// Goes up with each change to which pages there are, their order, or
    /// what their own content draws — made, undone or redone: what the text
    /// read of the pages depends on, and nothing else.
    layout: u64,
}

/// What opening said: the document, or why it opens only to be read, or
/// that it needs its password — `wrong`, when one was given and was not it.
pub enum Opened {
    Editable(Box<Doc>),
    ReadOnly(String),
    Locked { wrong: bool },
}

/// Bytes read as a document, with a password if one is given; the keys it
/// was read with, if it was encrypted — itself decrypted — or `Err(None)`
/// when it needs a password it was not given.
fn read_with(
    bytes: &[u8],
    password: Option<&str>,
) -> Result<(Document, Option<EncryptionState>), Option<String>> {
    let loaded = guarded("opening", || {
        match password {
            Some(p) => Document::load_mem_with_options(bytes, LoadOptions::with_password(p)),
            None => Document::load_mem(bytes),
        }
        .map_err(|e| format!("lopdf: {e}"))
    });
    let mut doc = match loaded {
        Ok(doc) => doc,
        Err(why) if why.contains("password") || why.contains("Password") => return Err(None),
        Err(why) => return Err(Some(why)),
    };
    // Encrypted and not decrypted: its password is needed.
    if doc.is_encrypted() && !doc.was_encrypted() {
        return Err(None);
    }
    let state = doc.encryption_state.clone();
    if state.is_some() {
        security::unprotect(&mut doc);
    }
    Ok((doc, state))
}

/// How the document is to be written: plain, under the keys it was read
/// with, or under new passwords.
enum Seal {
    Plain,
    Kept(EncryptionState),
    New(Protection),
}

impl Doc {
    /// Open a PDF from its bytes — editable, or, where it cannot be changed
    /// safely, said so with why.
    pub fn open(bytes: Arc<Vec<u8>>) -> Opened {
        Doc::open_with(bytes, None)
    }

    /// The same, with the password it is protected by.
    pub fn open_with(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Opened {
        if bytes.len() > LARGEST {
            return Opened::ReadOnly("too large to change here".into());
        }
        let (current, state) = match read_with(&bytes, password) {
            Ok(read) => read,
            Err(None) => {
                return Opened::Locked {
                    wrong: password.is_some(),
                }
            }
            Err(Some(why)) => {
                return Opened::ReadOnly(format!("it could not be read to change it — {why}"))
            }
        };
        let sealed = state.map(|s| (s, password.unwrap_or("").to_string()));
        let base = save::Base::of(&current);
        Opened::Editable(Box::new(Doc {
            current,
            base,
            disk: bytes,
            differs: BTreeSet::new(),
            log: Vec::new(),
            at: 0,
            saved: Some(0),
            sealed,
            layout: 0,
        }))
    }

    /// Make a change: what was undone past is let go of.
    pub fn apply(&mut self, op: Op) -> Result<(), String> {
        let mut next = self.current.clone();
        guarded("changing it", || op.apply(&mut next))?;
        let (before, after) = patches(&self.current, &next);
        self.differs.extend(after.ids());
        self.current = next;
        self.layout += u64::from(op.reshapes());
        self.log.truncate(self.at);
        if self.saved.is_some_and(|s| s > self.at) {
            // The file on disk is a state no longer in the log.
            self.saved = None;
        }
        self.log.push(Step { op, before, after });
        self.at += 1;
        Ok(())
    }

    /// Undo the last change made; what it was.
    pub fn undo(&mut self) -> Result<Option<String>, String> {
        if self.at == 0 {
            return Ok(None);
        }
        let step = &self.log[self.at - 1];
        step.before.onto(&mut self.current);
        self.differs.extend(step.before.ids());
        self.at -= 1;
        self.layout += u64::from(step.op.reshapes());
        Ok(Some(step.op.said()))
    }

    /// Make again the last change undone; what it was.
    pub fn redo(&mut self) -> Result<Option<String>, String> {
        let Some(step) = self.log.get(self.at) else {
            return Ok(None);
        };
        step.after.onto(&mut self.current);
        self.differs.extend(step.after.ids());
        self.at += 1;
        self.layout += u64::from(step.op.reshapes());
        Ok(Some(step.op.said()))
    }

    /// Which layout of its pages the document is in: changed only by a
    /// change to which pages there are, their order, or what they draw.
    pub fn layout(&self) -> u64 {
        self.layout
    }

    /// Whether what the window draws is what saving writes, byte for byte:
    /// nothing to encrypt. Then the view's length is the saved size.
    pub fn view_is_saved(&self) -> bool {
        self.sealed.is_none() && !self.protected()
    }

    /// What undo would undo, and redo redo.
    pub fn undoable(&self) -> Option<String> {
        self.at.checked_sub(1).map(|i| self.log[i].op.said())
    }
    pub fn redoable(&self) -> Option<String> {
        self.log.get(self.at).map(|s| s.op.said())
    }

    /// Changed since it was opened or saved.
    pub fn dirty(&self) -> bool {
        self.saved != Some(self.at)
    }

    /// How it is to be written: the last password change kept, or as the
    /// file on disk is.
    fn seal(&self) -> Seal {
        for op in self.log[..self.at].iter().rev().map(|s| &s.op) {
            match op {
                Op::Protect { protection } => return Seal::New(protection.clone()),
                Op::Unprotect => return Seal::Plain,
                _ => {}
            }
        }
        match &self.sealed {
            Some((state, _)) => Seal::Kept(state.clone()),
            None => Seal::Plain,
        }
    }

    /// Whether it will be written with a password.
    pub fn protected(&self) -> bool {
        !matches!(self.seal(), Seal::Plain)
    }

    /// Whether what is made can be appended to the file on disk: only to a
    /// plain one, and only changes that can be.
    fn incremental(&self) -> bool {
        self.sealed.is_none()
            && matches!(self.seal(), Seal::Plain)
            && self.log[..self.at].iter().all(|s| s.op.incremental())
    }

    /// The document as it is now, as it would be saved: the file on disk
    /// with an update appended; or written whole — compressed, protected —
    /// when that is what the changes ask.
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        if !self.dirty() && self.saved.is_some() {
            return Ok(self.disk.as_ref().clone());
        }
        guarded("writing it", || {
            if self.incremental() {
                return save::update(&self.disk, &self.base, &self.current, &self.differs);
            }
            let compress = self.log[..self.at]
                .iter()
                .any(|s| matches!(s.op, Op::Compress));
            let mut doc = self.current.clone();
            match self.seal() {
                Seal::Plain if compress => clean::compressed(&doc),
                Seal::Plain => save::whole(&doc),
                sealed => {
                    if compress {
                        doc.prune_objects();
                        doc.compress();
                    }
                    match sealed {
                        Seal::Kept(state) => security::reencrypt(&mut doc, &state)?,
                        Seal::New(p) => security::protect(&mut doc, &p)?,
                        Seal::Plain => {}
                    }
                    save::whole(&doc)
                }
            }
        })
    }

    /// The document as it is now, for the window to draw: never encrypted —
    /// it is held in memory, and drawn without asking for a password again.
    pub fn view_bytes(&self) -> Result<Vec<u8>, String> {
        if self.sealed.is_none() && !self.protected() {
            return self.bytes();
        }
        guarded("drawing it", || save::whole(&self.current))
    }

    /// Save to `path`: the changes appended where they can be, the whole
    /// written where they cannot; what is on disk from now on.
    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let bytes = self.bytes()?;
        // What it is read back with: the password it was given, or kept.
        let password = match self.seal() {
            Seal::New(p) => Some(p.user.clone()),
            Seal::Kept(_) => self.sealed.as_ref().map(|(_, p)| p.clone()),
            Seal::Plain => None,
        };
        let (on_disk, state) = read_with(&bytes, password.as_deref()).map_err(|e| {
            e.unwrap_or_else(|| "what was saved could not be read back with its password".into())
        })?;
        save::atomically(path, &bytes)?;
        self.sealed = state.map(|s| (s, password.unwrap_or_default()));
        self.disk = Arc::new(bytes);
        self.base = save::Base::of(&on_disk);
        // What is on disk is what is made: nothing differs from it now.
        self.differs.clear();
        self.saved = Some(self.at);
        Ok(())
    }

    /// What it says of itself, as it is now.
    pub fn info(&self) -> clean::Info {
        guarded("reading its properties", || Ok(clean::info(&self.current))).unwrap_or_default()
    }

    /// How large the file on disk is.
    pub fn size_on_disk(&self) -> usize {
        self.disk.len()
    }

    /// How many pages it has now.
    pub fn pages(&self) -> u32 {
        self.current.get_pages().len() as u32
    }

    /// The document as it is now, to read.
    pub fn document(&self) -> &Document {
        &self.current
    }

    /// Its form's fields, as it is now.
    pub fn fields(&self) -> Vec<crate::forms::Field> {
        guarded("listing its form", || Ok(crate::forms::list(&self.current))).unwrap_or_default()
    }

    /// Its comments, as it is now.
    pub fn comments(&self) -> Vec<crate::annot::Comment> {
        guarded("listing its comments", || {
            Ok(crate::annot::list(&self.current))
        })
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(pages: &[&str]) -> Arc<Vec<u8>> {
        Arc::new(crate::fixture::fixture(pages))
    }

    fn editable(bytes: Arc<Vec<u8>>) -> Doc {
        match Doc::open(bytes) {
            Opened::Editable(doc) => *doc,
            Opened::ReadOnly(why) => panic!("{why}"),
            Opened::Locked { .. } => panic!("locked"),
        }
    }

    fn rotations(bytes: &[u8]) -> Vec<i64> {
        let doc = Document::load_mem(bytes).unwrap();
        doc.get_pages()
            .values()
            .map(|id| {
                crate::ops::inherited(&doc, *id, b"Rotate")
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(0)
            })
            .collect()
    }

    #[test]
    fn a_change_is_appended_to_the_file_whose_first_bytes_are_kept() {
        let bytes = fixture(&["one", "two", "three"]);
        let mut doc = editable(bytes.clone());
        assert!(!doc.dirty());
        doc.apply(Op::Rotate {
            pages: vec![2],
            by: 90,
        })
        .unwrap();
        doc.apply(Op::Rotate {
            pages: vec![2],
            by: 90,
        })
        .unwrap();
        assert!(doc.dirty());
        let made = doc.bytes().unwrap();
        assert!(
            made.starts_with(&bytes),
            "the original bytes are kept, the change appended"
        );
        assert!(
            made.len() < bytes.len() + 1024,
            "only what changed is appended: {} → {}",
            bytes.len(),
            made.len()
        );
        assert_eq!(rotations(&made), [0, 180, 0]);
        assert_eq!(
            Document::load_mem(&made).unwrap().get_pages().len(),
            3,
            "it still reads"
        );
    }

    #[test]
    fn undo_and_redo_walk_the_log_and_a_new_change_lets_go_of_what_was_undone() {
        let mut doc = editable(fixture(&["one", "two", "three"]));
        doc.apply(Op::Rotate {
            pages: vec![1],
            by: 90,
        })
        .unwrap();
        doc.apply(Op::Delete { pages: vec![3] }).unwrap();
        assert_eq!(
            (doc.pages(), doc.undoable().as_deref()),
            (2, Some("delete page 3"))
        );
        assert_eq!(doc.undo().unwrap().as_deref(), Some("delete page 3"));
        assert_eq!(
            (doc.pages(), doc.redoable().as_deref()),
            (3, Some("delete page 3"))
        );
        doc.undo().unwrap();
        assert!(
            !doc.dirty(),
            "undone to where it was opened: nothing to save"
        );
        assert_eq!(doc.undo().unwrap(), None);
        doc.redo().unwrap();
        assert_eq!(rotations(&doc.bytes().unwrap()), [90, 0, 0]);
        doc.apply(Op::Rotate {
            pages: vec![2],
            by: -90,
        })
        .unwrap();
        assert_eq!(
            doc.redoable(),
            None,
            "a new change lets go of the delete undone"
        );
        assert_eq!(rotations(&doc.bytes().unwrap()), [90, 270, 0]);
        assert!(
            doc.apply(Op::Delete {
                pages: vec![1, 2, 3]
            })
            .is_err(),
            "a PDF keeps a page"
        );
        assert!(doc
            .apply(Op::Rotate {
                pages: vec![9],
                by: 90
            })
            .is_err());
        assert_eq!(
            doc.undoable().as_deref(),
            Some("turn page 2"),
            "a refused change is not in the log"
        );
    }

    #[test]
    fn a_save_is_atomic_kept_across_by_the_log_and_the_next_one_appended_again() {
        let dir = std::env::temp_dir().join(format!("quark-edit-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("paper.pdf");
        let bytes = fixture(&["one", "two"]);
        std::fs::write(&path, bytes.as_slice()).unwrap();
        let mut doc = editable(bytes.clone());
        doc.apply(Op::Rotate {
            pages: vec![1],
            by: 90,
        })
        .unwrap();
        doc.save(&path).unwrap();
        assert!(!doc.dirty());
        let first = std::fs::read(&path).unwrap();
        assert!(first.starts_with(&bytes) && rotations(&first) == [90, 0]);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "nothing left beside it"
        );

        // Undo after saving is a change to save; the next save is appended to the last.
        doc.undo().unwrap();
        assert!(doc.dirty());
        doc.apply(Op::Rotate {
            pages: vec![2],
            by: 180,
        })
        .unwrap();
        doc.save(&path).unwrap();
        let second = std::fs::read(&path).unwrap();
        assert!(second.starts_with(&first), "appended to what was saved");
        assert_eq!(rotations(&second), [0, 180]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undone_past_a_save_and_changed_again_what_is_written_is_what_is_made() {
        let dir = std::env::temp_dir().join(format!("quark-edit-patches-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.pdf");
        let mut doc = editable(fixture(&["One", "Two", "Three"]));
        let note = |text: &str| crate::annot::Mark {
            kind: "note".into(),
            boxes: Vec::new(),
            rect: Some([100.0, 700.0, 0.0, 0.0]),
            strokes: Vec::new(),
            color: "#ffd400".into(),
            text: text.into(),
        };
        let read = |bytes: Vec<u8>| {
            let d = Document::load_mem(&bytes).unwrap();
            (d.get_pages().len(), crate::annot::list(&d).len())
        };
        doc.apply(Op::Comment {
            page: 1,
            mark: note("a"),
            author: "Ada".into(),
            date: "D:2026".into(),
            name: "a".into(),
        })
        .unwrap();
        doc.apply(Op::Delete { pages: vec![3] }).unwrap();
        doc.save(&path).unwrap();
        assert_eq!(read(std::fs::read(&path).unwrap()), (2, 1));
        // Undone past the save: the page and the comment back, as written.
        doc.undo().unwrap();
        doc.undo().unwrap();
        assert_eq!(read(doc.bytes().unwrap()), (3, 0));
        // A new change there: the file on disk is no state of the log now.
        doc.apply(Op::Comment {
            page: 2,
            mark: note("b"),
            author: "Ada".into(),
            date: "D:2026".into(),
            name: "b".into(),
        })
        .unwrap();
        let made = doc.bytes().unwrap();
        assert!(
            made.starts_with(&std::fs::read(&path).unwrap()),
            "appended to what is on disk"
        );
        assert_eq!(read(made), (3, 1));
        assert!(doc.redoable().is_none(), "what was undone is let go of");
        doc.undo().unwrap();
        assert_eq!(read(doc.bytes().unwrap()), (3, 0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_layout_moves_with_the_pages_and_what_they_draw_and_not_with_comments_or_rotations() {
        let mut doc = editable(fixture(&["One", "Two", "Three"]));
        let note = crate::annot::Mark {
            kind: "note".into(),
            boxes: Vec::new(),
            rect: Some([100.0, 700.0, 0.0, 0.0]),
            strokes: Vec::new(),
            color: "#ffd400".into(),
            text: "Hm".into(),
        };
        doc.apply(Op::Comment {
            page: 1,
            mark: note,
            author: "Ada".into(),
            date: "D:2026".into(),
            name: "n".into(),
        })
        .unwrap();
        doc.apply(Op::Rotate {
            pages: vec![2],
            by: 90,
        })
        .unwrap();
        assert_eq!(
            doc.layout(),
            0,
            "a comment and a turn leave the pages' text as it was"
        );
        doc.apply(Op::Delete { pages: vec![3] }).unwrap();
        assert_eq!(doc.layout(), 1);
        doc.undo().unwrap();
        assert_eq!(
            doc.layout(),
            2,
            "undone, the pages are as they were — a layout of their own"
        );
        doc.undo().unwrap();
        assert_eq!(doc.layout(), 2, "undoing a turn changes no text");
        doc.redo().unwrap();
        doc.redo().unwrap();
        assert_eq!(doc.layout(), 3);
        assert!(doc.view_is_saved());
    }

    #[test]
    fn what_cannot_be_read_or_is_protected_opens_only_to_be_read() {
        assert!(
            matches!(Doc::open(Arc::new(b"%PDF-1.7\nnot really".to_vec())), Opened::ReadOnly(why) if why.contains("could not be read"))
        );
    }

    #[test]
    fn a_protected_pdf_asks_for_its_password_and_is_saved_under_the_same_keys_or_new_ones() {
        let dir = std::env::temp_dir().join(format!("quark-edit-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("locked.pdf");
        // Given a password, and saved.
        let mut doc = editable(fixture(&["Private."]));
        doc.apply(Op::Protect {
            protection: Protection {
                user: "pw".into(),
                owner: String::new(),
                print: true,
                copy: true,
                edit: false,
            },
        })
        .unwrap();
        assert!(doc.protected() && doc.dirty());
        assert!(
            !String::from_utf8_lossy(&doc.view_bytes().unwrap()).contains("/Encrypt"),
            "the window draws it without its password"
        );
        doc.save(&path).unwrap();
        let locked = Arc::new(std::fs::read(&path).unwrap());
        assert!(String::from_utf8_lossy(&locked).contains("/Encrypt"));

        // Opened: without it, or with the wrong one, it is locked; with it, it opens.
        assert!(matches!(
            Doc::open(locked.clone()),
            Opened::Locked { wrong: false }
        ));
        assert!(matches!(
            Doc::open_with(locked.clone(), Some("no")),
            Opened::Locked { wrong: true }
        ));
        let Opened::Editable(mut opened) = Doc::open_with(locked.clone(), Some("pw")) else {
            panic!("it did not open with its password")
        };
        assert!(opened.protected() && !opened.dirty());
        // Changed and saved: under the same keys.
        opened
            .apply(Op::Rotate {
                pages: vec![1],
                by: 90,
            })
            .unwrap();
        opened.save(&path).unwrap();
        let again = Arc::new(std::fs::read(&path).unwrap());
        assert!(
            matches!(Doc::open(again.clone()), Opened::Locked { .. }),
            "still protected"
        );
        assert!(matches!(
            Doc::open_with(again.clone(), Some("pw")),
            Opened::Editable(_)
        ));
        // Its password taken away.
        opened.apply(Op::Unprotect).unwrap();
        opened.save(&path).unwrap();
        assert!(
            matches!(
                Doc::open(Arc::new(std::fs::read(&path).unwrap())),
                Opened::Editable(_)
            ),
            "plain again"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
