//! Comments: highlights, underlines, squiggles and strike-outs over text,
//! sticky notes, text boxes, ink, rectangles, ellipses, polygons, polylines
//! and lines — each written as
//! the PDF standard has it, with an appearance of its own (`/AP`), so every
//! viewer draws it as Quark does; listed, with their replies, recoloured,
//! edited and deleted.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use serde::{Deserialize, Serialize};

use crate::pages;

/// The kinds a comment may be.
const KINDS: [&str; 12] = [
    "Highlight",
    "Underline",
    "StrikeOut",
    "Squiggly",
    "Text",
    "FreeText",
    "Ink",
    "Square",
    "Circle",
    "Line",
    "Polygon",
    "PolyLine",
];

/// What a new comment is: its kind and where, as the window says it — in
/// the page's own points, from its bottom left.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    /// `highlight`, `underline`, `strike`, `squiggly`, `note`, `text`,
    /// `ink`, `square`, `circle`, `line`, `arrow`, `polygon` or `polyline`.
    pub kind: String,
    /// Over text: a box per line, `[x1, y1, x2, y2]`.
    #[serde(default)]
    pub boxes: Vec<[f32; 4]>,
    /// A note's point, a text box's or shape's box, a line's two ends.
    #[serde(default)]
    pub rect: Option<[f32; 4]>,
    /// Ink: strokes, each a run of points. A polygon's or polyline's
    /// corners: the first stroke.
    #[serde(default)]
    pub strokes: Vec<Vec<[f32; 2]>>,
    /// `#ffd400`.
    pub color: String,
    #[serde(default)]
    pub text: String,
    /// How wide its lines are, in points: 2 when not said.
    #[serde(default)]
    pub width: Option<f32>,
    /// How opaque, 0 to 1: a highlight's 0.4, the rest 1, when not said.
    #[serde(default)]
    pub opacity: Option<f32>,
    /// A shape's fill, `#rrggbb`: none when not said.
    #[serde(default)]
    pub fill: Option<String>,
}

/// A comment as the window lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// The annotation's object: `12 0`.
    pub id: String,
    pub page: u32,
    /// The PDF's own name for its kind: `Highlight`, `Text`, `FreeText`.
    pub kind: String,
    /// Where, in the page's points from its bottom left.
    pub rect: [f32; 4],
    pub color: Option<String>,
    pub text: String,
    pub author: String,
    /// As the PDF has it: `D:20261012143000Z`.
    pub date: String,
    /// The comment this answers.
    pub reply_to: Option<String>,
}

/// An annotation's object, from how a comment names it.
pub fn object_of(id: &str) -> Result<ObjectId, String> {
    let mut parts = id.split_whitespace();
    let n = parts
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("{id}: not a comment"))?;
    let g = parts.next().map(|g| g.parse().unwrap_or(0)).unwrap_or(0);
    Ok((n, g))
}

/// Text as a PDF text string: ASCII as it is, anything else as UTF-16 with
/// its mark.
pub fn text_string(text: &str) -> Object {
    if text.is_ascii() {
        Object::String(text.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend(unit.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// A PDF text string read: UTF-16 with its mark, or else as Latin-1 —
/// near enough PDFDocEncoding for what comments say.
pub fn read_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).to_string();
    }
    bytes.iter().map(|b| *b as char).collect()
}

/// `#ffd400` as `[1.0, 0.83, 0.0]`.
fn rgb(color: &str) -> [f32; 3] {
    let hex = color.trim_start_matches('#');
    let part = |i: usize| {
        u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0
    };
    if hex.len() == 6 {
        [part(0), part(2), part(4)]
    } else {
        [1.0, 0.83, 0.0]
    }
}

/// `[1.0, 0.83, 0.0]` as `#ffd400`.
fn hex_of(c: &[f32]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        b(*c.first().unwrap_or(&0.0)),
        b(*c.get(1).unwrap_or(&0.0)),
        b(*c.get(2).unwrap_or(&0.0))
    )
}

