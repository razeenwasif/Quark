//! Reading and writing annotations.
//!
//! Quark's own [`Annotation`] model lives in `quark-core` and knows nothing
//! about PDFium; this module is the bridge. Reading maps PDFium's annotation
//! objects onto that model; writing goes the other way.
//!
//! PDFium can *create* only a subset of the annotation subtypes it can *read* —
//! there is no `create_circle_annotation`, for instance. Rather than silently
//! dropping the shapes it cannot make, [`unsupported_by_pdfium`] names them so
//! the caller can route them through the lopdf writer in
//! [`crate::organize`] instead.

use lopdf::{Dictionary, Object};
use pdfium_render::prelude::*;
use quark_core::annot::{
    AnnotId, AnnotKind, Annotation, BorderStyle, Color, InkStroke, LineEnding, NoteIcon,
    ReviewStatus, TextAlign,
};
use quark_core::geom::{Rect, Vec2};

use crate::doc::Document;
use crate::engine::EngineError;

/// Converts a PDFium colour to Quark's.
fn from_pdf_color(c: PdfColor) -> Color {
    Color::rgba(c.red(), c.green(), c.blue(), c.alpha())
}

fn rect_from_pdf(r: PdfRect) -> Rect {
    Rect::new(
        Vec2::new(r.left().value, r.bottom().value),
        Vec2::new(r.right().value, r.top().value),
    )
    .normalize()
}

/// Annotation subtypes PDFium can read but not create.
///
/// These are written by the lopdf path instead; see
/// [`crate::organize::append_annotations`].
pub fn unsupported_by_pdfium(kind: &AnnotKind) -> bool {
    matches!(
        kind,
        AnnotKind::Circle
            | AnnotKind::Line { .. }
            | AnnotKind::Polygon { .. }
            | AnnotKind::Polyline { .. }
            | AnnotKind::Redact { .. }
            | AnnotKind::FileAttachment { .. }
    )
}

/// Reads every annotation on a page.
pub fn read_page(doc: &Document, page_index: usize, next_id: &mut u64) -> Result<Vec<Annotation>, EngineError> {
    let pages = doc.inner().pages();
    let page = pages
        .get(page_index as i32)
        .map_err(|_| EngineError::Pdfium(format!("no page {page_index}")))?;

    let mut out = Vec::new();
    for a in page.annotations().iter() {
        // A widget is a form field, not a comment. It is handled by
        // `crate::forms` and would otherwise show up in the comments panel.
        if a.annotation_type() == PdfPageAnnotationType::Widget {
            continue;
        }

        let bounds = a.bounds().map(rect_from_pdf).unwrap_or(Rect::ZERO);
        let quads = read_quads(&a);
        let kind = match a.annotation_type() {
            PdfPageAnnotationType::Highlight => AnnotKind::Highlight { quads },
            PdfPageAnnotationType::Underline => AnnotKind::Underline { quads },
            PdfPageAnnotationType::Strikeout => AnnotKind::StrikeOut { quads },
            PdfPageAnnotationType::Squiggly => AnnotKind::Squiggly { quads },
            PdfPageAnnotationType::Text => AnnotKind::Note {
                icon: NoteIcon::Comment,
            },
            PdfPageAnnotationType::FreeText => AnnotKind::FreeText {
                text: a.contents().unwrap_or_default(),
                font_size: 12.0,
                font: "Helvetica".into(),
                align: TextAlign::Left,
                callout: None,
            },
            PdfPageAnnotationType::Ink => AnnotKind::Ink {
                strokes: read_ink(&a),
            },
            PdfPageAnnotationType::Square => AnnotKind::Square,
            PdfPageAnnotationType::Circle => AnnotKind::Circle,
            PdfPageAnnotationType::Stamp => AnnotKind::Stamp {
                name: a.name().unwrap_or_else(|| "Stamp".into()),
                image: None,
            },
            PdfPageAnnotationType::Redacted => AnnotKind::Redact {
                overlay_text: None,
                fill: Color::BLACK,
            },
            other => AnnotKind::Unsupported {
                subtype: format!("{other:?}"),
            },
        };

        let id = AnnotId(*next_id);
        *next_id += 1;

        let stroke = a.stroke_color().map(from_pdf_color).unwrap_or(Color::RED);
        let fill = a.fill_color().ok().map(from_pdf_color);

        out.push(Annotation {
            id,
            page: page_index,
            // Text markup carries its real geometry in the quads; the stored
            // /Rect is only a bounding box and is sometimes absent or stale.
            rect: bounds,
            color: stroke,
            interior_color: fill,
            border_width: 2.0,
            border_style: BorderStyle::Solid,
            // PDF stores opacity in /CA, which PDFium folds into the colour's
            // alpha channel rather than exposing separately.
            opacity: stroke.a as f32 / 255.0,
            author: a.creator().unwrap_or_default(),
            contents: a.contents().unwrap_or_default(),
            subject: String::new(),
            created: a.creation_date().unwrap_or_default(),
            modified: a.modification_date().unwrap_or_default(),
            replies: Vec::new(),
            status: ReviewStatus::None,
            locked: false,
            hidden: a.is_hidden(),
            // Straight from the file, so nothing to write back yet.
            dirty: false,
            kind,
        });
    }
    Ok(out)
}

