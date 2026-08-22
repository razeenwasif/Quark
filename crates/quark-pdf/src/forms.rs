//! Interactive forms (AcroForm).
//!
//! Form fields are `/Widget` annotations with extra structure. PDFium exposes
//! them as typed field objects, which is what makes filling and reading values
//! practical; the widget geometry comes back in page space so the UI can put a
//! real text box over the right part of the page.

use std::collections::BTreeMap;

use pdfium_render::prelude::*;
use quark_core::geom::{Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::engine::EngineError;

/// The kinds of field Quark presents in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FieldKind {
    Text,
    Checkbox,
    RadioButton,
    ComboBox,
    ListBox,
    PushButton,
    Signature,
    #[default]
    Unknown,
}

impl FieldKind {
    pub fn label(self) -> &'static str {
        match self {
            FieldKind::Text => "Text Field",
            FieldKind::Checkbox => "Check Box",
            FieldKind::RadioButton => "Radio Button",
            FieldKind::ComboBox => "Dropdown",
            FieldKind::ListBox => "List Box",
            FieldKind::PushButton => "Button",
            FieldKind::Signature => "Signature",
            FieldKind::Unknown => "Field",
        }
    }

    /// Whether the field holds a value the user can type or choose.
    ///
    /// Push buttons and signatures are excluded: neither has a value to export,
    /// and offering an edit box for them is meaningless.
    pub fn is_fillable(self) -> bool {
        matches!(
            self,
            FieldKind::Text
                | FieldKind::Checkbox
                | FieldKind::RadioButton
                | FieldKind::ComboBox
                | FieldKind::ListBox
        )
    }
}

/// One form field, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    /// Fully qualified field name, which is what form data is keyed by.
    pub name: String,
    pub kind: FieldKind,
    pub page: usize,
    /// The widget's rectangle in page space.
    pub rect: Rect,
    pub value: String,
    /// Choices, for combo and list boxes.
    pub options: Vec<String>,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
    pub password: bool,
    /// Index of the widget annotation on its page, for editing in place.
    pub annot_index: usize,
}

impl FormField {
    /// Whether the field has been filled in.
    pub fn is_filled(&self) -> bool {
        match self.kind {
            FieldKind::Checkbox | FieldKind::RadioButton => {
                // "Off" is the PDF convention for an unticked box, so a literal
                // value of "Off" is not a filled field.
                !self.value.is_empty() && self.value != "Off"
            }
            _ => !self.value.trim().is_empty(),
        }
    }

    /// Whether this field must be filled before the form is complete.
    pub fn is_missing_required(&self) -> bool {
        self.required && !self.read_only && !self.is_filled()
    }
}

fn rect_from_pdf(r: PdfRect) -> Rect {
    Rect::new(
        Vec2::new(r.left().value, r.bottom().value),
        Vec2::new(r.right().value, r.top().value),
    )
    .normalize()
}

fn kind_of(t: PdfFormFieldType) -> FieldKind {
    match t {
        PdfFormFieldType::Text => FieldKind::Text,
        PdfFormFieldType::Checkbox => FieldKind::Checkbox,
        PdfFormFieldType::RadioButton => FieldKind::RadioButton,
        PdfFormFieldType::ComboBox => FieldKind::ComboBox,
        PdfFormFieldType::ListBox => FieldKind::ListBox,
        PdfFormFieldType::PushButton => FieldKind::PushButton,
        PdfFormFieldType::Signature => FieldKind::Signature,
        _ => FieldKind::Unknown,
    }
}

/// Reads every form field in the document, in page order.
pub fn read_fields(doc: &Document) -> Result<Vec<FormField>, EngineError> {
    let mut out = Vec::new();
    let pages = doc.inner().pages();

    for (page_index, page) in pages.iter().enumerate() {
        for (annot_index, annot) in page.annotations().iter().enumerate() {
            if annot.annotation_type() != PdfPageAnnotationType::Widget {
                continue;
            }
            let Some(field) = annot.as_form_field() else {
                continue;
            };

            let kind = kind_of(field.field_type());
            let rect = annot.bounds().map(rect_from_pdf).unwrap_or(Rect::ZERO);

            let (value, options, multiline, password) = match field {
                PdfFormField::Text(t) => (
                    t.value().unwrap_or_default(),
                    Vec::new(),
                    t.is_multiline(),
                    t.is_password(),
                ),
                PdfFormField::Checkbox(c) => (
                    // A ticked box exports its /AS name; an unticked one is Off.
                    if c.is_checked().unwrap_or(false) {
                        c.group_value().unwrap_or_else(|| "Yes".into())
                    } else {
                        "Off".into()
                    },
                    Vec::new(),
                    false,
                    false,
                ),
                PdfFormField::RadioButton(r) => (
                    if r.is_checked().unwrap_or(false) {
                        r.group_value().unwrap_or_else(|| "Yes".into())
                    } else {
                        "Off".into()
                    },
                    Vec::new(),
                    false,
                    false,
                ),
                PdfFormField::ComboBox(c) => (
                    c.value().unwrap_or_default(),
                    c.options().iter().filter_map(|o| o.label().cloned()).collect(),
                    false,
                    false,
                ),
                PdfFormField::ListBox(l) => (
                    l.value().unwrap_or_default(),
                    l.options().iter().filter_map(|o| o.label().cloned()).collect(),
                    false,
                    false,
                ),
                _ => (String::new(), Vec::new(), false, false),
            };

            out.push(FormField {
                name: field.name().unwrap_or_default(),
                kind,
                page: page_index,
                rect,
                value,
                options,
                read_only: field.is_read_only(),
                required: field.is_required(),
                multiline,
                password,
                annot_index,
            });
        }
    }
    Ok(out)
}