/// A number written short, as content streams want it.
fn n(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The box round some points, widened by `pad`.
fn bounds(points: impl IntoIterator<Item = [f32; 2]>, pad: f32) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for [x, y] in points {
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    if b[0] > b[2] {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
}

/// A box with its corners in order.
fn norm(r: [f32; 4]) -> [f32; 4] {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}

/// The lines of text a box fits, broken at spaces, at `size` points of
/// Helvetica — near enough at 0.5 em a letter.
fn wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    let most = ((width / (size * 0.5)).floor() as usize).max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > most {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// Text in a content stream: WinAnsi's printable letters, the rest as `?`.
fn literal(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (' '..='~').contains(&c) => out.push(c),
            c if (c as u32) < 256 => out.push_str(&format!("\\{:03o}", c as u32)),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// What a comment draws, as a content stream over `rect`; its fill and
/// stroke in `color`; `alpha` through the graphics state `/G`.
fn drawing(
    kind: &str,
    rect: [f32; 4],
    quads: &[[f32; 8]],
    strokes: &[Vec<[f32; 2]>],
    line: Option<[f32; 4]>,
    color: [f32; 3],
    text: &str,
    arrow: bool,
) -> String {
    drawn(
        kind, rect, quads, strokes, line, color, text, arrow, 2.0, None,
    )
}

/// As [`drawing`], its lines `width` wide, and a shape filled with `fill`.
#[allow(clippy::too_many_arguments)]
fn drawn(
    kind: &str,
    rect: [f32; 4],
    quads: &[[f32; 8]],
    strokes: &[Vec<[f32; 2]>],
    line: Option<[f32; 4]>,
    color: [f32; 3],
    text: &str,
    arrow: bool,
    width: f32,
    fill: Option<[f32; 3]>,
) -> String {
    let [r, g, b] = color;
    let (rg, rg_stroke) = (
        format!("{} {} {} rg", n(r), n(g), n(b)),
        format!("{} {} {} RG", n(r), n(g), n(b)),
    );
    let w = n(width);
    // A shape is filled, then stroked, when it has a fill.
    let (fill_op, paint) = match fill {
        Some([fr, fg, fb]) => (format!(" {} {} {} rg", n(fr), n(fg), n(fb)), "B"),
        None => (String::new(), "S"),
    };
    let mut out = String::from("q /G gs ");
    match kind {
        "Highlight" => {
            out.push_str(&rg);
            for q in quads {
                out.push_str(&format!(
                    " {} {} m {} {} l {} {} l {} {} l h f",
                    n(q[4]),
                    n(q[5]),
                    n(q[6]),
                    n(q[7]),
                    n(q[2]),
                    n(q[3]),
                    n(q[0]),
                    n(q[1])
                ));
            }
        }
        "Underline" | "StrikeOut" => {
            out.push_str(&rg_stroke);
            for q in quads {
                let height = (q[1] - q[5]).abs();
                let y = if kind == "Underline" {
                    q[5] + height * 0.08
                } else {
                    q[5] + height * 0.5
                };
                out.push_str(&format!(
                    " {} w {} {} m {} {} l S",
                    n((height / 14.0).max(0.5)),
                    n(q[4]),
                    n(y),
                    n(q[6]),
                    n(y)
                ));
            }
        }
        "Text" => {
            // A note: a page with its corner turned, and lines on it.
            let [x1, y1, x2, y2] = rect;
            let fold = (x2 - x1) * 0.3;
            out.push_str(&format!(
                " {rg} 0 0 0 RG 0.6 w {} {} m {} {} l {} {} l {} {} l {} {} l h B {} {} m {} {} l {} {} l S 0.5 w {} {} m {} {} l S {} {} m {} {} l S",
                n(x1 + 1.0), n(y1 + 1.0), n(x2 - 1.0), n(y1 + 1.0), n(x2 - 1.0), n(y2 - 1.0 - fold), n(x2 - 1.0 - fold), n(y2 - 1.0), n(x1 + 1.0), n(y2 - 1.0),
                n(x2 - 1.0 - fold), n(y2 - 1.0), n(x2 - 1.0 - fold), n(y2 - 1.0 - fold), n(x2 - 1.0), n(y2 - 1.0 - fold),
                n(x1 + 4.0), n(y1 + (y2 - y1) * 0.55), n(x2 - 6.0), n(y1 + (y2 - y1) * 0.55), n(x1 + 4.0), n(y1 + (y2 - y1) * 0.35), n(x2 - 4.0), n(y1 + (y2 - y1) * 0.35)
            ));
        }
        "FreeText" => {
            let [x1, y1, x2, y2] = rect;
            let size = 11.0;
            out.push_str(&format!(
                " {rg_stroke} 0.75 w {} {} {} {} re S BT /Helv {} Tf {rg}",
                n(x1 + 0.5),
                n(y1 + 0.5),
                n(x2 - x1 - 1.0),
                n(y2 - y1 - 1.0),
                n(size)
            ));
            let mut y = y2 - 4.0 - size;
            for (i, l) in wrap(text, x2 - x1 - 8.0, size).iter().enumerate() {
                if y < y1 {
                    break;
                }
                if i == 0 {
                    out.push_str(&format!(" {} {} Td", n(x1 + 4.0), n(y)));
                } else {
                    out.push_str(&format!(" 0 {} Td", n(-size * 1.2)));
                }
                out.push_str(&format!(" {} Tj", literal(l)));
                y -= size * 1.2;
            }
            out.push_str(" ET");
        }
        "Squiggly" => {
            // A wave under each line of text, a period a little over its
            // height.
            out.push_str(&rg_stroke);
            for q in quads {
                let height = (q[1] - q[5]).abs();
                let (x1, x2, y) = (q[4], q[6], q[5] + height * 0.06);
                let step = (height / 4.0).max(1.5);
                let amp = (height / 12.0).max(0.6);
                out.push_str(&format!(
                    " {} w {} {} m",
                    n((height / 16.0).max(0.4)),
                    n(x1),
                    n(y)
                ));
                let mut x = x1;
                let mut up = true;
                while x < x2 {
                    x = (x + step).min(x2);
                    out.push_str(&format!(
                        " {} {} l",
                        n(x),
                        n(if up { y + amp } else { y - amp })
                    ));
                    up = !up;
                }
                out.push_str(" S");
            }
        }
        "Polygon" | "PolyLine" => {
            if let Some(points) = strokes.first() {
                out.push_str(&format!("{rg_stroke}{fill_op} {w} w 1 J 1 j"));
                for (i, [x, y]) in points.iter().enumerate() {
                    out.push_str(&format!(
                        " {} {} {}",
                        n(*x),
                        n(*y),
                        if i == 0 { "m" } else { "l" }
                    ));
                }
                if kind == "Polygon" {
                    out.push_str(&format!(" h {paint}"));
                } else {
                    out.push_str(" S");
                }
            }
        }
        "Ink" => {
            out.push_str(&format!("{rg_stroke} {w} w 1 J 1 j"));
            for stroke in strokes {
                for (i, [x, y]) in stroke.iter().enumerate() {
                    out.push_str(&format!(
                        " {} {} {}",
                        n(*x),
                        n(*y),
                        if i == 0 { "m" } else { "l" }
                    ));
                }
                if stroke.len() == 1 {
                    out.push_str(&format!(" {} {} l", n(stroke[0][0] + 0.1), n(stroke[0][1])));
                }
                out.push_str(" S");
            }
        }
        "Square" => {
            let [x1, y1, x2, y2] = rect;
            out.push_str(&format!(
                "{rg_stroke}{fill_op} {w} w {} {} {} {} re {paint}",
                n(x1 + 1.0),
                n(y1 + 1.0),
                n(x2 - x1 - 2.0),
                n(y2 - y1 - 2.0)
            ));
        }
        "Circle" => {
            // An ellipse, as four Bézier quarters.
            let [x1, y1, x2, y2] = rect;
            let (cx, cy, rx, ry) = (
                (x1 + x2) / 2.0,
                (y1 + y2) / 2.0,
                (x2 - x1) / 2.0 - 1.0,
                (y2 - y1) / 2.0 - 1.0,
            );
            let k = 0.5523;
            out.push_str(&format!(
                "{rg_stroke}{fill_op} {w} w {} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {paint}",
                n(cx + rx), n(cy),
                n(cx + rx), n(cy + k * ry), n(cx + k * rx), n(cy + ry), n(cx), n(cy + ry),
                n(cx - k * rx), n(cy + ry), n(cx - rx), n(cy + k * ry), n(cx - rx), n(cy),
                n(cx - rx), n(cy - k * ry), n(cx - k * rx), n(cy - ry), n(cx), n(cy - ry),
                n(cx + k * rx), n(cy - ry), n(cx + rx), n(cy - k * ry), n(cx + rx), n(cy)
            ));
        }
        "Line" => {
            if let Some([x1, y1, x2, y2]) = line {
                out.push_str(&format!(
                    "{rg_stroke} {w} w 1 J {} {} m {} {} l S",
                    n(x1),
                    n(y1),
                    n(x2),
                    n(y2)
                ));
                if arrow {
                    let angle = (y2 - y1).atan2(x2 - x1);
                    let (a1, a2) = (angle + 2.6, angle - 2.6);
                    out.push_str(&format!(
                        " {} {} m {} {} l {} {} l S",
                        n(x2 + 9.0 * a1.cos()),
                        n(y2 + 9.0 * a1.sin()),
                        n(x2),
                        n(y2),
                        n(x2 + 9.0 * a2.cos()),
                        n(y2 + 9.0 * a2.sin())
                    ));
                }
            }
        }
        _ => {}
    }
    out.push_str(" Q");
    out
}

/// The appearance stream for a comment over `rect`.
fn appearance(
    doc: &mut Document,
    rect: [f32; 4],
    content: String,
    alpha: f32,
    multiply: bool,
) -> ObjectId {
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let mut state = dictionary! { "Type" => "ExtGState", "CA" => alpha, "ca" => alpha };
    if multiply {
        state.set("BM", "Multiply");
    }
    doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            "Resources" => dictionary! { "ExtGState" => dictionary! { "G" => state }, "Font" => dictionary! { "Helv" => font } },
        },
        content.into_bytes(),
    ))
}

/// The page's own `Annots` array, made a direct array on the page if it
/// was a reference, so it can be changed in the page alone.
fn annots_of(doc: &mut Document, page: ObjectId) -> Result<Vec<Object>, String> {
    let dict = doc
        .get_object(page)
        .and_then(Object::as_dict)
        .map_err(|e| e.to_string())?;
    Ok(match dict.get(b"Annots") {
        Ok(Object::Array(items)) => items.clone(),
        Ok(Object::Reference(id)) => doc
            .get_object(*id)
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    })
}

fn set_annots(doc: &mut Document, page: ObjectId, annots: Vec<Object>) -> Result<(), String> {
    let dict = doc
        .get_object_mut(page)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?;
    if annots.is_empty() {
        dict.remove(b"Annots");
    } else {
        dict.set("Annots", annots);
    }
    Ok(())
}

/// The quads over text a set of boxes makes: upper left, upper right,
/// lower left, lower right, as the standard orders them.
fn quads(boxes: &[[f32; 4]]) -> Vec<[f32; 8]> {
    boxes
        .iter()
        .map(|b| norm(*b))
        .map(|[x1, y1, x2, y2]| [x1, y2, x2, y2, x1, y1, x2, y1])
        .collect()
}

/// Add a comment to page `page` (from one), by `author` on `date` — named
/// `name` so it can be found again. Its object.
pub fn add(
    doc: &mut Document,
    page: u32,
    mark: &Mark,
    author: &str,
    date: &str,
    name: &str,
) -> Result<ObjectId, String> {
    let page_id = *pages::ids(doc, &[page])?.first().ok_or("no page")?;
    let color = rgb(&mark.color);
    let (subtype, alpha, multiply) = match mark.kind.as_str() {
        "highlight" => ("Highlight", 0.4, true),
        "underline" => ("Underline", 1.0, false),
        "strike" => ("StrikeOut", 1.0, false),
        "squiggly" => ("Squiggly", 1.0, false),
        "polygon" => ("Polygon", 1.0, false),
        "polyline" => ("PolyLine", 1.0, false),
        "note" => ("Text", 1.0, false),
        "text" => ("FreeText", 1.0, false),
        "ink" => ("Ink", 1.0, false),
        "square" => ("Square", 1.0, false),
        "circle" => ("Circle", 1.0, false),
        "line" | "arrow" => ("Line", 1.0, false),
        other => return Err(format!("{other}: not a kind of comment")),
    };
    let alpha = mark.opacity.map(|o| o.clamp(0.05, 1.0)).unwrap_or(alpha);
    let width = mark.width.map(|w| w.clamp(0.25, 72.0)).unwrap_or(2.0);
    let fill = mark.fill.as_deref().map(rgb);
    let mut annot = dictionary! {
        "Type" => "Annot",
        "Subtype" => subtype,
        "P" => page_id,
        "C" => color.iter().map(|c| Object::Real(*c)).collect::<Vec<_>>(),
        "CA" => alpha,
        "T" => text_string(author),
        "M" => Object::string_literal(date),
        "CreationDate" => Object::string_literal(date),
        "NM" => Object::string_literal(name),
        "F" => 4,
    };
    if !mark.text.is_empty() {
        annot.set("Contents", text_string(&mark.text));
    }
    if mark.width.is_some() {
        annot.set("BS", dictionary! { "W" => width });
    }
    if let Some(f) = fill {
        annot.set("IC", f.iter().map(|c| Object::Real(*c)).collect::<Vec<_>>());
    }
    let (rect, content) = match subtype {
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            if mark.boxes.is_empty() {
                return Err("mark some text first".into());
            }
            let q = quads(&mark.boxes);
            annot.set(
                "QuadPoints",
                q.iter()
                    .flatten()
                    .map(|v| Object::Real(*v))
                    .collect::<Vec<_>>(),
            );
            let rect = bounds(q.iter().flat_map(|q| [[q[0], q[1]], [q[6], q[7]]]), 1.0);
            (
                rect,
                drawing(subtype, rect, &q, &[], None, color, "", false),
            )
        }
        "Text" => {
            let [x, y, ..] = mark.rect.ok_or("say where the note goes")?;
            let rect = [x, y - 24.0, x + 24.0, y];
            annot.set("Name", "Comment");
            (
                rect,
                drawing(subtype, rect, &[], &[], None, color, "", false),
            )
        }
        "FreeText" => {
            let rect = norm(mark.rect.ok_or("say where the text goes")?);
            let [r, g, b] = color;
            annot.set(
                "DA",
                Object::string_literal(format!("/Helv 11 Tf {} {} {} rg", n(r), n(g), n(b))),
            );
            (
                rect,
                drawing(subtype, rect, &[], &[], None, color, &mark.text, false),
            )
        }
        "Ink" => {
            if mark.strokes.iter().all(Vec::is_empty) {
                return Err("draw something first".into());
            }
            annot.set(
                "InkList",
                mark.strokes
                    .iter()
                    .map(|s| Object::Array(s.iter().flatten().map(|v| Object::Real(*v)).collect()))
                    .collect::<Vec<_>>(),
            );
            let rect = bounds(mark.strokes.iter().flatten().copied(), width.max(2.0));
            (
                rect,
                drawn(
                    subtype,
                    rect,
                    &[],
                    &mark.strokes,
                    None,
                    color,
                    "",
                    false,
                    width,
                    None,
                ),
            )
        }
        "Polygon" | "PolyLine" => {
            let points = mark
                .strokes
                .first()
                .filter(|p| p.len() >= 2)
                .ok_or("place two corners or more")?;
            annot.set(
                "Vertices",
                points
                    .iter()
                    .flatten()
                    .map(|v| Object::Real(*v))
                    .collect::<Vec<_>>(),
            );
            let rect = bounds(points.iter().copied(), width.max(2.0));
            (
                rect,
                drawn(
                    subtype,
                    rect,
                    &[],
                    &mark.strokes[..1],
                    None,
                    color,
                    "",
                    false,
                    width,
                    fill,
                ),
            )
        }
        "Square" | "Circle" => {
            let rect = norm(mark.rect.ok_or("say where it goes")?);
            (
                rect,
                drawn(subtype, rect, &[], &[], None, color, "", false, width, fill),
            )
        }
        _ => {
            let ends = mark.rect.ok_or("say where it goes")?;
            annot.set(
                "L",
                ends.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            );
            let arrow = mark.kind == "arrow";
            if arrow {
                annot.set(
                    "LE",
                    vec![
                        Object::Name(b"None".to_vec()),
                        Object::Name(b"OpenArrow".to_vec()),
                    ],
                );
            }
            let rect = bounds([[ends[0], ends[1]], [ends[2], ends[3]]], 10.0);
            (
                rect,
                drawn(
                    "Line",
                    rect,
                    &[],
                    &[],
                    Some(ends),
                    color,
                    "",
                    arrow,
                    width,
                    None,
                ),
            )
        }
    };
    annot.set(
        "Rect",
        rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
    );
    let ap = appearance(doc, rect, content, alpha, multiply);
    annot.set("AP", dictionary! { "N" => ap });
    let id = doc.add_object(annot);
    let mut annots = annots_of(doc, page_id)?;
    annots.push(Object::Reference(id));
    set_annots(doc, page_id, annots)?;
    Ok(id)
}

