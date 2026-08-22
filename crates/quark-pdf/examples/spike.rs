//! Proves the PDFium chain end to end: bind, load, render, extract, search.
use pdfium_render::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let lib = std::env::var("QUARK_PDFIUM")
        .unwrap_or_else(|_| "vendor/pdfium-linux/libpdfium.so".to_owned());
    let bindings = Pdfium::bind_to_library(&lib)?;
    let pdfium = Pdfium::new(bindings);
    println!("bound to {lib}");

    let doc = pdfium.load_pdf_from_file("/tmp/quark-test.pdf", None)?;
    println!("version: {:?}", doc.version());
    println!("pages:   {}", doc.pages().len());

    for (i, page) in doc.pages().iter().enumerate() {
        let size = page.page_size();
        println!(
            "page {i}: {:.1} x {:.1} pt",
            size.width().value,
            size.height().value
        );

        let text = page.text()?;
        let all = text.all();
        println!("  text: {:?}", all.chars().take(60).collect::<String>());

        let cfg = PdfRenderConfig::new().set_target_width(400);
        let bitmap = page.render_with_config(&cfg)?;
        let rgba = bitmap.as_rgba_bytes();
        let (w, h) = (bitmap.width(), bitmap.height());
        let non_white = rgba
            .chunks_exact(4)
            .filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250)
            .count();
        println!("  raster: {w}x{h}, {} bytes, {non_white} non-white px", rgba.len());
    }

    // Search across the document.
    let page = doc.pages().get(1)?;
    let text = page.text()?;
    let search = text.search("findable-token", &PdfSearchOptions::new())?;
    let hits = search.iter(PdfSearchDirection::SearchForward).count();
    println!("search 'findable-token' on page 1: {hits} hit(s)");

    Ok(())
}
