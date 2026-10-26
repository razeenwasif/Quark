//! Between Quark's annotations and `quark-edit`'s document.
//!
//! The edits a person makes are `quark-edit` changes, kept in its log and
//! saved by appending to the file; what Quark draws and lets the person pick
//! up is its own [`Annotation`]. This reads the one out of the other — every
//! kind, with its geometry, opacity, width, fill and replies, named by its
//! object number so it is found again whatever else changes — and says a new
//! annotation as the comment `quark-edit` writes.

use lopdf::{Dictionary, Document, Object, ObjectId};
use quark_core::annot::{
    AnnotId, AnnotKind, Annotation, BorderStyle, Color, InkStroke, LineEnding, NoteIcon, Reply,
    ReviewStatus, TextAlign,
};
use quark_core::geom::{Rect, Vec2};
use quark_edit::annot::{read_text, Mark};

use crate::forms::{FieldKind, FormField};

/// The comment a new annotation is, as `quark-edit` writes one; none for a
/// kind that is not a comment of its own — a redaction mark, which is
/// applied, a stamp, an attachment.
pub fn mark_of(a: &Annotation) -> Option<Mark> {
    let boxes = |quads: &[Rect]| quads.iter().map(|r| [r.min.x, r.min.y, r.max.x, r.max.y]).collect();
    let points = |p: &[Vec2]| p.iter().map(|v| [v.x, v.y]).collect::<Vec<_>>();
    let r = a.rect.normalize();
    let mut mark = Mark { color: hex(a.color), text: a.contents.clone(), ..Mark::default() };
    let lined = |m: &mut Mark| {
        m.width = Some(a.border_width);
        m.fill = a.interior_color.map(hex);
    };
    match &a.kind {
        AnnotKind::Highlight { quads } => {
            mark.kind = "highlight".into();
            mark.boxes = boxes(quads);
        }
        AnnotKind::Underline { quads } => {
            mark.kind = "underline".into();
            mark.boxes = boxes(quads);
        }
        AnnotKind::StrikeOut { quads } => {
            mark.kind = "strike".into();
            mark.boxes = boxes(quads);
        }
        AnnotKind::Squiggly { quads } => {
            mark.kind = "squiggly".into();
            mark.boxes = boxes(quads);
        }
        // A note is placed by its top left corner.
        AnnotKind::Note { .. } => {
            mark.kind = "note".into();
            mark.rect = Some([r.min.x, r.max.y, 0.0, 0.0]);
        }
        AnnotKind::FreeText { text, .. } => {
            mark.kind = "text".into();
            mark.rect = Some([r.min.x, r.min.y, r.max.x, r.max.y]);
            mark.text = text.clone();
        }
        AnnotKind::Ink { strokes } => {
            mark.kind = "ink".into();
            mark.strokes = strokes.iter().map(|s| points(&s.points)).collect();
            mark.width = Some(a.border_width);
        }
        AnnotKind::Line { start, end, start_ending, end_ending, .. } => {
            let arrow = *end_ending != LineEnding::None || *start_ending != LineEnding::None;
            mark.kind = if arrow { "arrow" } else { "line" }.into();
            // An arrow's head is at its end: one drawn from its end turned round.
            let (a1, a2) = if *start_ending != LineEnding::None && *end_ending == LineEnding::None {
                (end, start)
            } else {
                (start, end)
            };
            mark.rect = Some([a1.x, a1.y, a2.x, a2.y]);
            mark.width = Some(a.border_width);
        }
        AnnotKind::Square => {
            mark.kind = "square".into();
            mark.rect = Some([r.min.x, r.min.y, r.max.x, r.max.y]);
            lined(&mut mark);
        }
        AnnotKind::Circle => {
            mark.kind = "circle".into();
            mark.rect = Some([r.min.x, r.min.y, r.max.x, r.max.y]);
            lined(&mut mark);
        }
        AnnotKind::Polygon { points: p } => {
            mark.kind = "polygon".into();
            mark.strokes = vec![points(p)];
            lined(&mut mark);
        }
        AnnotKind::Polyline { points: p, .. } => {
            mark.kind = "polyline".into();
            mark.strokes = vec![points(p)];
            mark.width = Some(a.border_width);
        }
        AnnotKind::Stamp { .. }
        | AnnotKind::Redact { .. }
        | AnnotKind::FileAttachment { .. }
        | AnnotKind::Unsupported { .. } => return None,
    }
    // A highlight is see-through as it is; anything else only if it was made so.
    let usual = if matches!(a.kind, AnnotKind::Highlight { .. }) { 0.4 } else { 1.0 };
    if (a.opacity - usual).abs() > 0.01 {
        mark.opacity = Some(a.opacity);
    }
    Some(mark)
}