/// A reply to a comment, as the standard has one: a note in reply to it,
/// drawn nowhere of its own.
pub fn reply(
    doc: &mut Document,
    to: ObjectId,
    text: &str,
    author: &str,
    date: &str,
    name: &str,
) -> Result<ObjectId, String> {
    let parent = doc
        .get_object(to)
        .and_then(Object::as_dict)
        .map_err(|_| "there is no such comment".to_string())?;
    let page = parent
        .get(b"P")
        .and_then(Object::as_reference)
        .map_err(|_| "the comment says no page".to_string())?;
    let rect = parent
        .get(b"Rect")
        .cloned()
        .unwrap_or_else(|_| Object::Array(vec![0.into(), 0.into(), 0.into(), 0.into()]));
    let id = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "IRT" => to,
        "RT" => "R",
        "P" => page,
        "Rect" => rect,
        "Contents" => text_string(text),
        "T" => text_string(author),
        "M" => Object::string_literal(date),
        "NM" => Object::string_literal(name),
        // Hidden and not printed: a reply is read in the comments, not on the page.
        "F" => 2 | 32,
    });
    let mut annots = annots_of(doc, page)?;
    annots.push(Object::Reference(id));
    set_annots(doc, page, annots)?;
    Ok(id)
}

/// Change what a comment says, or its colour; its appearance drawn again
/// for the kinds Quark draws.
pub fn edit(
    doc: &mut Document,
    id: ObjectId,
    color: Option<&str>,
    text: Option<&str>,
    date: &str,
) -> Result<(), String> {
    let dict = doc
        .get_object(id)
        .and_then(Object::as_dict)
        .map_err(|_| "there is no such comment".to_string())?
        .clone();
    let mut changed = dict.clone();
    if let Some(text) = text {
        changed.set("Contents", text_string(text));
    }
    if let Some(color) = color {
        changed.set(
            "C",
            rgb(color)
                .iter()
                .map(|c| Object::Real(*c))
                .collect::<Vec<_>>(),
        );
    }
    changed.set("M", Object::string_literal(date));
    // Drawn again from what it is: the kinds whose look follows from their own fields.
    let subtype = name_of(&changed, b"Subtype");
    let numbers = |key: &[u8]| -> Vec<f32> {
        changed
            .get(key)
            .and_then(Object::as_array)
            .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
            .unwrap_or_default()
    };
    let rect = numbers(b"Rect");
    let color_now: Vec<f32> = numbers(b"C");
    let colour = [
        *color_now.first().unwrap_or(&1.0),
        *color_now.get(1).unwrap_or(&0.83),
        *color_now.get(2).unwrap_or(&0.0),
    ];
    if rect.len() == 4
        && [
            "Highlight",
            "Underline",
            "StrikeOut",
            "Text",
            "FreeText",
            "Square",
            "Circle",
        ]
        .contains(&subtype.as_str())
    {
        let rect = [rect[0], rect[1], rect[2], rect[3]];
        let q: Vec<[f32; 8]> = numbers(b"QuadPoints")
            .chunks_exact(8)
            .map(|c| [c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])
            .collect();
        let said = changed
            .get(b"Contents")
            .and_then(Object::as_str)
            .map(read_text)
            .unwrap_or_default();
        let alpha = changed.get(b"CA").and_then(Object::as_float).unwrap_or(1.0);
        let ap = appearance(
            doc,
            rect,
            drawing(&subtype, rect, &q, &[], None, colour, &said, false),
            alpha,
            subtype == "Highlight",
        );
        changed.set("AP", dictionary! { "N" => ap });
    }
    doc.objects.insert(id, Object::Dictionary(changed));
    Ok(())
}