/// Reads every annotation in the document.
pub fn read_all(doc: &Document) -> Result<Vec<Annotation>, EngineError> {
    let mut id = 1u64;
    let mut out = Vec::new();
    for p in 0..doc.page_count() {
        out.extend(read_page(doc, p, &mut id)?);
    }
    Ok(out)
}

fn read_quads(a: &PdfPageAnnotation) -> Vec<Rect> {
    if !a.has_attachment_points() {
        return Vec::new();
    }
    a.attachment_points()
        .iter()
        .map(|q| {
            Rect::new(
                Vec2::new(q.left().value, q.bottom().value),
                Vec2::new(q.right().value, q.top().value),
            )
            .normalize()
        })
        .collect()
}

/// Recovers ink strokes from the annotation's path objects.
fn read_ink(a: &PdfPageAnnotation) -> Vec<InkStroke> {
    let Some(ink) = a.as_ink_annotation() else {
        return Vec::new();
    };
    let mut strokes = Vec::new();
    for obj in ink.objects().iter() {
        let Some(path) = obj.as_path_object() else {
            continue;
        };
        let mut pts = Vec::new();
        for seg in path.segments().iter() {
            pts.push(Vec2::new(seg.x().value, seg.y().value));
        }
        if pts.len() > 1 {
            strokes.push(InkStroke { points: pts });
        }
    }
    strokes
}

