//! A PDF's form: its fields listed — text, check boxes, radio buttons,
//! choices — with where each widget is; a field's value set, with an
//! appearance drawn for it so every viewer shows it; and the form
//! flattened, each widget's appearance drawn into its page for good.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};

use crate::annot::{read_text, text_string};

/// Where a field is shown: a page, and a box in its points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    pub page: u32,
    pub rect: [f32; 4],
    /// A check box's or radio button's name for "on" here.
    pub state: Option<String>,
}

/// A field of the form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Its full name, its parents' first: `address.city`.
    pub name: String,
    /// `text`, `checkbox`, `radio`, `combo`, `list`, `button` or `signature`.
    pub kind: String,
    /// What it holds: text, a choice, or the state a box or button is in.
    pub value: String,
    /// A choice's options.
    pub options: Vec<String>,
    pub widgets: Vec<Widget>,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
}

/// The field flags this reads.
const READ_ONLY: i64 = 1;
const REQUIRED: i64 = 1 << 1;
const MULTILINE: i64 = 1 << 12;
const RADIO: i64 = 1 << 15;
const PUSH: i64 = 1 << 16;
const COMBO: i64 = 1 << 17;

fn dict(doc: &Document, id: ObjectId) -> Option<&Dictionary> {
    doc.get_object(id).and_then(Object::as_dict).ok()
}

/// A value, followed through a reference.
fn resolved<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    match object {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(object),
        other => other,
    }
}