/// `#rrggbb`.
pub fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// How `quark-edit` names an annotation: its object, `12 0`.
pub fn comment_id(id: AnnotId) -> String {
    format!("{} 0", id.0)
}

/// The form's fields, a widget each — a box or button, with its own "on" —
/// on the page and at the place each is drawn.
pub fn fields(doc: &quark_edit::Doc) -> Vec<FormField> {
    let mut out = Vec::new();
    for f in doc.fields() {
        let kind = match f.kind.as_str() {
            "text" => FieldKind::Text,
            "checkbox" => FieldKind::Checkbox,
            "radio" => FieldKind::RadioButton,
            "combo" => FieldKind::ComboBox,
            "list" => FieldKind::ListBox,
            "button" => FieldKind::PushButton,
            "signature" => FieldKind::Signature,
            _ => FieldKind::Unknown,
        };
        for (i, w) in f.widgets.iter().enumerate() {
            let [x1, y1, x2, y2] = w.rect;
            out.push(FormField {
                name: f.name.clone(),
                kind,
                page: w.page.saturating_sub(1) as usize,
                rect: Rect::new(Vec2::new(x1, y1), Vec2::new(x2, y2)).normalize(),
                value: f.value.clone(),
                options: f.options.clone(),
                read_only: f.read_only,
                required: f.required,
                multiline: f.multiline,
                password: false,
                annot_index: i,
                on_state: w.state.clone(),
            });
        }
    }
    out
}

/// The document's annotations, page by page in the order each page lists
/// them — but not its form fields, links or pop-ups, and with each reply
/// kept on the comment it answers.
pub fn annotations(doc: &Document) -> Vec<Annotation> {
    let mut out: Vec<Annotation> = Vec::new();
    let mut replies: Vec<(ObjectId, ObjectId, Reply)> = Vec::new();
    for (number, page) in doc.get_pages() {
        for id in annots_of(doc, page) {
            let Ok(dict) = doc.get_object(id).and_then(Object::as_dict) else { continue };
            let subtype = name(dict, b"Subtype");
            if matches!(subtype.as_str(), "Widget" | "Link" | "Popup") {
                continue;
            }
            let created = date(&string(doc, dict, b"CreationDate"));
            if let Ok(to) = dict.get(b"IRT").and_then(Object::as_reference) {
                let reply = Reply { author: string(doc, dict, b"T"), contents: string(doc, dict, b"Contents"), created };
                replies.push((to, id, reply));
                continue;
            }
            out.push(read(doc, dict, id, number as usize - 1, &subtype, created));
        }
    }
    // A reply to a reply belongs to the comment its thread began with.
    let root = |mut to: ObjectId| {
        for _ in 0..32 {
            match replies.iter().find(|(_, id, _)| *id == to) {
                Some((up, ..)) => to = *up,
                None => break,
            }
        }
        to
    };
    for (to, _, reply) in &replies {
        let to = root(*to);
        if let Some(a) = out.iter_mut().find(|a| a.id.0 == to.0 as u64) {
            a.replies.push(reply.clone());
        }
    }
    out
}