/// Delete a comment, and the replies to it.
/// Move a comment by `by`, in the page's points: its box and every point it
/// has. Its appearance is drawn into its box wherever that is, so it moves
/// with it unchanged.
pub fn shift(doc: &mut Document, id: ObjectId, by: [f32; 2]) -> Result<(), String> {
    let dict = doc
        .get_object_mut(id)
        .and_then(Object::as_dict_mut)
        .map_err(|_| "there is no such comment".to_string())?;
    if dict.get(b"Subtype").and_then(Object::as_name).is_err() {
        return Err("there is no such comment".into());
    }
    // Arrays of x, y pairs, moved pair by pair; InkList's are arrays of them.
    let moved = |items: &[Object]| -> Vec<Object> {
        items
            .iter()
            .enumerate()
            .map(|(i, v)| match v.as_float() {
                Ok(f) => Object::Real(f + by[i % 2]),
                Err(_) => v.clone(),
            })
            .collect()
    };
    for key in [b"Rect".as_slice(), b"QuadPoints", b"L", b"Vertices", b"CL"] {
        if let Ok(Object::Array(items)) = dict.get(key) {
            let shifted = moved(items);
            dict.set(key, shifted);
        }
    }
    if let Ok(Object::Array(strokes)) = dict.get(b"InkList") {
        let shifted: Vec<Object> = strokes
            .iter()
            .map(|s| match s {
                Object::Array(points) => Object::Array(moved(points)),
                other => other.clone(),
            })
            .collect();
        dict.set("InkList", shifted);
    }
    Ok(())
}