/// A field's value for `key`, its own or inherited from its parents.
fn inherited(doc: &Document, id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut at = id;
    for _ in 0..32 {
        let d = dict(doc, at)?;
        if let Ok(value) = d.get(key) {
            return Some(resolved(doc, value).clone());
        }
        at = d.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// The form's fields, by object.
fn roots(doc: &Document) -> Vec<ObjectId> {
    let Ok(catalog) = doc.catalog() else {
        return Vec::new();
    };
    let Some(form) = catalog
        .get(b"AcroForm")
        .ok()
        .map(|f| resolved(doc, f))
        .and_then(|f| f.as_dict().ok())
    else {
        return Vec::new();
    };
    form.get(b"Fields")
        .ok()
        .map(|f| resolved(doc, f))
        .and_then(|f| f.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default()
}

/// A field's terminal fields — those with no kids that are fields — with
/// their full names; widgets that are kids are a terminal field's own.
fn terminals(
    doc: &Document,
    id: ObjectId,
    prefix: &str,
    out: &mut Vec<(String, ObjectId)>,
    depth: usize,
) {
    let Some(d) = dict(doc, id) else { return };
    let own = d.get(b"T").and_then(Object::as_str).map(read_text).ok();
    let name = match (&own, prefix.is_empty()) {
        (Some(t), true) => t.clone(),
        (Some(t), false) => format!("{prefix}.{t}"),
        (None, _) => prefix.to_string(),
    };
    let kids: Vec<ObjectId> = d
        .get(b"Kids")
        .ok()
        .map(|k| resolved(doc, k))
        .and_then(|k| k.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    // Kids that have names of their own are fields; kids that do not are this field's widgets.
    let fields: Vec<ObjectId> = kids
        .iter()
        .copied()
        .filter(|k| dict(doc, *k).is_some_and(|kd| kd.has(b"T")))
        .collect();
    if fields.is_empty() || depth > 16 {
        out.push((name, id));
    } else {
        for kid in fields {
            terminals(doc, kid, &name, out, depth + 1);
        }
    }
}

/// A field's widgets: itself, if it is one, or its kids.
fn widgets_of(doc: &Document, id: ObjectId) -> Vec<ObjectId> {
    let Some(d) = dict(doc, id) else {
        return Vec::new();
    };
    let kids: Vec<ObjectId> = d
        .get(b"Kids")
        .ok()
        .map(|k| resolved(doc, k))
        .and_then(|k| k.as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    if kids.is_empty() {
        vec![id]
    } else {
        kids
    }
}

/// A widget's "on" state: its normal appearance's name that is not Off.
fn on_state(doc: &Document, widget: ObjectId) -> Option<String> {
    let ap = dict(doc, widget)?
        .get(b"AP")
        .ok()
        .map(|a| resolved(doc, a))?
        .as_dict()
        .ok()?;
    let normal = resolved(doc, ap.get(b"N").ok()?).as_dict().ok()?;
    normal
        .iter()
        .map(|(k, _)| String::from_utf8_lossy(k).to_string())
        .find(|k| k != "Off")
}

fn rect_of(doc: &Document, widget: ObjectId) -> Option<[f32; 4]> {
    let r: Vec<f32> = dict(doc, widget)?
        .get(b"Rect")
        .ok()
        .map(|r| resolved(doc, r))?
        .as_array()
        .ok()?
        .iter()
        .filter_map(|o| o.as_float().ok())
        .collect();
    (r.len() == 4).then(|| {
        [
            r[0].min(r[2]),
            r[1].min(r[3]),
            r[0].max(r[2]),
            r[1].max(r[3]),
        ]
    })
}

/// The page each widget is on: by its `/P`, or by which page lists it.
fn pages_of(doc: &Document) -> std::collections::HashMap<ObjectId, u32> {
    let mut out = std::collections::HashMap::new();
    for (n, page) in doc.get_pages() {
        let annots = dict(doc, page)
            .and_then(|d| d.get(b"Annots").ok())
            .map(|a| resolved(doc, a).clone());
        if let Some(Object::Array(items)) = annots {
            for item in items {
                if let Ok(id) = item.as_reference() {
                    out.insert(id, n);
                }
            }
        }
    }
    out
}

/// Text as a value reads: a string, or a name.
fn value_text(doc: &Document, value: Option<Object>) -> String {
    match value.as_ref().map(|v| resolved(doc, v)) {
        Some(Object::String(bytes, _)) => read_text(bytes),
        Some(Object::Name(name)) => String::from_utf8_lossy(name).to_string(),
        Some(Object::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().ok())
            .map(read_text)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Every field of the form.
pub fn list(doc: &Document) -> Vec<Field> {
    let mut terms = Vec::new();
    for root in roots(doc) {
        terminals(doc, root, "", &mut terms, 0);
    }
    let pages = pages_of(doc);
    terms
        .into_iter()
        .map(|(name, id)| {
            let ft = inherited(doc, id, b"FT")
                .and_then(|f| f.as_name().ok().map(|n| n.to_vec()))
                .unwrap_or_default();
            let flags = inherited(doc, id, b"Ff")
                .and_then(|f| f.as_i64().ok())
                .unwrap_or(0);
            let kind = match ft.as_slice() {
                b"Tx" => "text",
                b"Btn" if flags & PUSH != 0 => "button",
                b"Btn" if flags & RADIO != 0 => "radio",
                b"Btn" => "checkbox",
                b"Ch" if flags & COMBO != 0 => "combo",
                b"Ch" => "list",
                b"Sig" => "signature",
                _ => "text",
            };
            let options = inherited(doc, id, b"Opt")
                .and_then(|o| o.as_array().ok().cloned())
                .map(|items| {
                    items
                        .iter()
                        .map(|i| match resolved(doc, i) {
                            Object::Array(pair) => pair
                                .first()
                                .and_then(|e| e.as_str().ok())
                                .map(read_text)
                                .unwrap_or_default(),
                            other => other.as_str().map(read_text).unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let widgets = widgets_of(doc, id)
                .into_iter()
                .filter_map(|w| {
                    Some(Widget {
                        page: *pages.get(&w)?,
                        rect: rect_of(doc, w)?,
                        state: on_state(doc, w).filter(|_| kind == "checkbox" || kind == "radio"),
                    })
                })
                .collect();
            Field {
                name,
                kind: kind.into(),
                value: value_text(doc, inherited(doc, id, b"V")),
                options,
                widgets,
                read_only: flags & READ_ONLY != 0,
                required: flags & REQUIRED != 0,
                multiline: flags & MULTILINE != 0,
            }
        })
        .collect()
}

/// The terminal field named `name`.
fn find(doc: &Document, name: &str) -> Result<ObjectId, String> {
    let mut terms = Vec::new();
    for root in roots(doc) {
        terminals(doc, root, "", &mut terms, 0);
    }
    terms
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, id)| id)
        .ok_or_else(|| format!("the form has no field {name}"))
}

/// Text drawn in a widget: Helvetica, at the field's own size or one that
/// fits, lines broken where a multi-line field's are.
fn text_appearance(
    doc: &mut Document,
    rect: [f32; 4],
    text: &str,
    size: f32,
    multiline: bool,
) -> ObjectId {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let size = if size > 0.0 {
        size
    } else {
        (h * 0.65).clamp(6.0, 12.0)
    };
    // Bytes, not text: the font is WinAnsi, one byte a letter, so what
    // Latin-1 has is written as itself and the rest as `?`.
    let mut content: Vec<u8> = format!("/Tx BMC q BT /Helv {size} Tf 0 g").into_bytes();
    let lines: Vec<&str> = if multiline {
        text.lines().collect()
    } else {
        vec![text.lines().next().unwrap_or("")]
    };
    let top = if multiline {
        h - 2.0 - size
    } else {
        (h - size * 0.78) / 2.0
    };
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            content.extend(format!(" 2 {top} Td (").into_bytes());
        } else {
            content.extend(format!(" 0 {} Td (", -size * 1.15).into_bytes());
        }
        for c in line.chars() {
            match c {
                '(' | ')' | '\\' => content.extend([b'\\', c as u8]),
                c if (c as u32) < 256 => content.push(c as u32 as u8),
                _ => content.push(b'?'),
            }
        }
        content.extend(b") Tj");
    }
    content.extend(b" ET Q EMC");
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)],
            "Resources" => dictionary! { "Font" => dictionary! { "Helv" => font } },
        },
        content,
    ))
}

/// The font size a field's default appearance asks for, or 0 for "fit".
fn font_size(doc: &Document, id: ObjectId) -> f32 {
    let da = inherited(doc, id, b"DA")
        .and_then(|d| d.as_str().ok().map(read_text))
        .unwrap_or_default();
    let words: Vec<&str> = da.split_whitespace().collect();
    words
        .iter()
        .position(|w| *w == "Tf")
        .and_then(|i| words.get(i.wrapping_sub(1)))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// Set the field named `name` to `value`: text, a choice, or a check box's
/// or radio button's state ("" for none).
pub fn set(doc: &mut Document, name: &str, value: &str) -> Result<(), String> {
    let id = find(doc, name)?;
    let field = list(doc)
        .into_iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("the form has no field {name}"))?;
    if field.read_only {
        return Err(format!("{name} can only be read"));
    }
    let widgets = widgets_of(doc, id);
    match field.kind.as_str() {
        "checkbox" | "radio" => {
            let on = if value.is_empty() || value == "Off" {
                None
            } else {
                Some(value.to_string())
            };
            if let Some(state) = &on {
                if !field
                    .widgets
                    .iter()
                    .any(|w| w.state.as_deref() == Some(state))
                {
                    return Err(format!("{name} has no state {state}"));
                }
            }
            let v = Object::Name(on.clone().unwrap_or_else(|| "Off".into()).into_bytes());
            set_key(doc, id, "V", v)?;
            for w in widgets {
                let mine = on_state(doc, w);
                let shown = match (&on, mine) {
                    (Some(state), Some(own)) if *state == own => own,
                    _ => "Off".into(),
                };
                set_key(doc, w, "AS", Object::Name(shown.into_bytes()))?;
            }
        }
        "button" | "signature" => return Err(format!("{name} is not filled in here")),
        kind => {
            if (kind == "combo" || kind == "list")
                && !field.options.is_empty()
                && !value.is_empty()
                && !field.options.iter().any(|o| o == value)
            {
                return Err(format!("{name} has no choice {value}"));
            }
            set_key(doc, id, "V", text_string(value))?;
            let size = font_size(doc, id);
            for w in widgets {
                let Some(rect) = rect_of(doc, w) else {
                    continue;
                };
                let ap = text_appearance(doc, rect, value, size, field.multiline);
                set_key(doc, w, "AP", Object::Dictionary(dictionary! { "N" => ap }))?;
            }
        }
    }
    // Viewers that draw fields themselves draw them again.
    if let Ok(catalog) = doc.catalog() {
        if let Ok(Object::Reference(form)) = catalog.get(b"AcroForm") {
            let form = *form;
            set_key(doc, form, "NeedAppearances", Object::Boolean(true))?;
        } else if let Ok(form) = doc
            .catalog_mut()
            .and_then(|c| c.get_mut(b"AcroForm"))
            .and_then(Object::as_dict_mut)
        {
            form.set("NeedAppearances", true);
        }
    }
    Ok(())
}

fn set_key(doc: &mut Document, id: ObjectId, key: &str, value: Object) -> Result<(), String> {
    doc.get_object_mut(id)
        .and_then(Object::as_dict_mut)
        .map_err(|e| e.to_string())?
        .set(key, value);
    Ok(())
}

/// The form flattened: each widget's appearance — in the state it shows —
/// drawn into its page, the widgets and the form taken away. What a field
/// held is then the page's, and cannot be changed as a field again.
pub fn flatten(doc: &mut Document) -> Result<usize, String> {
    let fields = list(doc);
    if fields.is_empty() {
        return Err("the PDF has no form".into());
    }
    let mut terms = Vec::new();
    for root in roots(doc) {
        terminals(doc, root, "", &mut terms, 0);
    }
    let widgets: Vec<ObjectId> = terms
        .iter()
        .flat_map(|(_, id)| widgets_of(doc, *id))
        .collect();
    let pages = pages_of(doc);
    let mut drawn = 0;
    // Page by page: each widget's appearance placed as a form XObject.
    let mut by_page: std::collections::BTreeMap<u32, Vec<ObjectId>> =
        std::collections::BTreeMap::new();
    for w in &widgets {
        if let Some(page) = pages.get(w) {
            by_page.entry(*page).or_default().push(*w);
        }
    }
    let page_ids = doc.get_pages();
    for (n, ws) in by_page {
        let page = page_ids[&n];
        crate::pages::materialize(doc, page)?;
        let mut resources = dict(doc, page)
            .and_then(|d| d.get(b"Resources").ok())
            .map(|r| resolved(doc, r).clone())
            .and_then(|r| r.as_dict().ok().cloned())
            .unwrap_or_default();
        let mut xobjects = resources
            .get(b"XObject")
            .ok()
            .map(|x| resolved(doc, x).clone())
            .and_then(|x| x.as_dict().ok().cloned())
            .unwrap_or_default();
        let mut content = String::from("\nq\n");
        for (i, w) in ws.iter().enumerate() {
            let Some(d) = dict(doc, *w) else { continue };
            let Some(rect) = rect_of(doc, *w) else {
                continue;
            };
            let hidden = d
                .get(b"F")
                .and_then(Object::as_i64)
                .is_ok_and(|f| f & 2 != 0);
            let Some(normal) = d
                .get(b"AP")
                .ok()
                .map(|a| resolved(doc, a))
                .and_then(|a| a.as_dict().ok())
                .and_then(|a| a.get(b"N").ok())
                .map(|n| resolved(doc, n).clone())
            else {
                continue;
            };
            let stream = match normal {
                Object::Stream(_) => d
                    .get(b"AP")
                    .ok()
                    .map(|a| resolved(doc, a))
                    .and_then(|a| a.as_dict().ok())
                    .and_then(|a| a.get(b"N").ok())
                    .and_then(|n| n.as_reference().ok()),
                Object::Dictionary(states) => {
                    let shown = d
                        .get(b"AS")
                        .and_then(Object::as_name)
                        .map(|s| s.to_vec())
                        .unwrap_or_else(|_| b"Off".to_vec());
                    states.get(&shown).ok().and_then(|s| s.as_reference().ok())
                }
                _ => None,
            };
            let Some(stream) = stream.filter(|_| !hidden) else {
                continue;
            };
            let bbox: Vec<f32> = doc
                .get_object(stream)
                .and_then(Object::as_stream)
                .ok()
                .and_then(|s| s.dict.get(b"BBox").ok().cloned())
                .and_then(|b| {
                    b.as_array()
                        .ok()
                        .map(|a| a.iter().filter_map(|o| o.as_float().ok()).collect())
                })
                .unwrap_or_default();
            let [bx, by, bw, bh] = if bbox.len() == 4 {
                [
                    bbox[0],
                    bbox[1],
                    (bbox[2] - bbox[0]).max(1.0),
                    (bbox[3] - bbox[1]).max(1.0),
                ]
            } else {
                [0.0, 0.0, rect[2] - rect[0], rect[3] - rect[1]]
            };
            let (sx, sy) = ((rect[2] - rect[0]) / bw, (rect[3] - rect[1]) / bh);
            let label = format!("QuarkField{i}");
            xobjects.set(label.as_bytes(), Object::Reference(stream));
            content.push_str(&format!(
                "q {sx} 0 0 {sy} {} {} cm /{label} Do Q\n",
                rect[0] - bx * sx,
                rect[1] - by * sy
            ));
            drawn += 1;
        }
        content.push_str("Q\n");
        resources.set("XObject", Object::Dictionary(xobjects));
        let added = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_dict = doc
            .get_object_mut(page)
            .and_then(Object::as_dict_mut)
            .map_err(|e| e.to_string())?;
        page_dict.set("Resources", Object::Dictionary(resources));
        // The page's own drawing first, wrapped so it cannot leave its state behind.
        let contents = page_dict.get(b"Contents").ok().cloned();
        let mut all = match contents {
            Some(Object::Array(items)) => items,
            Some(other) => vec![other],
            None => Vec::new(),
        };
        all.push(Object::Reference(added));
        page_dict.set("Contents", Object::Array(all));
        // The widgets go from the page.
        let annots = page_dict.get(b"Annots").ok().cloned();
        let kept: Vec<Object> = match annots {
            Some(Object::Array(items)) => items
                .into_iter()
                .filter(|a| !a.as_reference().is_ok_and(|r| widgets.contains(&r)))
                .collect(),
            _ => Vec::new(),
        };
        let page_dict = doc
            .get_object_mut(page)
            .and_then(Object::as_dict_mut)
            .map_err(|e| e.to_string())?;
        if kept.is_empty() {
            page_dict.remove(b"Annots");
        } else {
            page_dict.set("Annots", kept);
        }
    }
    doc.catalog_mut()
        .map_err(|e| e.to_string())?
        .remove(b"AcroForm");
    Ok(drawn)
}

/// A form to test with: a text field, a multi-line one, a check box, two
/// radio buttons and a choice, on page 1 of `doc`.
#[cfg(any(test, feature = "fixture"))]
pub fn sample(doc: &mut Document) {
    let page = doc.get_pages()[&1];
    let mut annots = Vec::new();
    let mut fields = Vec::new();
    let state = |doc: &mut Document, on: &str| {
        let yes = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] }, b"0 0 1 rg 2 2 8 8 re f".to_vec()));
        let off = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] }, Vec::new()));
        let mut n = Dictionary::new();
        n.set(on.as_bytes(), Object::Reference(yes));
        n.set("Off", Object::Reference(off));
        dictionary! { "N" => n }
    };
    let name = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("name"), "Rect" => vec![100.into(), 600.into(), 300.into(), 620.into()], "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "P" => page });
    let notes = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "Ff" => MULTILINE, "T" => Object::string_literal("notes"), "Rect" => vec![100.into(), 500.into(), 300.into(), 580.into()], "P" => page });
    let agree_ap = state(doc, "Yes");
    let agree = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => Object::string_literal("agree"), "Rect" => vec![100.into(), 460.into(), 112.into(), 472.into()], "AP" => agree_ap, "AS" => "Off", "V" => "Off", "P" => page });
    let size = doc.new_object_id();
    let small_ap = state(doc, "small");
    let large_ap = state(doc, "large");
    let small = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => size, "Rect" => vec![100.into(), 420.into(), 112.into(), 432.into()], "AP" => small_ap, "AS" => "Off", "P" => page });
    let large = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => size, "Rect" => vec![140.into(), 420.into(), 152.into(), 432.into()], "AP" => large_ap, "AS" => "Off", "P" => page });
    doc.objects.insert(size, Object::Dictionary(dictionary! { "FT" => "Btn", "Ff" => RADIO, "T" => Object::string_literal("size"), "Kids" => vec![small.into(), large.into()], "V" => "Off" }));
    let colour = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "Ff" => COMBO, "T" => Object::string_literal("colour"), "Opt" => vec![Object::string_literal("Red"), Object::string_literal("Blue")], "Rect" => vec![100.into(), 380.into(), 200.into(), 396.into()], "P" => page });
    annots.extend([name, notes, agree, small, large, colour].map(Object::Reference));
    fields.extend([name, notes, agree, size, colour].map(Object::Reference));
    doc.get_object_mut(page)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Annots", annots);
    let font = doc.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let form = doc.add_object(dictionary! { "Fields" => fields, "DA" => Object::string_literal("/Helv 0 Tf 0 g"), "DR" => dictionary! { "Font" => dictionary! { "Helv" => font } } });
    doc.catalog_mut().unwrap().set("AcroForm", form);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        let mut d = Document::load_mem(&crate::fixture::fixture(&["A form."])).unwrap();
        sample(&mut d);
        d
    }

    #[test]
    fn a_form_s_fields_are_listed_with_where_each_is() {
        let fields = list(&doc());
        let summary: Vec<(&str, &str, usize)> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.kind.as_str(), f.widgets.len()))
            .collect();
        assert_eq!(
            summary,
            [
                ("name", "text", 1),
                ("notes", "text", 1),
                ("agree", "checkbox", 1),
                ("size", "radio", 2),
                ("colour", "combo", 1)
            ]
        );
        assert!(fields[1].multiline);
        assert_eq!(
            fields[3]
                .widgets
                .iter()
                .map(|w| w.state.clone().unwrap())
                .collect::<Vec<_>>(),
            ["small", "large"]
        );
        assert_eq!(fields[4].options, ["Red", "Blue"]);
        assert_eq!(fields[0].widgets[0].rect, [100.0, 600.0, 300.0, 620.0]);
    }

    #[test]
    fn fields_are_filled_in_drawn_and_flattened_into_their_pages() {
        let mut d = doc();
        set(&mut d, "name", "Ada (Lovelace)").unwrap();
        set(&mut d, "notes", "Two\nlines").unwrap();
        set(&mut d, "agree", "Yes").unwrap();
        set(&mut d, "size", "large").unwrap();
        set(&mut d, "colour", "Blue").unwrap();
        assert!(
            set(&mut d, "colour", "Green").is_err()
                && set(&mut d, "size", "huge").is_err()
                && set(&mut d, "nope", "x").is_err()
        );
        let values: Vec<String> = list(&d).into_iter().map(|f| f.value).collect();
        assert_eq!(
            values,
            ["Ada (Lovelace)", "Two\nlines", "Yes", "large", "Blue"]
        );
        // The radio button chosen shows on; the other off.
        let size = list(&d)[3].clone();
        let shown: Vec<String> = widgets_of(&d, find(&d, "size").unwrap())
            .into_iter()
            .map(|w| {
                String::from_utf8_lossy(dict(&d, w).unwrap().get(b"AS").unwrap().as_name().unwrap())
                    .to_string()
            })
            .collect();
        assert_eq!(
            (size.value.as_str(), shown),
            ("large", vec!["Off".to_string(), "large".to_string()])
        );

        let drawn = flatten(&mut d).unwrap();
        assert_eq!(drawn, 6, "every widget drawn into its page");
        assert!(
            list(&d).is_empty() && d.catalog().unwrap().get(b"AcroForm").is_err(),
            "the form is gone"
        );
        let mut bytes = Vec::new();
        d.save_to(&mut bytes).unwrap();
        let read = Document::load_mem(&bytes).unwrap();
        let content =
            String::from_utf8_lossy(&read.get_page_content(read.get_pages()[&1])).to_string();
        assert!(content.contains("/QuarkField0 Do"), "{content}");
        assert!(flatten(&mut d).is_err(), "nothing left to flatten");
    }
}