/// One annotation, from its dictionary.
fn read(doc: &Document, dict: &Dictionary, id: ObjectId, page: usize, subtype: &str, created: String) -> Annotation {
    let numbers = |key: &[u8]| -> Vec<f32> {
        match resolve(doc, dict.get(key).ok()) {
            Some(Object::Array(items)) => items.iter().filter_map(|o| o.as_float().ok()).collect(),
            _ => Vec::new(),
        }
    };
    let pairs = |v: &[f32]| v.chunks_exact(2).map(|p| Vec2::new(p[0], p[1])).collect::<Vec<_>>();
    let rect = match numbers(b"Rect").as_slice() {
        [x1, y1, x2, y2, ..] => Rect::new(Vec2::new(*x1, *y1), Vec2::new(*x2, *y2)).normalize(),
        _ => Rect::ZERO,
    };
    // Each quad is four corners: its box is theirs.
    let quads = || {
        numbers(b"QuadPoints")
            .chunks_exact(8)
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                Rect::new(
                    Vec2::new(xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min)),
                    Vec2::new(xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max)),
                )
            })
            .collect::<Vec<_>>()
    };
    let endings = || {
        let le: Vec<String> = match resolve(doc, dict.get(b"LE").ok()) {
            Some(Object::Array(items)) => {
                items.iter().map(|o| o.as_name().map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()).collect()
            }
            _ => Vec::new(),
        };
        (ending(le.first()), ending(le.get(1)))
    };
    let kind = match subtype {
        "Highlight" => AnnotKind::Highlight { quads: quads() },
        "Underline" => AnnotKind::Underline { quads: quads() },
        "StrikeOut" => AnnotKind::StrikeOut { quads: quads() },
        "Squiggly" => AnnotKind::Squiggly { quads: quads() },
        "Text" => AnnotKind::Note { icon: icon(&name(dict, b"Name")) },
        "FreeText" => AnnotKind::FreeText {
            text: string(doc, dict, b"Contents"),
            font_size: font_size(&string(doc, dict, b"DA")),
            font: "Helvetica".into(),
            align: match dict.get(b"Q").and_then(Object::as_i64) {
                Ok(1) => TextAlign::Center,
                Ok(2) => TextAlign::Right,
                _ => TextAlign::Left,
            },
            callout: None,
        },
        "Ink" => AnnotKind::Ink {
            strokes: match resolve(doc, dict.get(b"InkList").ok()) {
                Some(Object::Array(strokes)) => strokes
                    .iter()
                    .filter_map(|s| resolve(doc, Some(s)))
                    .filter_map(|s| s.as_array().ok())
                    .map(|s| InkStroke { points: pairs(&s.iter().filter_map(|o| o.as_float().ok()).collect::<Vec<_>>()) })
                    .filter(|s| !s.points.is_empty())
                    .collect(),
                _ => Vec::new(),
            },
        },
        "Line" => {
            let l = numbers(b"L");
            let (start_ending, end_ending) = endings();
            match l.as_slice() {
                [x1, y1, x2, y2, ..] => AnnotKind::Line {
                    start: Vec2::new(*x1, *y1),
                    end: Vec2::new(*x2, *y2),
                    start_ending,
                    end_ending,
                    measure: None,
                },
                _ => AnnotKind::Unsupported { subtype: subtype.into() },
            }
        }
        "Square" => AnnotKind::Square,
        "Circle" => AnnotKind::Circle,
        "Polygon" => AnnotKind::Polygon { points: pairs(&numbers(b"Vertices")) },
        "PolyLine" => {
            let (start_ending, end_ending) = endings();
            AnnotKind::Polyline { points: pairs(&numbers(b"Vertices")), start_ending, end_ending }
        }
        "Stamp" => AnnotKind::Stamp { name: name(dict, b"Name"), image: None },
        "Redact" => AnnotKind::Redact { overlay_text: None, fill: colour(doc, dict, b"IC").unwrap_or(Color::BLACK) },
        "FileAttachment" => AnnotKind::FileAttachment { filename: string(doc, dict, b"Contents") },
        other => AnnotKind::Unsupported { subtype: other.into() },
    };
    let opacity = dict.get(b"CA").and_then(Object::as_float).unwrap_or(1.0).clamp(0.0, 1.0);
    let width = match resolve(doc, dict.get(b"BS").ok()) {
        Some(Object::Dictionary(bs)) => bs.get(b"W").and_then(Object::as_float).ok(),
        _ => None,
    }
    .or_else(|| match resolve(doc, dict.get(b"Border").ok()) {
        Some(Object::Array(b)) => b.get(2).and_then(|w| w.as_float().ok()),
        _ => None,
    })
    .unwrap_or(1.0);
    let flags = dict.get(b"F").and_then(Object::as_i64).unwrap_or(0);
    let modified = date(&string(doc, dict, b"M"));
    Annotation {
        id: AnnotId(id.0 as u64),
        page,
        kind,
        rect,
        color: colour(doc, dict, b"C").unwrap_or(Color::YELLOW),
        interior_color: colour(doc, dict, b"IC"),
        border_width: width,
        border_style: BorderStyle::Solid,
        opacity,
        author: string(doc, dict, b"T"),
        contents: string(doc, dict, b"Contents"),
        subject: string(doc, dict, b"Subj"),
        created: if created.is_empty() { modified.clone() } else { created },
        modified,
        replies: Vec::new(),
        status: ReviewStatus::None,
        // The standard's Locked and Hidden flags.
        locked: flags & 128 != 0,
        hidden: flags & 2 != 0,
        dirty: false,
    }
}

