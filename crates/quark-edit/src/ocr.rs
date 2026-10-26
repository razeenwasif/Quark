//! What OCR read, laid over the page it read: tesseract's text-only PDF of
//! a page — its words invisible, where they are in the picture — brought in
//! as a form XObject and drawn over the page, turned and placed as the page
//! is shown, so its words can be found, selected and copied.

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

use crate::pages;

/// The matrix that puts a page drawn as it is shown — `rotate` degrees
/// clockwise, its crop box `[x0, y0, x1, y1]` — back into the page's own
/// space; `width` and `height` the size it was drawn at, in points.
pub fn placing(rotate: i64, crop: [f32; 4], width: f32, height: f32) -> [f32; 6] {
    let [x0, y0, x1, y1] = crop;
    let (w, h) = (x1 - x0, y1 - y0);
    let turned = matches!(rotate.rem_euclid(360), 90 | 270);
    // From the size drawn to the size shown.
    let (shown_w, shown_h) = if turned { (h, w) } else { (w, h) };
    let (sx, sy) = (shown_w / width.max(1.0), shown_h / height.max(1.0));
    // Shown (u, v) to the page's (x, y), relative to its crop box.
    let [a, b, c, d, e, f] = match rotate.rem_euclid(360) {
        90 => [0.0, 1.0, -1.0, 0.0, w, 0.0],
        180 => [-1.0, 0.0, 0.0, -1.0, w, h],
        270 => [0.0, -1.0, 1.0, 0.0, 0.0, h],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };
    [a * sx, b * sx, c * sy, d * sy, e + x0, f + y0]
}

/// The page's crop box — its media box when it has none — and its turn.
pub(crate) fn shown_as(doc: &Document, page: ObjectId) -> ([f32; 4], i64) {
    let numbers = |key: &[u8]| -> Option<[f32; 4]> {
        let mut at = page;
        for _ in 0..32 {
            let d = doc.get_object(at).and_then(Object::as_dict).ok()?;
            if let Ok(v) = d.get(key) {
                let v = match v {
                    Object::Reference(id) => doc.get_object(*id).ok()?,
                    other => other,
                };
                let n: Vec<f32> = v
                    .as_array()
                    .ok()?
                    .iter()
                    .filter_map(|o| o.as_float().ok())
                    .collect();
                return (n.len() == 4).then(|| {
                    [
                        n[0].min(n[2]),
                        n[1].min(n[3]),
                        n[0].max(n[2]),
                        n[1].max(n[3]),
                    ]
                });
            }
            at = d.get(b"Parent").and_then(Object::as_reference).ok()?;
        }
        None
    };
    let crop = numbers(b"CropBox")
        .or_else(|| numbers(b"MediaBox"))
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let rotate = crate::ops::inherited(doc, page, b"Rotate")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    (crop, rotate)
}