/// Builds the PDF annotation dictionary for one annotation.
///
/// # Why writing does not go through PDFium
///
/// PDFium can create only a handful of the subtypes it can read — there is no
/// `create_circle_annotation`, no line, no polygon, no redaction — and its ink
/// support works by adding stroked path objects rather than writing the
/// `/InkList` array the format actually specifies. Maintaining a PDFium path
/// for the easy half and a lopdf path for the rest would mean two writers that
/// drift apart.
///
/// So every annotation is written here, as a plain dictionary, and applied to
/// the file by [`crate::organize::apply_annotations`]. PDFium then generates
/// the appearance streams when it next opens the file, which is exactly what it
/// does for annotations written by any other producer.
pub fn to_dict(ann: &Annotation) -> Dictionary {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(ann.kind.subtype_name().as_bytes().to_vec()));
    d.set("Rect", rect_to_array(ann.rect));

    if !ann.contents.is_empty() {
        d.set("Contents", text_string(&ann.contents));
    }
    if !ann.author.is_empty() {
        d.set("T", text_string(&ann.author));
    }
    if !ann.subject.is_empty() {
        d.set("Subj", text_string(&ann.subject));
    }
    if !ann.created.is_empty() {
        d.set("CreationDate", Object::string_literal(to_pdf_date(&ann.created)));
    }
    if !ann.modified.is_empty() {
        d.set("M", Object::string_literal(to_pdf_date(&ann.modified)));
    }

    // /C is the annotation's own colour and is always three components in
    // DeviceRGB. Opacity lives in /CA, never in the colour array.
    d.set("C", color_array(ann.color));
    if let Some(ic) = ann.interior_color {
        d.set("IC", color_array(ic));
    }
    if ann.opacity < 1.0 {
        d.set("CA", Object::Real(ann.opacity as f32));
    }

    // Border: /BS is the modern form and takes precedence over /Border.
    let mut bs = Dictionary::new();
    bs.set("W", Object::Real(ann.border_width));
    bs.set("S", Object::Name(match ann.border_style {
        BorderStyle::Solid => b"S".to_vec(),
        BorderStyle::Dashed => b"D".to_vec(),
        BorderStyle::Dotted => b"D".to_vec(),
    }));
    if ann.border_style == BorderStyle::Dotted {
        bs.set("D", Object::Array(vec![Object::Integer(1), Object::Integer(2)]));
    } else if ann.border_style == BorderStyle::Dashed {
        bs.set("D", Object::Array(vec![Object::Integer(4), Object::Integer(2)]));
    }
    d.set("BS", Object::Dictionary(bs));

    // Flags: bit 3 (value 4) is Print, which is what makes an annotation
    // appear on paper. Acrobat sets it on every markup annotation, and one
    // that lacks it silently vanishes when the document is printed.
    let mut flags = 4;
    if ann.hidden {
        flags |= 2;
    }
    if ann.locked {
        flags |= 128;
    }
    d.set("F", Object::Integer(flags));

    match &ann.kind {
        AnnotKind::Highlight { quads }
        | AnnotKind::Underline { quads }
        | AnnotKind::StrikeOut { quads }
        | AnnotKind::Squiggly { quads } => {
            d.set("QuadPoints", quad_points_array(quads));
        }
        AnnotKind::Note { icon } => {
            d.set("Name", Object::Name(note_icon_name(*icon).as_bytes().to_vec()));
            d.set("Open", Object::Boolean(false));
        }
        AnnotKind::FreeText {
            text,
            font_size,
            font,
            align,
            callout,
        } => {
            d.set("Contents", text_string(text));
            // /DA is a content-stream fragment giving the font and colour the
            // text is drawn with. Without it a reader has nothing to lay out.
            let c = ann.color.to_f32();
            d.set(
                "DA",
                Object::string_literal(format!(
                    "/{} {} Tf {:.3} {:.3} {:.3} rg",
                    font.replace(' ', ""),
                    font_size,
                    c[0],
                    c[1],
                    c[2]
                )),
            );
            d.set("Q", Object::Integer(match align {
                TextAlign::Left => 0,
                TextAlign::Center => 1,
                TextAlign::Right => 2,
            }));
            if let Some(cl) = callout {
                d.set("IT", Object::Name(b"FreeTextCallout".to_vec()));
                d.set("CL", Object::Array(
                    cl.iter()
                        .flat_map(|p| [Object::Real(p.x), Object::Real(p.y)])
                        .collect(),
                ));
                d.set("LE", Object::Name(b"OpenArrow".to_vec()));
            }
        }
        AnnotKind::Ink { strokes } => {
            // /InkList is an array of arrays: one flat x y x y ... run per
            // stroke. This is the representation the format specifies, and is
            // why ink is written here rather than as PDFium path objects.
            d.set("InkList", Object::Array(
                strokes
                    .iter()
                    .filter(|s| s.points.len() > 1)
                    .map(|s| {
                        Object::Array(
                            s.points
                                .iter()
                                .flat_map(|p| [Object::Real(p.x), Object::Real(p.y)])
                                .collect(),
                        )
                    })
                    .collect(),
            ));
        }
        AnnotKind::Line {
            start,
            end,
            start_ending,
            end_ending,
            measure,
        } => {
            d.set("L", Object::Array(vec![
                Object::Real(start.x),
                Object::Real(start.y),
                Object::Real(end.x),
                Object::Real(end.y),
            ]));
            d.set("LE", Object::Array(vec![
                Object::Name(line_ending_name(*start_ending).as_bytes().to_vec()),
                Object::Name(line_ending_name(*end_ending).as_bytes().to_vec()),
            ]));
            if let Some(m) = measure {
                let len = (*end - *start).length();
                d.set("Contents", text_string(&m.format(len)));
                d.set("IT", Object::Name(b"LineDimension".to_vec()));
            }
        }
        AnnotKind::Polygon { points } | AnnotKind::Polyline { points, .. } => {
            d.set("Vertices", Object::Array(
                points
                    .iter()
                    .flat_map(|p| [Object::Real(p.x), Object::Real(p.y)])
                    .collect(),
            ));
            if let AnnotKind::Polyline {
                start_ending,
                end_ending,
                ..
            } = &ann.kind
            {
                d.set("LE", Object::Array(vec![
                    Object::Name(line_ending_name(*start_ending).as_bytes().to_vec()),
                    Object::Name(line_ending_name(*end_ending).as_bytes().to_vec()),
                ]));
            }
        }
        AnnotKind::Stamp { name, .. } => {
            d.set("Name", Object::Name(name.as_bytes().to_vec()));
        }
        AnnotKind::Redact { overlay_text, fill } => {
            // /IC is the colour the area is painted with once the redaction is
            // applied; /OverlayText is drawn on top of it.
            d.set("IC", color_array(*fill));
            if let Some(t) = overlay_text {
                d.set("OverlayText", text_string(t));
                d.set("Repeat", Object::Boolean(false));
                d.set("DA", Object::string_literal("/Helv 10 Tf 1 1 1 rg"));
            }
            d.set("QuadPoints", quad_points_array(&[ann.rect]));
        }
        AnnotKind::FileAttachment { filename } => {
            d.set("Name", Object::Name(b"PushPin".to_vec()));
            d.set("Contents", text_string(filename));
        }
        AnnotKind::Square | AnnotKind::Circle | AnnotKind::Unsupported { .. } => {}
    }

    d
}