/// A page's annotations, in the order it lists them.
fn annots_of(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    let Ok(dict) = doc.get_object(page).and_then(Object::as_dict) else { return Vec::new() };
    match resolve(doc, dict.get(b"Annots").ok()) {
        Some(Object::Array(items)) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => Vec::new(),
    }
}

/// An object, its reference followed.
fn resolve<'a>(doc: &'a Document, object: Option<&'a Object>) -> Option<&'a Object> {
    match object? {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

fn name(dict: &Dictionary, key: &[u8]) -> String {
    dict.get(key).and_then(Object::as_name).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()
}

/// A text string's words.
fn string(doc: &Document, dict: &Dictionary, key: &[u8]) -> String {
    match resolve(doc, dict.get(key).ok()) {
        Some(Object::String(bytes, _)) => read_text(bytes),
        _ => String::new(),
    }
}

/// A colour of none, one (grey), three (RGB) or four (CMYK) components.
fn colour(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<Color> {
    let Some(Object::Array(items)) = resolve(doc, dict.get(key).ok()) else { return None };
    let v: Vec<f32> = items.iter().filter_map(|o| o.as_float().ok()).collect();
    let byte = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    match v.as_slice() {
        [g] => Some(Color::rgb(byte(*g), byte(*g), byte(*g))),
        [r, g, b] => Some(Color::rgb(byte(*r), byte(*g), byte(*b))),
        [c, m, y, k] => {
            let f = |x: f32| byte((1.0 - x) * (1.0 - k));
            Some(Color::rgb(f(*c), f(*m), f(*y)))
        }
        _ => None,
    }
}

fn ending(name: Option<&String>) -> LineEnding {
    match name.map(String::as_str) {
        Some("OpenArrow") => LineEnding::OpenArrow,
        Some("ClosedArrow") => LineEnding::ClosedArrow,
        Some("Circle") => LineEnding::Circle,
        Some("Square") => LineEnding::Square,
        Some("Diamond") => LineEnding::Diamond,
        Some("Butt") => LineEnding::Butt,
        _ => LineEnding::None,
    }
}

fn icon(name: &str) -> NoteIcon {
    match name {
        "Note" => NoteIcon::Note,
        "Help" => NoteIcon::Help,
        "Key" => NoteIcon::Key,
        "NewParagraph" => NoteIcon::NewParagraph,
        "Paragraph" => NoteIcon::Paragraph,
        "Insert" => NoteIcon::Insert,
        _ => NoteIcon::Comment,
    }
}

/// A text box's size, from its `/DA`: `/Helv 11 Tf …`.
fn font_size(da: &str) -> f32 {
    let words: Vec<&str> = da.split_whitespace().collect();
    words
        .iter()
        .position(|w| *w == "Tf")
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| words[i].parse().ok())
        .unwrap_or(12.0)
}

/// A PDF date, `D:20261012143000Z`, as RFC 3339; empty for none.
pub fn date(pdf: &str) -> String {
    let digits: String = pdf.trim_start_matches("D:").chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 8 {
        return String::new();
    }
    let part = |a: usize, b: usize, or: &str| digits.get(a..b).unwrap_or(or).to_string();
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        part(0, 4, "1970"),
        part(4, 6, "01"),
        part(6, 8, "01"),
        part(8, 10, "00"),
        part(10, 12, "00"),
        part(12, 14, "00")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_edit::{Doc, Op, Opened};
    use std::sync::Arc;

    fn editable() -> Box<Doc> {
        match Doc::open(Arc::new(quark_edit::fixture::fixture(&["Some words to mark.", "Two"]))) {
            Opened::Editable(doc) => doc,
            _ => panic!("the fixture opens"),
        }
    }

    fn add(doc: &mut Doc, a: &Annotation) {
        let mark = mark_of(a).expect("a comment");
        doc.apply(Op::Comment {
            page: a.page as u32 + 1,
            mark,
            author: a.author.clone(),
            date: "D:20261027090000Z".into(),
            name: format!("quark-{}", a.id.0),
        })
        .unwrap();
    }

    #[test]
    fn every_kind_quark_makes_is_written_and_read_back_by_its_object_number() {
        let mut doc = editable();
        let r = |x1: f32, y1: f32, x2: f32, y2: f32| Rect::new(Vec2::new(x1, y1), Vec2::new(x2, y2));
        let mut made = vec![
            Annotation::new(AnnotId(1), 0, AnnotKind::Highlight { quads: vec![r(72.0, 716.0, 180.0, 732.0)] }, r(72.0, 716.0, 180.0, 732.0), "Ada"),
            Annotation::new(AnnotId(2), 0, AnnotKind::Squiggly { quads: vec![r(72.0, 716.0, 180.0, 732.0)] }, r(72.0, 716.0, 180.0, 732.0), "Ada"),
            Annotation::new(AnnotId(3), 0, AnnotKind::Note { icon: NoteIcon::Comment }, r(300.0, 500.0, 324.0, 524.0), "Ada"),
            Annotation::new(
                AnnotId(4),
                0,
                AnnotKind::Line { start: Vec2::new(10.0, 10.0), end: Vec2::new(90.0, 60.0), start_ending: LineEnding::None, end_ending: LineEnding::OpenArrow, measure: None },
                r(10.0, 10.0, 90.0, 60.0),
                "Ada",
            ),
            Annotation::new(AnnotId(5), 1, AnnotKind::Polygon { points: vec![Vec2::new(100.0, 100.0), Vec2::new(150.0, 160.0), Vec2::new(200.0, 100.0)] }, r(100.0, 100.0, 200.0, 160.0), "Ada"),
            Annotation::new(AnnotId(6), 1, AnnotKind::Ink { strokes: vec![InkStroke { points: vec![Vec2::new(50.0, 50.0), Vec2::new(60.0, 70.0)] }] }, r(50.0, 50.0, 60.0, 70.0), "Ada"),
        ];
        made[2].contents = "Look here".into();
        made[4].interior_color = Some(Color::GREEN);
        made[4].border_width = 3.0;
        for a in &made {
            add(&mut doc, a);
        }
        let read = annotations(doc.document());
        let kinds: Vec<&str> = read.iter().map(|a| a.kind.subtype_name()).collect();
        assert_eq!(kinds, ["Highlight", "Squiggly", "Text", "Line", "Polygon", "Ink"], "{read:#?}");
        // Named by object, so two never share a name.
        let mut ids: Vec<u64> = read.iter().map(|a| a.id.0).collect();
        ids.dedup();
        assert_eq!(ids.len(), 6);
        assert!(read.iter().all(|a| a.id.0 > 6), "ids are object numbers, after the fixture's own objects: {ids:?}");
        assert_eq!(read[2].contents, "Look here");
        assert!(matches!(&read[3].kind, AnnotKind::Line { end_ending: LineEnding::OpenArrow, end, .. } if (end.x - 90.0).abs() < 0.01));
        assert_eq!(read[4].interior_color, Some(Color::GREEN));
        assert_eq!(read[4].border_width, 3.0);
        assert_eq!(read[4].page, 1);
        assert!(matches!(&read[5].kind, AnnotKind::Ink { strokes } if strokes[0].points.len() == 2));
        assert_eq!(read[0].author, "Ada");
        assert_eq!(read[0].created, "2026-10-27T09:00:00Z");
        assert!((read[0].opacity - 0.4).abs() < 0.01);

        // A reply is kept on what it answers; deleting by the id read deletes it.
        doc.apply(Op::Reply { to: comment_id(read[2].id), text: "Agreed".into(), author: "Bo".into(), date: "D:2026".into(), name: "r".into() }).unwrap();
        let read = annotations(doc.document());
        assert_eq!(read.len(), 6);
        assert_eq!(read[2].replies.len(), 1);
        assert_eq!(read[2].replies[0].contents, "Agreed");
        doc.apply(Op::DeleteComment { id: comment_id(read[0].id) }).unwrap();
        assert_eq!(annotations(doc.document()).len(), 5);
    }

    #[test]
    fn redaction_marks_and_stamps_are_not_comments_and_dates_are_read() {
        let a = Annotation::new(AnnotId(1), 0, AnnotKind::Redact { overlay_text: None, fill: Color::BLACK }, Rect::ZERO, "Ada");
        assert!(mark_of(&a).is_none());
        assert_eq!(date("D:20261012143000+02'00'"), "2026-10-12T14:30:00Z");
        assert_eq!(date("D:2026"), "");
        assert_eq!(font_size("/Helv 9 Tf 0 g"), 9.0);
        assert_eq!(hex(Color::rgb(255, 212, 0)), "#ffd400");
    }

    #[test]
    fn a_forms_fields_are_a_widget_each_with_each_buttons_own_on() {
        let mut lo = lopdf::Document::load_mem(&quark_edit::fixture::fixture(&["A form"])).unwrap();
        quark_edit::forms::sample(&mut lo);
        let mut bytes = Vec::new();
        lo.save_to(&mut bytes).unwrap();
        let mut doc = match quark_edit::Doc::open(std::sync::Arc::new(bytes)) {
            Opened::Editable(doc) => doc,
            _ => panic!("the form opens"),
        };
        let read = fields(&doc);
        let size: Vec<&FormField> = read.iter().filter(|f| f.name == "size").collect();
        assert_eq!(size.len(), 2, "two radio buttons, a widget each: {read:#?}");
        assert!(size.iter().all(|f| f.kind == FieldKind::RadioButton && f.page == 0));
        let states: Vec<Option<&str>> = size.iter().map(|f| f.on_state.as_deref()).collect();
        assert!(states.contains(&Some("small")), "{states:?}");
        let agree = read.iter().find(|f| f.name == "agree").unwrap();
        assert_eq!((agree.kind, agree.on_state.as_deref(), agree.value.as_str()), (FieldKind::Checkbox, Some("Yes"), "Off"));
        assert!(read.iter().any(|f| f.name == "notes" && f.multiline));
        // Ticked by its own on, and read back so.
        doc.apply(Op::SetField { name: "size".into(), value: "small".into() }).unwrap();
        doc.apply(Op::SetField { name: "agree".into(), value: "Yes".into() }).unwrap();
        let read = fields(&doc);
        assert!(read.iter().filter(|f| f.name == "size").all(|f| f.value == "small"));
        assert_eq!(read.iter().find(|f| f.name == "agree").unwrap().value, "Yes");
    }

}
