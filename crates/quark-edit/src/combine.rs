//! New PDFs made of others: some pages taken out into a file of their own,
//! a PDF split into parts, PDFs combined into one — each part a bookmark.
//! The PDFs they are made from are not changed.

use lopdf::{Bookmark, Document, Object};

use crate::pages;

/// `doc` with only `numbers` of its pages — in the order given — as a PDF
/// of its own. Its bookmarks go: they would point at pages it has not got.
pub fn extract(doc: &Document, numbers: &[u32]) -> Result<Vec<u8>, String> {
    if numbers.is_empty() {
        return Err("choose the pages to take out".into());
    }
    let mut part = doc.clone();
    let keep = pages::ids(&part, numbers)?;
    pages::set_order(&mut part, &keep)?;
    if let Ok(catalog) = part.catalog_mut() {
        catalog.remove(b"Outlines");
    }
    part.prune_objects();
    part.renumber_objects();
    let mut out = Vec::new();
    part.save_to(&mut out)
        .map_err(|e| format!("the pages could not be written: {e}"))?;
    Ok(out)
}

/// The ranges of pages a split makes: every `every` pages, or a new part
/// starting at each of `at` — page numbers from one.
pub fn ranges(pages: u32, every: Option<u32>, at: &[u32]) -> Result<Vec<Vec<u32>>, String> {
    let mut starts: Vec<u32> = match every {
        Some(0) => return Err("split every one page or more".into()),
        Some(n) => (1..=pages).step_by(n as usize).collect(),
        None => {
            let mut s: Vec<u32> = std::iter::once(1)
                .chain(at.iter().copied().filter(|p| *p > 1 && *p <= pages))
                .collect();
            s.sort_unstable();
            s.dedup();
            s
        }
    };
    if starts.len() < 2 {
        return Err("that makes one part — the PDF as it is".into());
    }
    starts.push(pages + 1);
    Ok(starts.windows(2).map(|w| (w[0]..w[1]).collect()).collect())
}

/// PDFs combined into one, in order — each a bookmark, by its name, at its
/// first page. Their own bookmarks are let go of.
pub fn combine(parts: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let Some((first_name, first)) = parts.first() else {
        return Err("choose the PDFs to combine".into());
    };
    if parts.len() < 2 {
        return Err("combining takes two PDFs or more".into());
    }
    let mut doc =
        Document::load_mem(first).map_err(|e| format!("{first_name} could not be read: {e}"))?;
    if doc.was_encrypted() || doc.is_encrypted() {
        return Err(format!("{first_name} is protected by a password"));
    }
    if let Ok(catalog) = doc.catalog_mut() {
        catalog.remove(b"Outlines");
    }
    let mut order = pages::order(&doc);
    let mut starts = vec![(first_name.clone(), order[0])];
    for (name, bytes) in &parts[1..] {
        let brought = pages::import(&mut doc, bytes, None).map_err(|e| format!("{name}: {e}"))?;
        if let Some(first) = brought.first() {
            starts.push((name.clone(), *first));
        }
        order.extend(brought);
    }
    pages::set_order(&mut doc, &order)?;
    for (name, page) in starts {
        doc.add_bookmark(Bookmark::new(name, [0.0, 0.0, 0.0], 0, page), None);
    }
    if let Some(outline) = doc.build_outline() {
        let catalog = doc.catalog_mut().map_err(|e| e.to_string())?;
        catalog.set("Outlines", Object::Reference(outline));
        catalog.set("PageMode", "UseOutlines");
    }
    doc.prune_objects();
    doc.renumber_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| format!("the combined PDF could not be written: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(bytes: &[u8]) -> usize {
        Document::load_mem(bytes).unwrap().get_pages().len()
    }

    #[test]
    fn pages_are_taken_out_and_a_pdf_split_into_parts() {
        let doc = Document::load_mem(&crate::fixture::fixture(&["a", "b", "c", "d", "e"])).unwrap();
        assert_eq!(count(&extract(&doc, &[4, 2]).unwrap()), 2);
        assert_eq!(
            ranges(5, Some(2), &[]).unwrap(),
            [vec![1, 2], vec![3, 4], vec![5]]
        );
        assert_eq!(
            ranges(5, None, &[4, 2, 9]).unwrap(),
            [vec![1], vec![2, 3], vec![4, 5]]
        );
        assert!(
            ranges(5, Some(5), &[]).is_err()
                && ranges(5, Some(0), &[]).is_err()
                && ranges(5, None, &[]).is_err()
        );
        assert_eq!(
            doc.get_pages().len(),
            5,
            "the PDF it came from is as it was"
        );
    }

    #[test]
    fn pdfs_are_combined_in_order_each_a_bookmark() {
        let one = crate::fixture::fixture(&["a", "b"]);
        let two = crate::fixture::fixture(&["c", "d", "e"]);
        let made = combine(&[("one.pdf".into(), one.clone()), ("two.pdf".into(), two)]).unwrap();
        let doc = Document::load_mem(&made).unwrap();
        assert_eq!(doc.get_pages().len(), 5);
        let toc = doc.get_toc().unwrap();
        assert_eq!(
            toc.toc
                .iter()
                .map(|t| (t.title.clone(), t.page))
                .collect::<Vec<_>>(),
            [("one.pdf".to_string(), 1), ("two.pdf".to_string(), 3)]
        );
        assert!(combine(&[("one.pdf".into(), one)]).is_err());
    }
}