/// A PDF array of four numbers for a rectangle, in PDF's own corner order.
fn rect_to_array(r: Rect) -> Object {
    let r = r.normalize();
    Object::Array(vec![
        Object::Real(r.min.x),
        Object::Real(r.min.y),
        Object::Real(r.max.x),
        Object::Real(r.max.y),
    ])
}

/// `/QuadPoints` for text markup.
///
/// The corner order is the one thing that is easy to get wrong here: the
/// specification lists the corners as upper-left, upper-right, lower-left,
/// lower-right — *not* in a consistent winding order. Readers that follow the
/// spec literally render a bow-tie if the corners are given clockwise.
fn quad_points_array(quads: &[Rect]) -> Object {
    let mut v = Vec::with_capacity(quads.len() * 8);
    for q in quads {
        let q = q.normalize();
        for (x, y) in [
            (q.min.x, q.max.y),
            (q.max.x, q.max.y),
            (q.min.x, q.min.y),
            (q.max.x, q.min.y),
        ] {
            v.push(Object::Real(x));
            v.push(Object::Real(y));
        }
    }
    Object::Array(v)
}

fn color_array(c: Color) -> Object {
    let f = c.to_f32();
    Object::Array(vec![
        Object::Real(f[0]),
        Object::Real(f[1]),
        Object::Real(f[2]),
    ])
}

/// A PDF text string.
///
/// Anything outside ASCII is written as UTF-16BE with a byte-order mark, which
/// is how PDF signals a non-PDFDocEncoding string. Writing raw UTF-8 instead
/// produces mojibake in every conforming reader.
fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        return Object::string_literal(s);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for u in s.encode_utf16() {
        bytes.extend_from_slice(&u.to_be_bytes());
    }
    Object::String(bytes, lopdf::StringFormat::Hexadecimal)
}

/// Converts an RFC 3339 timestamp to a PDF date string (`D:YYYYMMDDHHmmSSZ`).
fn to_pdf_date(rfc3339: &str) -> String {
    let digits: String = rfc3339.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 14 {
        format!("D:{}Z", &digits[..14])
    } else {
        // Better an absent-looking date than a malformed one that makes a
        // reader reject the whole dictionary.
        format!("D:{}", digits)
    }
}

