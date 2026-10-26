//! A PDF's properties — title, author, subject, keywords — read and
//! changed; what it carries that its reader does not see taken out; and the
//! whole made smaller.

use lopdf::{dictionary, Document, Object};
use serde::{Deserialize, Serialize};

use crate::annot::{read_text, text_string};

/// What a PDF says of itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    /// As the PDF has them: `D:20261012143000Z`.
    pub created: String,
    pub modified: String,
    pub version: String,
    pub pages: u32,
    /// What it holds that is not seen on its pages.
    pub hidden: Vec<String>,
}

/// The document information dictionary, followed through its reference.
fn info_dict(doc: &Document) -> Option<lopdf::Dictionary> {
    match doc.trailer.get(b"Info").ok()? {
        Object::Reference(id) => doc.get_object(*id).and_then(Object::as_dict).ok().cloned(),
        Object::Dictionary(d) => Some(d.clone()),
        _ => None,
    }
}

/// What it carries that its reader does not see.
fn hidden(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    if catalog.has(b"Metadata") {
        out.push("XMP metadata".into());
    }
    let names = catalog.get(b"Names").ok().and_then(|n| match n {
        Object::Reference(id) => doc.get_object(*id).and_then(Object::as_dict).ok().cloned(),
        Object::Dictionary(d) => Some(d.clone()),
        _ => None,
    });
    if let Some(names) = names {
        if names.has(b"JavaScript") {
            out.push("JavaScript".into());
        }
        if names.has(b"EmbeddedFiles") {
            out.push("attached files".into());
        }
    }
    if catalog.has(b"OpenAction") || catalog.has(b"AA") {
        out.push("actions run on opening".into());
    }
    if catalog.has(b"PieceInfo") {
        out.push("an editor's private data".into());
    }
    let pages = doc.get_pages();
    let mut thumbs = false;
    let mut piece = false;
    for id in pages.values() {
        if let Ok(page) = doc.get_object(*id).and_then(Object::as_dict) {
            thumbs |= page.has(b"Thumb");
            piece |= page.has(b"PieceInfo") || page.has(b"AA");
        }
    }
    if thumbs {
        out.push("page thumbnails".into());
    }
    if piece && !out.iter().any(|h| h.contains("private")) {
        out.push("an editor's private data".into());
    }
    if info_dict(doc).is_some_and(|i| i.has(b"Author") || i.has(b"Creator") || i.has(b"Producer")) {
        out.push("who made it, and with what".into());
    }
    out
}

/// What a PDF says of itself.
pub fn info(doc: &Document) -> Info {
    let dict = info_dict(doc).unwrap_or_default();
    let text = |key: &[u8]| {
        dict.get(key)
            .and_then(Object::as_str)
            .map(read_text)
            .unwrap_or_default()
    };
    Info {
        title: text(b"Title"),
        author: text(b"Author"),
        subject: text(b"Subject"),
        keywords: text(b"Keywords"),
        creator: text(b"Creator"),
        producer: text(b"Producer"),
        created: text(b"CreationDate"),
        modified: text(b"ModDate"),
        version: doc.version.clone(),
        pages: doc.get_pages().len() as u32,
        hidden: hidden(doc),
    }
}

/// Set its title, author, subject and keywords — an empty one taken out.
pub fn set_info(
    doc: &mut Document,
    title: &str,
    author: &str,
    subject: &str,
    keywords: &str,
    date: &str,
) -> Result<(), String> {
    let mut dict = info_dict(doc).unwrap_or_default();
    for (key, value) in [
        ("Title", title),
        ("Author", author),
        ("Subject", subject),
        ("Keywords", keywords),
    ] {
        if value.trim().is_empty() {
            dict.remove(key.as_bytes());
        } else {
            dict.set(key, text_string(value.trim()));
        }
    }
    dict.set("ModDate", Object::string_literal(date));
    match doc.trailer.get(b"Info") {
        Ok(Object::Reference(id)) => {
            let id = *id;
            doc.objects.insert(id, Object::Dictionary(dict));
        }
        _ => {
            let id = doc.add_object(dict);
            doc.trailer.set("Info", id);
        }
    }
    Ok(())
}