/// Lay the words of `text` — a PDF of one page, as tesseract writes one —
/// over page `page` (from one) of `doc`.
pub fn graft(doc: &mut Document, page: u32, text: &[u8]) -> Result<(), String> {
    let target = *pages::ids(doc, &[page])?.first().ok_or("no page")?;
    let mut words = Document::load_mem(text)
        .map_err(|e| format!("what OCR read of page {page} could not be read: {e}"))?;
    words.renumber_objects_with(doc.max_id + 1);
    let source = *words
        .get_pages()
        .values()
        .next()
        .ok_or_else(|| format!("OCR read nothing of page {page}"))?;
    pages::materialize(&mut words, source)?;
    let source_dict = words
        .get_object(source)
        .and_then(Object::as_dict)
        .map_err(|e| e.to_string())?
        .clone();
    let content = words.get_page_content(source);
    let resources = match source_dict.get(b"Resources") {
        Ok(Object::Reference(id)) => words
            .get_object(*id)
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_default(),
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => dictionary! {},
    };
    let size: Vec<f32> = source_dict
        .get(b"MediaBox")
        .and_then(Object::as_array)
        .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
        .unwrap_or_default();
    let [w, h] = if size.len() == 4 {
        [size[2] - size[0], size[3] - size[1]]
    } else {
        [612.0, 792.0]
    };
    // Its fonts and whatever else its words need, numbered after the document's.
    let max = words.max_id;
    for (id, object) in words.objects {
        if matches!(
            object.type_name().unwrap_or(b""),
            b"Catalog" | b"Pages" | b"Page"
        ) {
            continue;
        }
        doc.objects.insert(id, object);
    }
    doc.max_id = doc.max_id.max(max);
    let form = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)],
            "Resources" => Object::Dictionary(resources),
        },
        content,
    ));
    let (crop, rotate) = shown_as(doc, target);
    let m = placing(rotate, crop, w, h);
    pages::materialize(doc, target)?;
    let page_dict = doc
        .get_object(target)
        .and_then(Object::as_dict)
        .map_err(|e| e.to_string())?
        .clone();
    let mut page_resources = match page_dict.get(b"Resources") {
        Ok(Object::Reference(id)) => doc
            .get_object(*id)
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_default(),
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => dictionary! {},
    };
    let mut xobjects = match page_resources.get(b"XObject") {
        Ok(Object::Reference(id)) => doc
            .get_object(*id)
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_default(),
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => dictionary! {},
    };
    let name = (1..)
        .map(|i| format!("QuarkOCR{i}"))
        .find(|n| !xobjects.has(n.as_bytes()))
        .unwrap_or_default();
    xobjects.set(name.as_bytes(), Object::Reference(form));
    page_resources.set("XObject", Object::Dictionary(xobjects));
    // The page's own drawing kept to itself, then the words over it.
    let before = doc.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
    let after = doc.add_object(Stream::new(
        dictionary! {},
        format!(
            "\nQ\nq {} {} {} {} {} {} cm /{name} Do Q\n",
            m[0], m[1], m[2], m[3], m[4], m[5]
        )
        .into_bytes(),
    ));
    let mut contents = match page_dict.get(b"Contents") {
        Ok(Object::Array(items)) => items.clone(),
        Ok(other) => vec![other.clone()],
        Err(_) => Vec::new(),
    };
    contents.insert(0, Object::Reference(before));
    contents.push(Object::Reference(after));
    let page = doc
        .get_object_mut(target)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?;
    page.set("Resources", Object::Dictionary(page_resources));
    page.set("Contents", Object::Array(contents));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_shown_turned_is_placed_back_into_its_own_space() {
        // Upright: as drawn.
        assert_eq!(
            placing(0, [0.0, 0.0, 612.0, 792.0], 612.0, 792.0),
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
        );
        // Turned a quarter: shown 792 wide, its top left the page's bottom left turned.
        let m = placing(90, [0.0, 0.0, 612.0, 792.0], 792.0, 612.0);
        let at = |u: f32, v: f32| [m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]];
        assert_eq!(at(0.0, 0.0), [612.0, 0.0]);
        assert_eq!(at(792.0, 612.0), [0.0, 792.0]);
        // A crop box moves it; a picture drawn at another size is scaled to the page.
        assert_eq!(
            placing(0, [10.0, 20.0, 110.0, 220.0], 200.0, 400.0),
            [0.5, 0.0, 0.0, 0.5, 10.0, 20.0]
        );
        let half = placing(180, [0.0, 0.0, 100.0, 200.0], 100.0, 200.0);
        assert_eq!(half, [-1.0, 0.0, 0.0, -1.0, 100.0, 200.0]);
    }

    #[test]
    fn the_words_read_are_laid_over_their_page_in_a_form_of_their_own() {
        let mut doc = Document::load_mem(&crate::fixture::fixture(&["(a scan)", "two"])).unwrap();
        let words = crate::fixture::fixture(&["Words OCR found."]);
        graft(&mut doc, 1, &words).unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let read = Document::load_mem(&bytes).unwrap();
        let page = read.get_pages()[&1];
        let content = String::from_utf8_lossy(&read.get_page_content(page)).to_string();
        assert!(
            content.starts_with("q\n") && content.contains("/QuarkOCR1 Do"),
            "{content}"
        );
        let resources = read
            .get_object(page)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap();
        let form = resources
            .get(b"XObject")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"QuarkOCR1")
            .unwrap()
            .as_reference()
            .unwrap();
        let stream = read.get_object(form).unwrap().as_stream().unwrap();
        let drawn = String::from_utf8_lossy(
            &stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone()),
        )
        .to_string();
        assert!(drawn.contains("Words OCR found."), "{drawn}");
        // Its font came with it.
        let fonts = stream
            .dict
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap();
        assert!(matches!(
            fonts,
            Object::Dictionary(_) | Object::Reference(_)
        ));
        assert!(graft(&mut doc, 9, &words).is_err());
    }
}