fn note_icon_name(i: NoteIcon) -> &'static str {
    match i {
        NoteIcon::Comment => "Comment",
        NoteIcon::Note => "Note",
        NoteIcon::Help => "Help",
        NoteIcon::Key => "Key",
        NoteIcon::NewParagraph => "NewParagraph",
        NoteIcon::Paragraph => "Paragraph",
        NoteIcon::Insert => "Insert",
    }
}

fn line_ending_name(e: LineEnding) -> &'static str {
    match e {
        LineEnding::None => "None",
        LineEnding::OpenArrow => "OpenArrow",
        LineEnding::ClosedArrow => "ClosedArrow",
        LineEnding::Circle => "Circle",
        LineEnding::Square => "Square",
        LineEnding::Diamond => "Diamond",
        LineEnding::Butt => "Butt",
        LineEnding::Slash => "Slash",
    }
}

/// Removes an annotation by its index on the page.
pub fn delete_annotation(doc: &mut Document, page: usize, index: usize) -> Result<(), EngineError> {
    let pages = doc.inner_mut().pages_mut();
    let mut p = pages
        .get(page as i32)
        .map_err(|_| EngineError::Pdfium(format!("no page {page}")))?;
    let annotations = p.annotations_mut();
    let a = annotations
        .get(index)
        .map_err(|_| EngineError::Pdfium(format!("no annotation {index}")))?;
    annotations.delete_annotation(a).map_err(EngineError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organize;
    use crate::testutil;
    use quark_core::annot::{InkStroke, Measure, MeasureUnit};

    /// Applies annotations and reads them back through PDFium, which is the
    /// only check that actually proves a reader will accept what we wrote.
    fn round_trip(anns: &[Annotation]) -> Vec<Annotation> {
        let out = organize::apply_annotations(&testutil::sample_bytes(), anns).unwrap();
        let d = Document::from_bytes(out, None).expect("PDFium rejected our annotations");
        read_all(&d).unwrap()
    }

    fn ann(kind: AnnotKind, rect: Rect) -> Annotation {
        Annotation::new(AnnotId(1), 0, kind, rect, "tester")
    }

    #[test]
    fn a_clean_document_has_no_annotations() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(read_all(&d).unwrap().is_empty());
    }

    #[test]
    fn shapes_pdfium_cannot_create_are_named_as_such() {
        assert!(unsupported_by_pdfium(&AnnotKind::Circle));
        assert!(unsupported_by_pdfium(&AnnotKind::Polygon { points: vec![] }));
        assert!(!unsupported_by_pdfium(&AnnotKind::Square));
        assert!(!unsupported_by_pdfium(&AnnotKind::Highlight { quads: vec![] }));
    }

    #[test]
    fn the_subtype_name_is_written_into_the_dictionary() {
        let d = to_dict(&ann(AnnotKind::Circle, Rect::from_xywh(0.0, 0.0, 10.0, 10.0)));
        assert_eq!(
            d.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Circle"
        );
    }

    #[test]
    fn every_annotation_is_flagged_to_print() {
        // Without the Print flag a comment is invisible on paper, which is a
        // silent and very confusing failure.
        let d = to_dict(&ann(AnnotKind::Square, Rect::ZERO));
        let f = d.get(b"F").unwrap().as_i64().unwrap();
        assert_eq!(f & 4, 4, "Print flag missing");
    }

    #[test]
    fn hidden_and_locked_set_their_flag_bits() {
        let mut a = ann(AnnotKind::Square, Rect::ZERO);
        a.hidden = true;
        a.locked = true;
        let f = to_dict(&a).get(b"F").unwrap().as_i64().unwrap();
        assert_eq!(f & 2, 2, "Hidden bit");
        assert_eq!(f & 128, 128, "Locked bit");
    }

    #[test]
    fn opacity_is_written_to_ca_and_never_into_the_colour() {
        let mut a = ann(AnnotKind::Highlight { quads: vec![] }, Rect::ZERO);
        a.opacity = 0.4;
        let d = to_dict(&a);
        let ca = d.get(b"CA").unwrap().as_float().unwrap();
        assert!((ca - 0.4).abs() < 1e-4);
        // /C is three components: RGB only, no alpha.
        assert_eq!(d.get(b"C").unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn a_fully_opaque_annotation_omits_ca() {
        let d = to_dict(&ann(AnnotKind::Square, Rect::ZERO));
        assert!(d.get(b"CA").is_err(), "/CA should be absent at full opacity");
    }

    #[test]
    fn quad_points_use_the_corner_order_the_specification_asks_for() {
        // Upper-left, upper-right, lower-left, lower-right. Emitting these in
        // winding order instead renders text markup as a bow-tie.
        let r = Rect::from_xywh(10.0, 20.0, 100.0, 12.0);
        let d = to_dict(&ann(AnnotKind::Highlight { quads: vec![r] }, r));
        let q = d.get(b"QuadPoints").unwrap().as_array().unwrap();
        assert_eq!(q.len(), 8);
        let v: Vec<f32> = q.iter().map(|o| o.as_float().unwrap()).collect();
        assert_eq!(&v[0..2], &[10.0, 32.0], "corner 1 should be upper-left");
        assert_eq!(&v[2..4], &[110.0, 32.0], "corner 2 should be upper-right");
        assert_eq!(&v[4..6], &[10.0, 20.0], "corner 3 should be lower-left");
        assert_eq!(&v[6..8], &[110.0, 20.0], "corner 4 should be lower-right");
    }

    #[test]
    fn non_ascii_comments_are_written_as_utf16() {
        // A raw UTF-8 byte string shows up as mojibake in a conforming reader.
        let mut a = ann(AnnotKind::Square, Rect::ZERO);
        a.contents = "réunion — 日本語".into();
        let d = to_dict(&a);
        match d.get(b"Contents").unwrap() {
            Object::String(bytes, _) => {
                assert_eq!(&bytes[0..2], &[0xFE, 0xFF], "missing UTF-16 byte order mark");
            }
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn ascii_comments_stay_as_plain_literals() {
        let mut a = ann(AnnotKind::Square, Rect::ZERO);
        a.contents = "plain ascii".into();
        match to_dict(&a).get(b"Contents").unwrap() {
            Object::String(bytes, _) => assert_eq!(bytes, b"plain ascii"),
            other => panic!("expected a literal, got {other:?}"),
        }
    }

    #[test]
    fn timestamps_are_converted_to_pdf_date_syntax() {
        assert_eq!(to_pdf_date("2026-08-21T13:45:07Z"), "D:20260821134507Z");
    }

    #[test]
    fn a_line_records_its_endpoints_and_endings() {
        let a = ann(
            AnnotKind::Line {
                start: Vec2::new(10.0, 20.0),
                end: Vec2::new(110.0, 220.0),
                start_ending: LineEnding::None,
                end_ending: LineEnding::OpenArrow,
                measure: None,
            },
            Rect::from_xywh(10.0, 20.0, 100.0, 200.0),
        );
        let d = to_dict(&a);
        let l: Vec<f32> = d
            .get(b"L")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_float().unwrap())
            .collect();
        assert_eq!(l, vec![10.0, 20.0, 110.0, 220.0]);
        let le = d.get(b"LE").unwrap().as_array().unwrap();
        assert_eq!(le[1].as_name().unwrap(), b"OpenArrow");
    }

    #[test]
    fn a_measured_line_states_its_length_as_its_comment() {
        let a = ann(
            AnnotKind::Line {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(144.0, 0.0),
                start_ending: LineEnding::None,
                end_ending: LineEnding::None,
                measure: Some(Measure {
                    unit: MeasureUnit::Inches,
                    scale: 1.0,
                }),
            },
            Rect::from_xywh(0.0, 0.0, 144.0, 1.0),
        );
        let d = to_dict(&a);
        match d.get(b"Contents").unwrap() {
            Object::String(b, _) => assert_eq!(String::from_utf8_lossy(b), "2.00 in"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn free_text_carries_a_default_appearance_string() {
        // Without /DA a reader has no font to lay the text out with.
        let a = ann(
            AnnotKind::FreeText {
                text: "note".into(),
                font_size: 14.0,
                font: "Helvetica".into(),
                align: TextAlign::Center,
                callout: None,
            },
            Rect::from_xywh(0.0, 0.0, 100.0, 40.0),
        );
        let d = to_dict(&a);
        let da = match d.get(b"DA").unwrap() {
            Object::String(b, _) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("{other:?}"),
        };
        assert!(da.contains("14"), "font size missing from /DA: {da}");
        assert!(da.contains("Tf"), "no font operator in /DA: {da}");
        assert_eq!(d.get(b"Q").unwrap().as_i64().unwrap(), 1, "centre alignment");
    }

    #[test]
    fn ink_is_written_as_one_array_per_stroke() {
        let strokes = vec![
            InkStroke {
                points: vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)],
            },
            InkStroke {
                points: vec![Vec2::new(20.0, 0.0), Vec2::new(30.0, 10.0)],
            },
            // A single-point stroke is not a line and must be dropped.
            InkStroke {
                points: vec![Vec2::new(50.0, 50.0)],
            },
        ];
        let d = to_dict(&ann(AnnotKind::Ink { strokes }, Rect::ZERO));
        let list = d.get(b"InkList").unwrap().as_array().unwrap();
        assert_eq!(list.len(), 2, "degenerate stroke was not dropped");
        assert_eq!(list[0].as_array().unwrap().len(), 4, "x y x y");
    }

    #[test]
    fn a_polygon_records_its_vertices() {
        let pts = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(50.0, 0.0),
            Vec2::new(25.0, 40.0),
        ];
        let d = to_dict(&ann(AnnotKind::Polygon { points: pts }, Rect::ZERO));
        assert_eq!(d.get(b"Vertices").unwrap().as_array().unwrap().len(), 6);
    }

    #[test]
    fn a_redaction_records_its_fill_and_overlay() {
        let a = ann(
            AnnotKind::Redact {
                overlay_text: Some("REDACTED".into()),
                fill: Color::BLACK,
            },
            Rect::from_xywh(10.0, 10.0, 100.0, 20.0),
        );
        let d = to_dict(&a);
        assert!(d.get(b"IC").is_ok(), "no interior colour");
        assert!(d.get(b"OverlayText").is_ok(), "no overlay text");
        assert!(d.get(b"QuadPoints").is_ok(), "redaction needs quad points");
    }

    #[test]
    fn a_highlight_survives_a_round_trip_through_a_real_file() {
        let _g = testutil::pdfium_guard();
        let quad = Rect::from_xywh(72.0, 690.0, 200.0, 24.0);
        let mut a = ann(AnnotKind::Highlight { quads: vec![quad] }, quad);
        a.contents = "important".into();
        let read = round_trip(&[a]);
        assert_eq!(read.len(), 1);
        assert!(matches!(read[0].kind, AnnotKind::Highlight { .. }));
        assert_eq!(read[0].contents, "important");
        assert_eq!(read[0].author, "tester");
    }

    #[test]
    fn a_highlights_quads_come_back_where_they_were_put() {
        let _g = testutil::pdfium_guard();
        let quad = Rect::from_xywh(72.0, 690.0, 200.0, 24.0);
        let read = round_trip(&[ann(AnnotKind::Highlight { quads: vec![quad] }, quad)]);
        let quads = read[0].kind.quads().expect("quads");
        assert_eq!(quads.len(), 1);
        assert!((quads[0].min.x - 72.0).abs() < 1.0, "{:?}", quads[0]);
        assert!((quads[0].width() - 200.0).abs() < 1.0, "{:?}", quads[0]);
    }

    #[test]
    fn annotations_read_back_carry_the_page_they_are_on() {
        let _g = testutil::pdfium_guard();
        let anns: Vec<Annotation> = (0..2)
            .map(|p| {
                Annotation::new(
                    AnnotId(p as u64),
                    p,
                    AnnotKind::Square,
                    Rect::from_xywh(20.0, 20.0, 60.0, 60.0),
                    "t",
                )
            })
            .collect();
        let read = round_trip(&anns);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].page, 0);
        assert_eq!(read[1].page, 1);
    }

    #[test]
    fn form_widgets_are_not_reported_as_comments() {
        // Widgets belong to the forms panel; leaking them here would fill the
        // comments list with every text box in a form.
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(read_all(&d).unwrap().is_empty());
    }
}