pub fn delete(doc: &mut Document, id: ObjectId) -> Result<(), String> {
    let page = doc
        .get_object(id)
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"P"))
        .and_then(Object::as_reference)
        .ok();
    let pages: Vec<ObjectId> = match page {
        Some(p) => vec![p],
        None => doc.get_pages().into_values().collect(),
    };
    let mut gone = vec![id];
    let mut found = false;
    for page in pages {
        let annots = annots_of(doc, page)?;
        // The replies, and their replies.
        loop {
            let more: Vec<ObjectId> = annots
                .iter()
                .filter_map(|a| a.as_reference().ok())
                .filter(|a| !gone.contains(a))
                .filter(|a| {
                    doc.get_object(*a)
                        .and_then(Object::as_dict)
                        .and_then(|d| d.get(b"IRT"))
                        .and_then(Object::as_reference)
                        .is_ok_and(|to| gone.contains(&to))
                })
                .collect();
            if more.is_empty() {
                break;
            }
            gone.extend(more);
        }
        let kept: Vec<Object> = annots
            .iter()
            .filter(|a| !a.as_reference().is_ok_and(|r| gone.contains(&r)))
            .cloned()
            .collect();
        if kept.len() != annots.len() {
            found = true;
            set_annots(doc, page, kept)?;
        }
    }
    if found {
        Ok(())
    } else {
        Err("there is no such comment".into())
    }
}

