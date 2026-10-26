//! A PDF's pages, rearranged: their order set, a blank one put in, pages
//! brought in from another PDF. Reordering flattens the page tree under its
//! root, so what a page inherited from the nodes between — its resources,
//! its size, its turn — is written on the page first.

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

/// What a page may inherit from the nodes above it.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// The root of the page tree.
pub(crate) fn root(doc: &Document) -> Result<ObjectId, String> {
    doc.catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| format!("the PDF has no page tree: {e}"))
}

/// A page's own value for `key`, or the nearest of its parents', as written
/// — a reference stays a reference.
fn inherited_raw(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut at = page;
    for _ in 0..64 {
        let dict = doc.get_object(at).and_then(Object::as_dict).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        at = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// Write on a page what it inherits, so it can be moved to another parent.
pub(crate) fn materialize(doc: &mut Document, page: ObjectId) -> Result<(), String> {
    let found: Vec<(&[u8], Object)> = INHERITED
        .iter()
        .filter_map(|key| inherited_raw(doc, page, key).map(|v| (*key, v)))
        .collect();
    let dict = doc
        .get_object_mut(page)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?;
    for (key, value) in found {
        if !dict.has(key) {
            dict.set(key, value);
        }
    }
    Ok(())
}

/// The pages, in order, by object.
pub(crate) fn order(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// Put the pages in `pages` order, directly under the root.
pub(crate) fn set_order(doc: &mut Document, pages: &[ObjectId]) -> Result<(), String> {
    if pages.is_empty() {
        return Err("a PDF keeps at least one page".into());
    }
    for page in pages {
        materialize(doc, *page)?;
    }
    let root = root(doc)?;
    for page in pages {
        doc.get_object_mut(*page)
            .and_then(Object::as_dict_mut)
            .map_err(|e| e.to_string())?
            .set("Parent", root);
    }
    let tree = doc
        .get_object_mut(root)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?;
    tree.set(
        "Kids",
        pages
            .iter()
            .map(|p| Object::Reference(*p))
            .collect::<Vec<_>>(),
    );
    tree.set("Count", pages.len() as i64);
    Ok(())
}

/// Page numbers, from one, as objects — every one there.
pub(crate) fn ids(doc: &Document, numbers: &[u32]) -> Result<Vec<ObjectId>, String> {
    let pages = doc.get_pages();
    numbers
        .iter()
        .map(|n| {
            pages
                .get(n)
                .copied()
                .ok_or_else(|| format!("there is no page {n}"))
        })
        .collect()
}

/// `count` blank pages after page `after` (0: before the first), each the
/// size of the page beside it.
pub fn insert_blank(doc: &mut Document, after: u32, count: u32) -> Result<(), String> {
    let mut pages = order(doc);
    if after as usize > pages.len() {
        return Err(format!("there is no page {after}"));
    }
    let beside = pages[(after as usize).saturating_sub(1).min(pages.len() - 1)];
    let size = inherited_raw(doc, beside, b"MediaBox")
        .unwrap_or_else(|| Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    let root = root(doc)?;
    let mut made = Vec::new();
    for _ in 0..count.max(1) {
        let contents = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        made.push(doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => root,
            "MediaBox" => size.clone(),
            "Resources" => dictionary! {},
            "Contents" => contents,
        }));
    }
    let at = after as usize;
    pages.splice(at..at, made);
    set_order(doc, &pages)
}

/// Move `numbers` — kept in their order — to before page `before` (one past
/// the last: to the end).
pub fn move_pages(doc: &mut Document, numbers: &[u32], before: u32) -> Result<(), String> {
    let pages = order(doc);
    if before == 0 || before as usize > pages.len() + 1 {
        return Err(format!("there is no page {before} to move them before"));
    }
    let mut moving: Vec<u32> = numbers.to_vec();
    moving.sort_unstable();
    moving.dedup();
    let moved = ids(doc, &moving)?;
    // Where they go, among the pages that stay.
    let staying: Vec<ObjectId> = pages
        .iter()
        .copied()
        .filter(|p| !moved.contains(p))
        .collect();
    let at = pages[..(before as usize - 1)]
        .iter()
        .filter(|p| !moved.contains(p))
        .count();
    let mut next = staying;
    next.splice(at..at, moved);
    set_order(doc, &next)
}

/// Bring the pages of `other` — `numbers` of them, or all — into `doc`
/// after page `after`; the pages brought in, in order.
pub fn insert_from(
    doc: &mut Document,
    other: &[u8],
    numbers: Option<&[u32]>,
    after: u32,
) -> Result<Vec<ObjectId>, String> {
    let mut pages = order(doc);
    if after as usize > pages.len() {
        return Err(format!("there is no page {after}"));
    }
    let brought = import(doc, other, numbers)?;
    let at = after as usize;
    pages.splice(at..at, brought.iter().copied());
    set_order(doc, &pages)?;
    Ok(brought)
}

/// The pages of another PDF's bytes — `numbers` of them, or all — added to
/// `doc`'s objects, written with what they inherited, numbered after
/// `doc`'s; not yet in its page tree.
pub(crate) fn import(
    doc: &mut Document,
    other: &[u8],
    numbers: Option<&[u32]>,
) -> Result<Vec<ObjectId>, String> {
    let mut other =
        Document::load_mem(other).map_err(|e| format!("the other PDF could not be read: {e}"))?;
    if other.was_encrypted() || other.is_encrypted() {
        return Err("the other PDF is protected by a password".into());
    }
    other.renumber_objects_with(doc.max_id + 1);
    let all = other.get_pages();
    let wanted: Vec<ObjectId> = match numbers {
        Some(numbers) => numbers
            .iter()
            .map(|n| {
                all.get(n)
                    .copied()
                    .ok_or_else(|| format!("the other PDF has no page {n}"))
            })
            .collect::<Result<_, _>>()?,
        None => all.values().copied().collect(),
    };
    for page in &wanted {
        materialize(&mut other, *page)?;
    }
    let max = other.max_id;
    for (id, object) in other.objects {
        match object.type_name().unwrap_or(b"") {
            b"Catalog" | b"Pages" | b"Outlines" => continue,
            b"Page" if !wanted.contains(&id) => continue,
            _ => {}
        }
        doc.objects.insert(id, object);
    }
    doc.max_id = doc.max_id.max(max);
    Ok(wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(pages: &[&str]) -> Document {
        Document::load_mem(&crate::fixture::fixture(pages)).unwrap()
    }

    fn texts(doc: &Document) -> Vec<String> {
        // Each fixture page draws its words; what a page draws is what it is.
        doc.get_pages()
            .values()
            .map(|id| {
                let content = doc.get_page_content(*id);
                String::from_utf8_lossy(&content)
                    .split('(')
                    .nth(1)
                    .and_then(|s| s.split(')').next())
                    .unwrap_or("blank")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn pages_move_in_order_and_blanks_go_where_they_are_put() {
        let mut doc = fixture(&["one", "two", "three", "four"]);
        move_pages(&mut doc, &[3, 1], 5).unwrap();
        assert_eq!(
            texts(&doc),
            ["two", "four", "one", "three"],
            "to the end, in their order"
        );
        move_pages(&mut doc, &[4], 1).unwrap();
        assert_eq!(texts(&doc), ["three", "two", "four", "one"]);
        insert_blank(&mut doc, 2, 2).unwrap();
        assert_eq!(
            texts(&doc),
            ["three", "two", "blank", "blank", "four", "one"]
        );
        insert_blank(&mut doc, 0, 1).unwrap();
        assert_eq!(texts(&doc)[0], "blank");
        assert!(move_pages(&mut doc, &[1], 99).is_err() && insert_blank(&mut doc, 99, 1).is_err());
        // Every page keeps a size and its resources, wherever it went.
        for id in doc.get_pages().values() {
            let page = doc.get_object(*id).unwrap().as_dict().unwrap();
            assert!(page.has(b"MediaBox") || inherited_raw(&doc, *id, b"MediaBox").is_some());
        }
    }

    #[test]
    fn pages_are_brought_in_from_another_pdf_with_what_they_inherited() {
        let mut doc = fixture(&["one", "two"]);
        let other = crate::fixture::fixture(&["alpha", "beta", "gamma"]);
        let brought = insert_from(&mut doc, &other, Some(&[3, 1]), 1).unwrap();
        assert_eq!(brought.len(), 2);
        assert_eq!(texts(&doc), ["one", "gamma", "alpha", "two"]);
        let page = doc.get_object(brought[0]).unwrap().as_dict().unwrap();
        assert!(
            page.has(b"Resources") && page.has(b"MediaBox"),
            "a page brought in carries what it inherited"
        );
        // And it reads back.
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert_eq!(
            texts(&Document::load_mem(&bytes).unwrap()),
            ["one", "gamma", "alpha", "two"]
        );
    }
}
