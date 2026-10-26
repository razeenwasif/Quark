//! A PDF written: as an update appended to the file it was — exactly the
//! objects that differ from the file — or whole; and on disk atomically, so
//! a save cut short leaves the file as it was.

use std::io::Write;
use std::path::Path;

use lopdf::xref::{Xref, XrefType};
use lopdf::{Dictionary, Document, IncrementalDocument, ObjectId};

/// What an update appended to a file needs of the file — its trailer, where
/// its cross-reference starts and of which kind, its last object's number —
/// and nothing of its objects, which are not copied to write one.
#[derive(Debug, Clone)]
pub struct Base {
    trailer: Dictionary,
    xref_start: usize,
    xref_type: XrefType,
    max_id: u32,
    max_bookmark_id: u32,
}

impl Base {
    pub fn of(doc: &Document) -> Base {
        Base {
            trailer: doc.trailer.clone(),
            xref_start: doc.xref_start,
            xref_type: doc.reference_table.cross_reference_type,
            max_id: doc.max_id,
            max_bookmark_id: doc.max_bookmark_id,
        }
    }

    /// The file as an update's starting point: no objects, only what is
    /// pointed back to.
    fn document(&self) -> Document {
        let mut doc = Document::new();
        doc.trailer = self.trailer.clone();
        doc.xref_start = self.xref_start;
        doc.reference_table = Xref::new(0, self.xref_type);
        doc.max_id = self.max_id;
        doc.max_bookmark_id = self.max_bookmark_id;
        doc
    }
}

/// `current` as an update appended to `disk`, the file `base` is of: the
/// objects among `changed` that are in it now — every one that may differ
/// from the file — and the trailer pointing back.
pub fn update<'a>(
    disk: &[u8],
    base: &Base,
    current: &Document,
    changed: impl IntoIterator<Item = &'a ObjectId>,
) -> Result<Vec<u8>, String> {
    let mut update = IncrementalDocument::create_from(disk.to_vec(), base.document());
    for id in changed {
        if let Some(object) = current.objects.get(id) {
            update.new_document.objects.insert(*id, object.clone());
        }
    }
    update.new_document.max_id = current.max_id.max(base.max_id);
    // What the document is now: its catalog and its information.
    for key in [b"Root".as_slice(), b"Info"] {
        match current.trailer.get(key) {
            Ok(value) => update.new_document.trailer.set(key, value.clone()),
            Err(_) => {
                update.new_document.trailer.remove(key);
            }
        }
    }
    let mut out = Vec::with_capacity(disk.len() + 4096);
    update
        .save_to(&mut out)
        .map_err(|e| format!("the update could not be written: {e}"))?;
    Ok(out)
}

/// `current` written whole.
pub fn whole(current: &Document) -> Result<Vec<u8>, String> {
    let mut doc = current.clone();
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| format!("the PDF could not be written: {e}"))?;
    Ok(out)
}

/// `bytes` written to `path` atomically: to a file beside it, made durable,
/// then put in its place — what was there is left as it was if any step
/// fails.
pub fn atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document.pdf".into());
    let temporary = dir.join(format!(".{name}.quark-{}", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // The file's own permissions, when it has some.
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&temporary, meta.permissions());
        }
        std::fs::rename(&temporary, path)?;
        if let Ok(folder) = std::fs::File::open(dir) {
            let _ = folder.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(())
}