fn name_of(dict: &Dictionary, key: &[u8]) -> String {
    dict.get(key)
        .and_then(Object::as_name)
        .map(|n| String::from_utf8_lossy(n).to_string())
        .unwrap_or_default()
}

/// Every comment in the document, page by page, replies after what they
/// answer.
pub fn list(doc: &Document) -> Vec<Comment> {
    let mut out = Vec::new();
    for (page, page_id) in doc.get_pages() {
        let annots = doc
            .get_object(page_id)
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"Annots"))
            .ok()
            .and_then(|a| match a {
                Object::Array(items) => Some(items.clone()),
                Object::Reference(id) => {
                    doc.get_object(*id).and_then(Object::as_array).ok().cloned()
                }
                _ => None,
            });
        for annot in annots.unwrap_or_default() {
            let Ok(id) = annot.as_reference() else {
                continue;
            };
            let Ok(dict) = doc.get_object(id).and_then(Object::as_dict) else {
                continue;
            };
            let kind = name_of(dict, b"Subtype");
            if !KINDS.contains(&kind.as_str()) {
                continue;
            }
            let numbers: Vec<f32> = dict
                .get(b"Rect")
                .and_then(Object::as_array)
                .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
                .unwrap_or_default();
            let rect = if numbers.len() == 4 {
                norm([numbers[0], numbers[1], numbers[2], numbers[3]])
            } else {
                [0.0; 4]
            };
            let color: Vec<f32> = dict
                .get(b"C")
                .and_then(Object::as_array)
                .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
                .unwrap_or_default();
            let text = |key: &[u8]| {
                dict.get(key)
                    .and_then(Object::as_str)
                    .map(read_text)
                    .unwrap_or_default()
            };
            out.push(Comment {
                id: format!("{} {}", id.0, id.1),
                page,
                kind,
                rect,
                color: (color.len() == 3).then(|| hex_of(&color)),
                text: text(b"Contents"),
                author: text(b"T"),
                date: text(b"M"),
                reply_to: dict
                    .get(b"IRT")
                    .and_then(Object::as_reference)
                    .ok()
                    .map(|r| format!("{} {}", r.0, r.1)),
            });
        }
    }
    out
}