/// Sets a text field's value by name.
pub fn set_text_value(doc: &mut Document, name: &str, value: &str) -> Result<(), EngineError> {
    let pages = doc.inner_mut().pages_mut();
    for mut page in pages.iter() {
        let annotations = page.annotations_mut();
        for i in 0..annotations.len() {
            let Ok(mut annot) = annotations.get(i) else {
                continue;
            };
            if annot.annotation_type() != PdfPageAnnotationType::Widget {
                continue;
            }
            let matches = annot
                .as_form_field()
                .and_then(|f| f.name())
                .is_some_and(|n| n == name);
            if !matches {
                continue;
            }
            if let Some(PdfFormField::Text(t)) = annot.as_form_field_mut() {
                return t.set_value(value).map_err(EngineError::from);
            }
        }
    }
    Err(EngineError::Pdfium(format!("no text field named {name:?}")))
}

/// Ticks or unticks a checkbox by name.
pub fn set_checkbox(doc: &mut Document, name: &str, checked: bool) -> Result<(), EngineError> {
    let pages = doc.inner_mut().pages_mut();
    for mut page in pages.iter() {
        let annotations = page.annotations_mut();
        for i in 0..annotations.len() {
            let Ok(mut annot) = annotations.get(i) else {
                continue;
            };
            if annot.annotation_type() != PdfPageAnnotationType::Widget {
                continue;
            }
            let matches = annot
                .as_form_field()
                .and_then(|f| f.name())
                .is_some_and(|n| n == name);
            if !matches {
                continue;
            }
            if let Some(PdfFormField::Checkbox(c)) = annot.as_form_field_mut() {
                return c.set_checked(checked).map_err(EngineError::from);
            }
        }
    }
    Err(EngineError::Pdfium(format!("no checkbox named {name:?}")))
}

/// Every field's current value, keyed by name.
pub fn field_values(doc: &Document) -> BTreeMap<String, String> {
    read_fields(doc)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.kind.is_fillable())
        .map(|f| (f.name, f.value))
        .collect()
}

/// Serialises form data as JSON.
///
/// JSON rather than FDF because it is what anything downstream — a script, a
/// spreadsheet, a web form — can actually read without a PDF library.
/// [`export_xfdf`] covers interchange with other PDF tools.
pub fn export_json(doc: &Document) -> Result<String, EngineError> {
    serde_json::to_string_pretty(&field_values(doc))
        .map_err(|e| EngineError::Pdfium(format!("could not serialise form data: {e}")))
}

/// Serialises form data as XFDF, the XML interchange format other PDF tools read.
pub fn export_xfdf(doc: &Document) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n  <fields>\n",
    );
    for (name, value) in field_values(doc) {
        s.push_str(&format!(
            "    <field name=\"{}\">\n      <value>{}</value>\n    </field>\n",
            xml_escape(&name),
            xml_escape(&value)
        ));
    }
    s.push_str("  </fields>\n</xfdf>\n");
    s
}

/// Escapes the five characters XML cannot carry literally.
///
/// A field value containing `&` or `<` — which a form asking for a company name
/// very often does — produces malformed XML without this.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Applies a name-to-value map to the document.
///
/// Returns how many fields were set. Names that are absent are skipped rather
/// than failing the whole import, because form data is routinely shared between
/// revisions of a form that have gained or lost a field.
pub fn import_values(
    doc: &mut Document,
    values: &BTreeMap<String, String>,
) -> Result<usize, EngineError> {
    let existing = read_fields(doc)?;
    let mut applied = 0;
    for (name, value) in values {
        let Some(f) = existing.iter().find(|f| &f.name == name) else {
            continue;
        };
        let ok = match f.kind {
            FieldKind::Text => set_text_value(doc, name, value).is_ok(),
            FieldKind::Checkbox => {
                let on = !matches!(value.as_str(), "" | "Off" | "off" | "false" | "0");
                set_checkbox(doc, name, on).is_ok()
            }
            _ => false,
        };
        if ok {
            applied += 1;
        }
    }
    Ok(applied)
}

