//! A PDF to test with, made here: no binary fixture in the repository.

/// Build a small real PDF, for tests.
///
/// Public behind `cfg(test)`-shaped gating would not reach the integration
/// tests, and mocking the extractor would test the mock. One page per string,
/// with the text drawn on it — which is what makes it possible to assert that
/// page three's words come back as page three.
#[cfg(any(test, feature = "fixture"))]
pub fn fixture(pages: &[&str]) -> Vec<u8> {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.5");
    let tree = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font },
    });

    let mut ids = Vec::new();
    for text in pages {
        // One `Tj` per line, moved down the page, so the extracted text keeps
        // the lines the caller wrote.
        let mut ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 14.into()]),
            Operation::new("Td", vec![72.into(), 720.into()]),
        ];
        for line in text.lines() {
            ops.push(Operation::new("Tj", vec![Object::string_literal(line)]));
            ops.push(Operation::new("Td", vec![0.into(), (-18).into()]));
        }
        ops.push(Operation::new("ET", vec![]));

        let content = doc.add_object(Stream::new(
            dictionary! {},
            Content { operations: ops }.encode().unwrap(),
        ));
        ids.push(
            doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => tree,
                "Contents" => content,
            })
            .into(),
        );
    }

    doc.objects.insert(
        tree,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => ids.len() as i64,
            "Kids" => ids,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => tree });
    doc.trailer.set("Root", catalog);

    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}