/// A date as the PDF standard writes one, from seconds since 1970 in UTC:
/// `D:20261012143000Z`.
pub fn date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::load_mem(&crate::fixture::fixture(&[
            "Some words to mark.",
            "Page two.",
        ]))
        .unwrap()
    }

    fn mark(kind: &str) -> Mark {
        Mark {
            kind: kind.into(),
            boxes: Vec::new(),
            rect: None,
            strokes: Vec::new(),
            color: "#ffd400".into(),
            text: String::new(),
            width: None,
            opacity: None,
            fill: None,
        }
    }

    #[test]
    fn every_kind_is_written_with_an_appearance_and_listed_back() {
        let mut d = doc();
        let marks = [
            Mark {
                boxes: vec![[72.0, 716.0, 180.0, 732.0], [72.0, 698.0, 120.0, 714.0]],
                ..mark("highlight")
            },
            Mark {
                boxes: vec![[72.0, 716.0, 180.0, 732.0]],
                ..mark("underline")
            },
            Mark {
                boxes: vec![[72.0, 716.0, 180.0, 732.0]],
                ..mark("strike")
            },
            Mark {
                rect: Some([300.0, 700.0, 0.0, 0.0]),
                text: "Why?".into(),
                ..mark("note")
            },
            Mark {
                rect: Some([100.0, 400.0, 300.0, 460.0]),
                text: "A box (with brackets) — and é".into(),
                ..mark("text")
            },
            Mark {
                strokes: vec![vec![[100.0, 100.0], [120.0, 130.0], [150.0, 110.0]]],
                ..mark("ink")
            },
            Mark {
                rect: Some([50.0, 50.0, 90.0, 80.0]),
                ..mark("square")
            },
            Mark {
                rect: Some([50.0, 50.0, 90.0, 80.0]),
                ..mark("circle")
            },
            Mark {
                rect: Some([10.0, 10.0, 200.0, 40.0]),
                ..mark("arrow")
            },
        ];
        for m in &marks {
            add(&mut d, 1, m, "Ada", "D:20261012000000Z", "quark-1").unwrap();
        }
        let listed = list(&d);
        assert_eq!(
            listed.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(),
            [
                "Highlight",
                "Underline",
                "StrikeOut",
                "Text",
                "FreeText",
                "Ink",
                "Square",
                "Circle",
                "Line"
            ]
        );
        assert!(listed
            .iter()
            .all(|c| c.page == 1 && c.author == "Ada" && c.color.as_deref() == Some("#ffd400")));
        assert_eq!(
            listed[4].text, "A box (with brackets) — and é",
            "UTF-16 text survives"
        );
        assert_eq!(
            listed[0].rect,
            [71.0, 697.0, 181.0, 733.0],
            "the highlight covers both lines"
        );
        // Each has its own appearance, which survives writing and reading.
        let mut bytes = Vec::new();
        d.save_to(&mut bytes).unwrap();
        let read = Document::load_mem(&bytes).unwrap();
        for c in list(&read) {
            let annot = read
                .get_object(object_of(&c.id).unwrap())
                .unwrap()
                .as_dict()
                .unwrap();
            let ap = annot
                .get(b"AP")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"N")
                .unwrap()
                .as_reference()
                .unwrap();
            let stream = read.get_object(ap).unwrap().as_stream().unwrap();
            assert!(
                String::from_utf8_lossy(&stream.content).starts_with("q /G gs"),
                "{}",
                c.kind
            );
        }
        assert!(
            add(&mut d, 1, &mark("highlight"), "Ada", "D:", "x").is_err(),
            "a highlight needs text"
        );
        assert!(add(&mut d, 9, &mark("note"), "Ada", "D:", "x").is_err());
    }

    #[test]
    fn a_comment_is_answered_recoloured_edited_and_deleted_with_its_replies() {
        let mut d = doc();
        let note = add(
            &mut d,
            2,
            &Mark {
                rect: Some([100.0, 700.0, 0.0, 0.0]),
                text: "First".into(),
                ..mark("note")
            },
            "Ada",
            "D:1",
            "a",
        )
        .unwrap();
        let answer = reply(&mut d, note, "An answer", "Grace", "D:2", "b").unwrap();
        reply(&mut d, answer, "And another", "Ada", "D:3", "c").unwrap();
        let listed = list(&d);
        assert_eq!(
            listed
                .iter()
                .map(|c| (c.text.as_str(), c.reply_to.is_some()))
                .collect::<Vec<_>>(),
            [("First", false), ("An answer", true), ("And another", true)]
        );
        edit(&mut d, note, Some("#3366ff"), Some("First, edited"), "D:4").unwrap();
        let first = &list(&d)[0];
        assert_eq!(
            (
                first.text.as_str(),
                first.color.as_deref(),
                first.date.as_str()
            ),
            ("First, edited", Some("#3366ff"), "D:4")
        );
        delete(&mut d, note).unwrap();
        assert!(list(&d).is_empty(), "the replies go with it");
        assert!(delete(&mut d, note).is_err());
    }

    #[test]
    fn a_date_is_written_as_the_standard_has_it() {
        assert_eq!(date(0), "D:19700101000000Z");
        assert_eq!(date(1_791_772_200), "D:20261012023000Z");
        assert_eq!(date(951_782_400), "D:20000229000000Z", "a leap day");
    }

    /// Every kind written into a PDF at `QUARK_PDF_SAMPLE`, to be looked
    /// at in another viewer: run by hand.
    #[test]
    #[ignore = "writes a sample PDF where QUARK_PDF_SAMPLE says"]
    fn a_sample_of_every_kind_to_look_at() {
        let Ok(path) = std::env::var("QUARK_PDF_SAMPLE") else {
            return;
        };
        let mut d = doc();
        let marks = [
            Mark {
                boxes: vec![[72.0, 716.0, 200.0, 732.0]],
                ..mark("highlight")
            },
            Mark {
                boxes: vec![[72.0, 690.0, 200.0, 706.0]],
                color: "#4fa3ff".into(),
                ..mark("underline")
            },
            Mark {
                boxes: vec![[72.0, 664.0, 200.0, 680.0]],
                color: "#ff5a4f".into(),
                ..mark("strike")
            },
            Mark {
                rect: Some([300.0, 720.0, 0.0, 0.0]),
                text: "Why?".into(),
                ..mark("note")
            },
            Mark {
                rect: Some([72.0, 520.0, 300.0, 600.0]),
                text: "A text box that wraps onto a second line, with é.".into(),
                color: "#222222".into(),
                ..mark("text")
            },
            Mark {
                strokes: vec![vec![
                    [350.0, 500.0],
                    [380.0, 560.0],
                    [420.0, 520.0],
                    [460.0, 580.0],
                ]],
                color: "#7bd148".into(),
                ..mark("ink")
            },
            Mark {
                rect: Some([72.0, 380.0, 200.0, 460.0]),
                color: "#ff7eb6".into(),
                ..mark("square")
            },
            Mark {
                rect: Some([240.0, 380.0, 380.0, 460.0]),
                color: "#4fa3ff".into(),
                ..mark("circle")
            },
            Mark {
                rect: Some([420.0, 380.0, 540.0, 460.0]),
                color: "#ff5a4f".into(),
                ..mark("arrow")
            },
        ];
        for m in &marks {
            add(&mut d, 1, m, "Sample", "D:20261012000000Z", "sample").unwrap();
        }
        let mut bytes = Vec::new();
        d.save_to(&mut bytes).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn squiggles_polygons_and_polylines_are_written_with_their_width_opacity_and_fill() {
        let mut d = doc();
        let corners = vec![[100.0, 500.0], [160.0, 560.0], [220.0, 500.0]];
        let squiggly = add(
            &mut d,
            1,
            &Mark {
                boxes: vec![[72.0, 716.0, 180.0, 732.0]],
                ..mark("squiggly")
            },
            "Ada",
            "D:",
            "s",
        )
        .unwrap();
        let polygon = add(
            &mut d,
            1,
            &Mark {
                strokes: vec![corners.clone()],
                width: Some(4.0),
                opacity: Some(0.5),
                fill: Some("#00ff00".into()),
                ..mark("polygon")
            },
            "Ada",
            "D:",
            "p",
        )
        .unwrap();
        let polyline = add(
            &mut d,
            1,
            &Mark {
                strokes: vec![corners],
                ..mark("polyline")
            },
            "Ada",
            "D:",
            "l",
        )
        .unwrap();
        let get = |id| d.get_object(id).unwrap().as_dict().unwrap().clone();
        let p = get(polygon);
        assert_eq!(p.get(b"Subtype").unwrap().as_name().unwrap(), b"Polygon");
        assert_eq!(p.get(b"Vertices").unwrap().as_array().unwrap().len(), 6);
        assert_eq!(p.get(b"CA").unwrap().as_float().unwrap(), 0.5);
        assert_eq!(
            p.get(b"BS")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"W")
                .unwrap()
                .as_float()
                .unwrap(),
            4.0
        );
        assert_eq!(p.get(b"IC").unwrap().as_array().unwrap().len(), 3);
        let ap = p
            .get(b"AP")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"N")
            .unwrap()
            .as_reference()
            .unwrap();
        let drawn =
            String::from_utf8_lossy(&d.get_object(ap).unwrap().as_stream().unwrap().content)
                .to_string();
        assert!(
            drawn.contains(" 4 w") && drawn.contains(" h B"),
            "filled and stroked 4 wide: {drawn}"
        );
        assert_eq!(
            get(polyline).get(b"Subtype").unwrap().as_name().unwrap(),
            b"PolyLine"
        );
        assert_eq!(
            get(squiggly).get(b"Subtype").unwrap().as_name().unwrap(),
            b"Squiggly"
        );
        let kinds: Vec<String> = list(&d).into_iter().map(|c| c.kind).collect();
        assert!(
            kinds.contains(&"Squiggly".into())
                && kinds.contains(&"Polygon".into())
                && kinds.contains(&"PolyLine".into()),
            "{kinds:?}"
        );
        assert!(
            add(
                &mut d,
                1,
                &Mark {
                    strokes: vec![vec![[1.0, 1.0]]],
                    ..mark("polygon")
                },
                "Ada",
                "D:",
                "x"
            )
            .is_err(),
            "one corner is not a polygon"
        );
    }

    #[test]
    fn a_comment_moves_with_everything_it_points_at() {
        let mut d = doc();
        let ink = add(&mut d, 1, &Mark { strokes: vec![vec![[100.0, 100.0], [120.0, 130.0]]], ..mark("ink") }, "Ada", "D:", "i").unwrap();
        let line = add(&mut d, 1, &Mark { rect: Some([10.0, 10.0, 50.0, 60.0]), ..mark("arrow") }, "Ada", "D:", "l").unwrap();
        shift(&mut d, ink, [5.0, -10.0]).unwrap();
        shift(&mut d, line, [1.0, 2.0]).unwrap();
        let get = |id, key: &[u8]| -> Vec<f32> {
            d.get_object(id).unwrap().as_dict().unwrap().get(key).unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect()
        };
        let stroke: Vec<f32> = d.get_object(ink).unwrap().as_dict().unwrap().get(b"InkList").unwrap().as_array().unwrap()[0]
            .as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect();
        assert_eq!(stroke, [105.0, 90.0, 125.0, 120.0]);
        assert_eq!(get(line, b"L"), [11.0, 12.0, 51.0, 62.0]);
        let rect = get(ink, b"Rect");
        assert!((rect[0] - 103.0).abs() < 0.01 && (rect[1] - 88.0).abs() < 0.01, "{rect:?}");
        assert!(shift(&mut d, (999, 0), [1.0, 1.0]).is_err());
    }

}