/// Parses a JSON form-data file.
pub fn parse_json(text: &str) -> Result<BTreeMap<String, String>, EngineError> {
    serde_json::from_str(text)
        .map_err(|e| EngineError::Pdfium(format!("not valid form data: {e}")))
}

/// Clears every fillable field.
pub fn clear_form(doc: &mut Document) -> Result<usize, EngineError> {
    let fields = read_fields(doc)?;
    let mut cleared = 0;
    for f in fields.iter().filter(|f| f.kind.is_fillable() && !f.read_only) {
        let ok = match f.kind {
            FieldKind::Text => set_text_value(doc, &f.name, "").is_ok(),
            FieldKind::Checkbox => set_checkbox(doc, &f.name, false).is_ok(),
            _ => false,
        };
        if ok {
            cleared += 1;
        }
    }
    Ok(cleared)
}

/// Fields that are required but still empty.
pub fn missing_required(doc: &Document) -> Vec<FormField> {
    read_fields(doc)
        .unwrap_or_default()
        .into_iter()
        .filter(FormField::is_missing_required)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn a_document_without_a_form_reports_no_fields() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(read_fields(&d).unwrap().is_empty());
        assert!(field_values(&d).is_empty());
    }

    #[test]
    fn buttons_and_signatures_are_not_treated_as_fillable() {
        assert!(!FieldKind::PushButton.is_fillable());
        assert!(!FieldKind::Signature.is_fillable());
        assert!(FieldKind::Text.is_fillable());
        assert!(FieldKind::Checkbox.is_fillable());
    }

    #[test]
    fn an_unticked_checkbox_does_not_count_as_filled() {
        // "Off" is the PDF convention for unticked, not a value the user chose.
        let f = FormField {
            name: "agree".into(),
            kind: FieldKind::Checkbox,
            page: 0,
            rect: Rect::ZERO,
            value: "Off".into(),
            options: vec![],
            read_only: false,
            required: true,
            multiline: false,
            password: false,
            annot_index: 0,
        };
        assert!(!f.is_filled());
        assert!(f.is_missing_required());

        let ticked = FormField {
            value: "Yes".into(),
            ..f.clone()
        };
        assert!(ticked.is_filled());
        assert!(!ticked.is_missing_required());
    }

    #[test]
    fn whitespace_does_not_count_as_a_filled_text_field() {
        let f = FormField {
            name: "name".into(),
            kind: FieldKind::Text,
            page: 0,
            rect: Rect::ZERO,
            value: "   ".into(),
            options: vec![],
            read_only: false,
            required: true,
            multiline: false,
            password: false,
            annot_index: 0,
        };
        assert!(!f.is_filled());
    }

    #[test]
    fn a_read_only_field_is_never_reported_as_missing() {
        // The user cannot fill it, so demanding they do is a dead end.
        let f = FormField {
            name: "computed".into(),
            kind: FieldKind::Text,
            page: 0,
            rect: Rect::ZERO,
            value: String::new(),
            options: vec![],
            read_only: true,
            required: true,
            multiline: false,
            password: false,
            annot_index: 0,
        };
        assert!(!f.is_missing_required());
    }

    #[test]
    fn xml_escaping_covers_every_reserved_character() {
        assert_eq!(
            xml_escape("Smith & Sons <\"Ltd\">'"),
            "Smith &amp; Sons &lt;&quot;Ltd&quot;&gt;&apos;"
        );
    }

    #[test]
    fn xfdf_export_is_well_formed_for_an_empty_form() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::sample_pdf(), None).unwrap();
        let x = export_xfdf(&d);
        assert!(x.starts_with("<?xml"));
        assert!(x.contains("<fields>"));
        assert!(x.trim_end().ends_with("</xfdf>"));
    }

    #[test]
    fn json_form_data_round_trips() {
        let mut m = BTreeMap::new();
        m.insert("name".to_string(), "Ada".to_string());
        m.insert("agree".to_string(), "Yes".to_string());
        let json = serde_json::to_string(&m).unwrap();
        let back = parse_json(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn malformed_form_data_is_rejected_with_a_message() {
        let e = parse_json("{not json").unwrap_err();
        assert!(format!("{e}").contains("not valid form data"));
    }

    #[test]
    fn setting_a_field_that_does_not_exist_is_an_error() {
        let _g = testutil::pdfium_guard();
        let mut d = Document::open(testutil::sample_pdf(), None).unwrap();
        assert!(set_text_value(&mut d, "nonexistent", "x").is_err());
        assert!(set_checkbox(&mut d, "nonexistent", true).is_err());
    }

    #[test]
    fn importing_into_a_form_without_those_fields_applies_nothing() {
        // Skipping unknown names rather than failing is what lets form data be
        // shared between revisions of a form.
        let _g = testutil::pdfium_guard();
        let mut d = Document::open(testutil::sample_pdf(), None).unwrap();
        let mut m = BTreeMap::new();
        m.insert("absent".to_string(), "value".to_string());
        assert_eq!(import_values(&mut d, &m).unwrap(), 0);
    }
}