/// What it carries that its reader does not see, taken out: XMP metadata,
/// JavaScript and actions run on opening, attached files, thumbnails, an
/// editor's private data, and who made it — its title kept.
pub fn sanitize(doc: &mut Document) -> Result<(), String> {
    let catalog = doc.catalog_mut().map_err(|e| e.to_string())?;
    for key in [b"Metadata".as_slice(), b"OpenAction", b"AA", b"PieceInfo"] {
        catalog.remove(key);
    }
    let names = catalog.get(b"Names").ok().cloned();
    match names {
        Some(Object::Reference(id)) => {
            if let Ok(names) = doc.get_object_mut(id).and_then(Object::as_dict_mut) {
                names.remove(b"JavaScript");
                names.remove(b"EmbeddedFiles");
            }
        }
        Some(Object::Dictionary(mut names)) => {
            names.remove(b"JavaScript");
            names.remove(b"EmbeddedFiles");
            doc.catalog_mut()
                .map_err(|e| e.to_string())?
                .set("Names", Object::Dictionary(names));
        }
        _ => {}
    }
    for id in doc.get_pages().into_values() {
        if let Ok(page) = doc.get_object_mut(id).and_then(Object::as_dict_mut) {
            for key in [b"Thumb".as_slice(), b"PieceInfo", b"AA"] {
                page.remove(key);
            }
        }
    }
    let title = info(doc).title;
    let kept = if title.is_empty() {
        dictionary! {}
    } else {
        dictionary! { "Title" => text_string(&title) }
    };
    match doc.trailer.get(b"Info") {
        Ok(Object::Reference(id)) => {
            let id = *id;
            doc.objects.insert(id, Object::Dictionary(kept));
        }
        _ => {
            doc.trailer.remove(b"Info");
        }
    }
    doc.prune_objects();
    Ok(())
}

/// The whole written smaller: its streams compressed, its objects packed
/// into object streams, what nothing refers to taken out.
pub fn compressed(doc: &Document) -> Result<Vec<u8>, String> {
    let mut doc = doc.clone();
    doc.prune_objects();
    doc.compress();
    let mut out = Vec::new();
    doc.save_modern(&mut out)
        .map_err(|e| format!("it could not be written: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_are_read_set_and_what_is_hidden_taken_out() {
        let mut doc = Document::load_mem(&crate::fixture::fixture(&["Words."])).unwrap();
        set_info(
            &mut doc,
            "A Title",
            "Ada Lovelace",
            "Notes",
            "engines, numbers",
            "D:20261012000000Z",
        )
        .unwrap();
        let read = info(&doc);
        assert_eq!(
            (
                read.title.as_str(),
                read.author.as_str(),
                read.keywords.as_str(),
                read.modified.as_str()
            ),
            (
                "A Title",
                "Ada Lovelace",
                "engines, numbers",
                "D:20261012000000Z"
            )
        );
        // Hidden: metadata, a script, a thumbnail.
        let xmp = doc.add_object(lopdf::Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            b"<x/>".to_vec(),
        ));
        let js = doc.add_object(dictionary! { "Names" => vec![] });
        let names = doc.add_object(dictionary! { "JavaScript" => js });
        let catalog = doc.catalog_mut().unwrap();
        catalog.set("Metadata", xmp);
        catalog.set("Names", names);
        let page = doc.get_pages()[&1];
        doc.get_object_mut(page)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Thumb", 1);
        assert_eq!(
            info(&doc).hidden,
            [
                "XMP metadata",
                "JavaScript",
                "page thumbnails",
                "who made it, and with what"
            ]
        );
        sanitize(&mut doc).unwrap();
        let after = info(&doc);
        assert!(after.hidden.is_empty(), "{:?}", after.hidden);
        assert_eq!(
            (after.title.as_str(), after.author.as_str()),
            ("A Title", ""),
            "the title is kept"
        );
        set_info(&mut doc, "", "", "", "", "D:1").unwrap();
        assert_eq!(info(&doc).title, "");
    }

    #[test]
    fn the_whole_is_written_smaller_and_still_reads() {
        let words: Vec<String> = (0..60)
            .map(|i| {
                format!("Line {i} of a page that says much the same thing again and again.\n")
                    .repeat(3)
            })
            .collect();
        let pages: Vec<&str> = words.iter().map(String::as_str).collect();
        let bytes = crate::fixture::fixture(&pages);
        let doc = Document::load_mem(&bytes).unwrap();
        let smaller = compressed(&doc).unwrap();
        assert!(
            smaller.len() < bytes.len(),
            "{} → {}",
            bytes.len(),
            smaller.len()
        );
        assert_eq!(Document::load_mem(&smaller).unwrap().get_pages().len(), 60);
    }
}
