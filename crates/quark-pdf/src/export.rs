//! Exporting out of PDF, creating PDFs from other formats, and page stamping.

use std::path::{Path, PathBuf};

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document as LoDocument, Object, Stream};
use quark_core::geom::PageSize;
use quark_core::prefs::PageTint;
use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::engine::EngineError;
use crate::render::{self, RenderRequest};

type Result<T> = std::result::Result<T, EngineError>;

/// Raster formats pages can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Tiff,
    Bmp,
    WebP,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Tiff => "tif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::WebP => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Tiff => "TIFF",
            ImageFormat::Bmp => "BMP",
            ImageFormat::WebP => "WebP",
        }
    }

    fn to_image_crate(self) -> image::ImageFormat {
        match self {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            ImageFormat::Tiff => image::ImageFormat::Tiff,
            ImageFormat::Bmp => image::ImageFormat::Bmp,
            ImageFormat::WebP => image::ImageFormat::WebP,
        }
    }

    /// Whether the format can carry an alpha channel.
    ///
    /// JPEG cannot, so a page exported to it must be composited onto white
    /// first or the transparent areas come out black.
    pub fn supports_alpha(self) -> bool {
        !matches!(self, ImageFormat::Jpeg)
    }

    pub const ALL: [ImageFormat; 5] = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Tiff,
        ImageFormat::Bmp,
        ImageFormat::WebP,
    ];
}

/// Settings for a page-to-image export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImageExport {
    pub format: ImageFormat,
    /// Output resolution. 72 dpi is one pixel per PDF point.
    pub dpi: f32,
    pub tint: PageTint,
    pub annotations: bool,
}

impl Default for ImageExport {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            // 150 dpi is the usual compromise: sharp enough to read on screen
            // and to print acceptably, without quadrupling the file size.
            dpi: 150.0,
            tint: PageTint::None,
            annotations: true,
        }
    }
}

/// Encodes one page as an image file.
pub fn page_to_image_bytes(
    doc: &Document,
    page: usize,
    opts: &ImageExport,
) -> Result<Vec<u8>> {
    let scale = opts.dpi / quark_core::geom::POINTS_PER_INCH;
    let raster = render::render_page(doc, &RenderRequest {
        page,
        scale,
        tint: opts.tint,
        annotations: opts.annotations,
        ..Default::default()
    })?;

    let img = image::RgbaImage::from_raw(raster.width, raster.height, raster.rgba)
        .ok_or_else(|| EngineError::Pdfium("raster size did not match its buffer".into()))?;

    let mut out = std::io::Cursor::new(Vec::new());
    if opts.format.supports_alpha() {
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, opts.format.to_image_crate())
            .map_err(|e| EngineError::Pdfium(format!("could not encode image: {e}")))?;
    } else {
        // Flatten onto white rather than dropping alpha, which would turn
        // transparent regions black.
        let mut rgb = image::RgbImage::new(img.width(), img.height());
        for (x, y, px) in img.enumerate_pixels() {
            let a = px[3] as f32 / 255.0;
            let blend = |c: u8| (c as f32 * a + 255.0 * (1.0 - a)).round() as u8;
            rgb.put_pixel(x, y, image::Rgb([blend(px[0]), blend(px[1]), blend(px[2])]));
        }
        image::DynamicImage::ImageRgb8(rgb)
            .write_to(&mut out, opts.format.to_image_crate())
            .map_err(|e| EngineError::Pdfium(format!("could not encode image: {e}")))?;
    }
    Ok(out.into_inner())
}

/// Writes every requested page as a separate image file.
///
/// Returns the paths written, so the caller can report or reveal them.
pub fn export_pages_as_images(
    doc: &Document,
    pages: &[usize],
    dir: impl AsRef<Path>,
    stem: &str,
    opts: &ImageExport,
) -> Result<Vec<PathBuf>> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)
        .map_err(|e| EngineError::Pdfium(format!("could not create {}: {e}", dir.display())))?;

    // Zero-padded to the widest page number, so the files sort correctly in
    // any file manager rather than going 1, 10, 11, 2.
    let width = doc.page_count().to_string().len().max(1);

    let mut written = Vec::new();
    for &p in pages {
        let bytes = page_to_image_bytes(doc, p, opts)?;
        let name = format!("{stem}-{:0width$}.{}", p + 1, opts.format.extension());
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|e| EngineError::Pdfium(format!("could not write {}: {e}", path.display())))?;
        written.push(path);
    }
    Ok(written)
}

/// Builds a PDF from a list of image files, one page per image.
pub fn pdf_from_images(paths: &[PathBuf], fit: PageSize) -> Result<Vec<u8>> {
    if paths.is_empty() {
        return Err(EngineError::Pdfium("no images given".into()));
    }

    let mut doc = LoDocument::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut page_ids = Vec::new();

    for path in paths {
        let img = image::open(path).map_err(|e| {
            EngineError::Pdfium(format!("could not read {}: {e}", path.display()))
        })?;
        let rgb = img.to_rgb8();
        let (iw, ih) = (rgb.width(), rgb.height());

        // Scale the image to fit the page while keeping its aspect ratio, and
        // centre it. Stretching to the page would distort every photograph.
        let sx = fit.width / iw as f32;
        let sy = fit.height / ih as f32;
        let s = sx.min(sy);
        let (dw, dh) = (iw as f32 * s, ih as f32 * s);
        let (ox, oy) = ((fit.width - dw) / 2.0, (fit.height - dh) / 2.0);

        let mut img_dict = Dictionary::new();
        img_dict.set("Type", Object::Name(b"XObject".to_vec()));
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", Object::Integer(iw as i64));
        img_dict.set("Height", Object::Integer(ih as i64));
        img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        img_dict.set("BitsPerComponent", Object::Integer(8));

        let mut stream = Stream::new(img_dict, rgb.into_raw());
        // Flate keeps the file a fraction of the raw size; without it a page
        // of photographs is tens of megabytes of uncompressed samples.
        let _ = stream.compress();
        let img_id = doc.add_object(Object::Stream(stream));

        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                // The image XObject draws into the unit square, so the CTM is
                // what gives it its size and position.
                Operation::new("cm", vec![
                    Object::Real(dw),
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(dh),
                    Object::Real(ox),
                    Object::Real(oy),
                ]),
                Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            content
                .encode()
                .map_err(|e| EngineError::Pdfium(format!("content encode failed: {e}")))?,
        ));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im0", Object::Reference(img_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set("Resources", Object::Dictionary(resources));
        page.set("MediaBox", Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(fit.width),
            Object::Real(fit.height),
        ]));
        page_ids.push(doc.add_object(page));
    }

    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Count", Object::Integer(page_ids.len() as i64));
    pages.set("Kids", Object::Array(
        page_ids.iter().map(|id| Object::Reference(*id)).collect(),
    ));
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok(out)
}

/// Where a stamp sits on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    #[default]
    Center,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    pub fn label(self) -> &'static str {
        match self {
            Anchor::TopLeft => "Top Left",
            Anchor::TopCenter => "Top Centre",
            Anchor::TopRight => "Top Right",
            Anchor::Center => "Centre",
            Anchor::BottomLeft => "Bottom Left",
            Anchor::BottomCenter => "Bottom Centre",
            Anchor::BottomRight => "Bottom Right",
        }
    }

    pub const ALL: [Anchor; 7] = [
        Anchor::TopLeft,
        Anchor::TopCenter,
        Anchor::TopRight,
        Anchor::Center,
        Anchor::BottomLeft,
        Anchor::BottomCenter,
        Anchor::BottomRight,
    ];

    /// Baseline position for a text run of the given width on a page.
    pub fn position(self, page: PageSize, text_width: f32, font_size: f32, margin: f32) -> (f32, f32) {
        let x = match self {
            Anchor::TopLeft | Anchor::BottomLeft => margin,
            Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => {
                (page.width - text_width) / 2.0
            }
            Anchor::TopRight | Anchor::BottomRight => page.width - text_width - margin,
        };
        let y = match self {
            // The baseline sits a font-size below the top edge, or the text is
            // drawn off the page.
            Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => {
                page.height - margin - font_size
            }
            Anchor::Center => (page.height - font_size) / 2.0,
            Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => margin,
        };
        (x, y)
    }
}

/// A text stamp applied to a range of pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStamp {
    pub text: String,
    pub font_size: f32,
    pub anchor: Anchor,
    /// 0.0 – 1.0.
    pub opacity: f32,
    /// Rotation in degrees, anticlockwise about the anchor point.
    pub rotation: f32,
    pub color: [f32; 3],
    pub margin: f32,
    /// Draw behind the page content rather than over it.
    pub behind: bool,
}

impl Default for TextStamp {
    fn default() -> Self {
        Self {
            text: "DRAFT".into(),
            font_size: 48.0,
            anchor: Anchor::Center,
            opacity: 0.25,
            rotation: 45.0,
            color: [0.5, 0.5, 0.5],
            margin: 36.0,
            behind: false,
        }
    }
}

/// Rough width of a Helvetica string, in points.
///
/// Real metrics would mean parsing the AFM tables; 0.5 em per character is
/// close enough for centring a watermark, and being a little off only shifts
/// it a few points.
pub fn approx_text_width(text: &str, font_size: f32) -> f32 {
    text.chars().count() as f32 * font_size * 0.5
}

/// Substitutes the tokens a header, footer or watermark may contain.
///
/// `<<n>>` is the page number, `<<N>>` the page count, `<<date>>` today's date
/// and `<<file>>` the document's name.
pub fn expand_tokens(template: &str, page: usize, total: usize, filename: &str) -> String {
    let date = quark_core::annot::now_rfc3339();
    let date = date.split('T').next().unwrap_or(&date).to_owned();
    template
        .replace("<<n>>", &(page + 1).to_string())
        .replace("<<N>>", &total.to_string())
        .replace("<<date>>", &date)
        .replace("<<file>>", filename)
}

/// Stamps text onto the given pages.
pub fn stamp_text(bytes: &[u8], pages: &[usize], stamp: &TextStamp, filename: &str) -> Result<Vec<u8>> {
    let mut doc = LoDocument::load_mem(bytes)
        .map_err(|e| EngineError::Pdfium(format!("parse failed: {e}")))?;

    let page_ids: Vec<_> = {
        let mut v: Vec<_> = doc.get_pages().into_iter().collect();
        v.sort_by_key(|(n, _)| *n);
        v.into_iter().map(|(_, id)| id).collect()
    };
    let total = page_ids.len();

    // One shared font object for every stamped page.
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = doc.add_object(Object::Dictionary(font));

    // Transparency needs an ExtGState; without it /CA and /ca have nowhere to
    // live and the stamp is fully opaque.
    let gs_id = if stamp.opacity < 1.0 {
        let mut gs = Dictionary::new();
        gs.set("Type", Object::Name(b"ExtGState".to_vec()));
        gs.set("CA", Object::Real(stamp.opacity));
        gs.set("ca", Object::Real(stamp.opacity));
        Some(doc.add_object(Object::Dictionary(gs)))
    } else {
        None
    };

    for &p in pages {
        let Some(&pid) = page_ids.get(p) else { continue };

        let size = page_media_size(&doc, pid);
        let text = expand_tokens(&stamp.text, p, total, filename);
        let w = approx_text_width(&text, stamp.font_size);
        let (x, y) = stamp.anchor.position(size, w, stamp.font_size, stamp.margin);

        let mut ops = vec![Operation::new("q", vec![])];
        if gs_id.is_some() {
            ops.push(Operation::new("gs", vec![Object::Name(b"QuarkGS".to_vec())]));
        }
        ops.push(Operation::new("BT", vec![]));
        ops.push(Operation::new("rg", vec![
            Object::Real(stamp.color[0]),
            Object::Real(stamp.color[1]),
            Object::Real(stamp.color[2]),
        ]));
        ops.push(Operation::new("Tf", vec![
            Object::Name(b"QuarkFont".to_vec()),
            Object::Real(stamp.font_size),
        ]));

        if stamp.rotation.abs() > 0.01 {
            // Rotate about the text's own centre so the stamp stays put
            // instead of swinging off the page.
            let (cx, cy) = (x + w / 2.0, y + stamp.font_size / 2.0);
            let r = stamp.rotation.to_radians();
            let (c, s) = (r.cos(), r.sin());
            let e = cx - (c * cx - s * cy);
            let f = cy - (s * cx + c * cy);
            ops.push(Operation::new("Tm", vec![
                Object::Real(c),
                Object::Real(s),
                Object::Real(-s),
                Object::Real(c),
                Object::Real(x * c - y * s + e),
                Object::Real(x * s + y * c + f),
            ]));
        } else {
            ops.push(Operation::new("Td", vec![Object::Real(x), Object::Real(y)]));
        }

        ops.push(Operation::new("Tj", vec![Object::string_literal(text)]));
        ops.push(Operation::new("ET", vec![]));
        ops.push(Operation::new("Q", vec![]));

        let encoded = Content { operations: ops }
            .encode()
            .map_err(|e| EngineError::Pdfium(format!("content encode failed: {e}")))?;

        // Register the resources this content refers to, without disturbing
        // any the page already had.
        add_page_resources(&mut doc, pid, font_id, gs_id)?;

        if stamp.behind {
            let existing = doc.get_page_content(pid);
            let mut combined = encoded;
            combined.extend_from_slice(b"\n");
            combined.extend_from_slice(&existing);
            doc.change_page_content(pid, combined)
                .map_err(|e| EngineError::Pdfium(format!("could not write content: {e}")))?;
        } else {
            let mut combined = doc.get_page_content(pid);
            combined.extend_from_slice(b"\n");
            combined.extend_from_slice(&encoded);
            doc.change_page_content(pid, combined)
                .map_err(|e| EngineError::Pdfium(format!("could not write content: {e}")))?;
        }
    }

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| EngineError::Pdfium(format!("write failed: {e}")))?;
    Ok(out)
}

/// Adds Quark's font and graphics state to a page's resource dictionary.
fn add_page_resources(
    doc: &mut LoDocument,
    pid: lopdf::ObjectId,
    font_id: lopdf::ObjectId,
    gs_id: Option<lopdf::ObjectId>,
) -> Result<()> {
    // /Resources may be inherited or held indirectly, so the existing value is
    // resolved rather than assumed to be a dictionary on the page itself.
    let existing: Dictionary = match doc
        .get_object(pid)
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Resources"))
    {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(r)) => doc
            .get_object(*r)
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_default(),
        _ => Dictionary::new(),
    };

    let mut resources = existing;

    let mut fonts = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(r)) => doc
            .get_object(*r)
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_default(),
        _ => Dictionary::new(),
    };
    fonts.set("QuarkFont", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));

    if let Some(gs) = gs_id {
        let mut states = match resources.get(b"ExtGState") {
            Ok(Object::Dictionary(d)) => d.clone(),
            Ok(Object::Reference(r)) => doc
                .get_object(*r)
                .and_then(Object::as_dict)
                .cloned()
                .unwrap_or_default(),
            _ => Dictionary::new(),
        };
        states.set("QuarkGS", Object::Reference(gs));
        resources.set("ExtGState", Object::Dictionary(states));
    }

    if let Ok(d) = doc.get_object_mut(pid).and_then(Object::as_dict_mut) {
        d.set("Resources", Object::Dictionary(resources));
    }
    Ok(())
}

fn page_media_size(doc: &LoDocument, pid: lopdf::ObjectId) -> PageSize {
    // Walks /Parent because /MediaBox is inheritable.
    let mut id = pid;
    for _ in 0..64 {
        let Ok(dict) = doc.get_object(id).and_then(Object::as_dict) else {
            break;
        };
        if let Ok(arr) = dict.get(b"MediaBox").and_then(Object::as_array) {
            let v: Vec<f32> = arr.iter().filter_map(|o| o.as_float().ok()).collect();
            if v.len() == 4 {
                return PageSize::new((v[2] - v[0]).abs(), (v[3] - v[1]).abs());
            }
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(p) => id = p,
            Err(_) => break,
        }
    }
    PageSize::LETTER
}

/// Settings for Bates numbering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatesOptions {
    pub prefix: String,
    pub suffix: String,
    pub start: u64,
    /// Zero-padded width of the number itself.
    pub digits: usize,
    pub anchor: Anchor,
    pub font_size: f32,
    pub margin: f32,
}

impl Default for BatesOptions {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            suffix: String::new(),
            start: 1,
            // Six digits is the legal-industry convention.
            digits: 6,
            anchor: Anchor::BottomRight,
            font_size: 10.0,
            margin: 24.0,
        }
    }
}

impl BatesOptions {
    /// The stamp for the nth numbered page.
    pub fn label(&self, offset: usize) -> String {
        format!(
            "{}{:0width$}{}",
            self.prefix,
            self.start + offset as u64,
            self.suffix,
            width = self.digits
        )
    }
}

/// Applies Bates numbering to the given pages, in order.
pub fn apply_bates(bytes: &[u8], pages: &[usize], opts: &BatesOptions) -> Result<Vec<u8>> {
    let mut current = bytes.to_vec();
    for (offset, &p) in pages.iter().enumerate() {
        let stamp = TextStamp {
            text: opts.label(offset),
            font_size: opts.font_size,
            anchor: opts.anchor,
            opacity: 1.0,
            rotation: 0.0,
            color: [0.0, 0.0, 0.0],
            margin: opts.margin,
            behind: false,
        };
        current = stamp_text(&current, &[p], &stamp, "")?;
    }
    Ok(current)
}

/// Writes the document's text to a file.
pub fn export_text(doc: &Document, path: impl AsRef<Path>) -> Result<()> {
    let text = crate::text::extract_all(doc)?;
    std::fs::write(path.as_ref(), text)
        .map_err(|e| EngineError::Pdfium(format!("could not write text: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn doc() -> Document {
        Document::open(testutil::sample_pdf(), None).unwrap()
    }

    #[test]
    fn jpeg_is_the_only_format_without_alpha() {
        assert!(!ImageFormat::Jpeg.supports_alpha());
        assert!(ImageFormat::Png.supports_alpha());
    }

    #[test]
    fn a_page_exports_as_a_real_png() {
        let _g = testutil::pdfium_guard();
        let bytes = page_to_image_bytes(&doc(), 0, &ImageExport::default()).unwrap();
        assert_eq!(&bytes[1..4], b"PNG", "not a PNG file");
        let img = image::load_from_memory(&bytes).expect("decodes as an image");
        // 150 dpi over a 612x792pt page.
        assert_eq!(img.width(), 1275);
    }

    #[test]
    fn dpi_controls_the_exported_resolution() {
        let _g = testutil::pdfium_guard();
        let low = page_to_image_bytes(&doc(), 0, &ImageExport {
            dpi: 72.0,
            ..Default::default()
        })
        .unwrap();
        let img = image::load_from_memory(&low).unwrap();
        assert_eq!(img.width(), 612, "72 dpi should be one pixel per point");
    }

    #[test]
    fn a_jpeg_export_is_flattened_onto_white_not_black() {
        let _g = testutil::pdfium_guard();
        let bytes = page_to_image_bytes(&doc(), 0, &ImageExport {
            format: ImageFormat::Jpeg,
            dpi: 72.0,
            ..Default::default()
        })
        .unwrap();
        let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
        // A blank corner of the page must be white.
        let px = img.get_pixel(5, 5);
        assert!(px[0] > 240 && px[1] > 240, "corner is {px:?}, expected white");
    }

    #[test]
    fn exported_filenames_are_zero_padded_so_they_sort() {
        let _g = testutil::pdfium_guard();
        let d = Document::open(testutil::many_page_pdf(12), None).unwrap();
        let dir = testutil::tmp_path("img-export");
        let paths = export_pages_as_images(
            &d,
            &[0, 9],
            &dir,
            "page",
            &ImageExport {
                dpi: 36.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(paths.len(), 2);
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names[0], "page-01.png", "should pad to the page count width");
        assert_eq!(names[1], "page-10.png");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_pdf_built_from_images_has_one_page_per_image() {
        let _g = testutil::pdfium_guard();
        // Two small images of different aspect ratios.
        let mut paths = Vec::new();
        for (i, (w, h)) in [(100u32, 50u32), (50, 100)].iter().enumerate() {
            let img = image::RgbImage::from_fn(*w, *h, |x, _| {
                if x < w / 2 {
                    image::Rgb([220, 40, 40])
                } else {
                    image::Rgb([40, 40, 220])
                }
            });
            let p = testutil::tmp_path(&format!("src-{i}.png"));
            img.save(&p).unwrap();
            paths.push(p);
        }

        let bytes = pdf_from_images(&paths, PageSize::LETTER).unwrap();
        let d = Document::from_bytes(bytes, None).expect("PDFium rejected the built PDF");
        assert_eq!(d.page_count(), 2);
        let s = d.info.page_size(0);
        assert!((s.width - 612.0).abs() < 1.0);

        // The image must actually be drawn, not just referenced.
        let r = crate::render::render_page(&d, &RenderRequest::default()).unwrap();
        let coloured = r
            .rgba
            .chunks_exact(4)
            .filter(|p| p[0] > 150 && p[1] < 100)
            .count();
        assert!(coloured > 100, "the image did not render: {coloured} red px");

        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn building_a_pdf_from_no_images_is_an_error() {
        assert!(pdf_from_images(&[], PageSize::LETTER).is_err());
    }

    #[test]
    fn anchors_place_text_inside_the_page() {
        let page = PageSize::LETTER;
        for a in Anchor::ALL {
            let (x, y) = a.position(page, 100.0, 12.0, 36.0);
            assert!(x >= 0.0 && x + 100.0 <= page.width, "{a:?} x={x}");
            assert!(y >= 0.0 && y + 12.0 <= page.height, "{a:?} y={y}");
        }
    }

    #[test]
    fn a_top_anchor_leaves_room_for_the_glyph_height() {
        // Placing the baseline at the very top edge draws the text off-page.
        let (_, y) = Anchor::TopLeft.position(PageSize::LETTER, 50.0, 24.0, 0.0);
        assert!(y <= 792.0 - 24.0);
    }

    #[test]
    fn tokens_expand_to_page_numbers_and_counts() {
        let s = expand_tokens("Page <<n>> of <<N>> — <<file>>", 4, 12, "report.pdf");
        assert_eq!(s, "Page 5 of 12 — report.pdf");
    }

    #[test]
    fn the_date_token_expands_to_an_iso_date() {
        let s = expand_tokens("<<date>>", 0, 1, "");
        assert_eq!(s.len(), 10, "expected YYYY-MM-DD, got {s:?}");
        assert_eq!(s.matches('-').count(), 2);
    }

    #[test]
    fn a_watermark_lands_in_the_file_and_still_renders() {
        let _g = testutil::pdfium_guard();
        let out = stamp_text(
            &testutil::sample_bytes(),
            &[0],
            &TextStamp::default(),
            "test.pdf",
        )
        .unwrap();
        let d = Document::from_bytes(out, None).expect("PDFium rejected the stamped file");
        assert_eq!(d.page_count(), 2);
        let t = crate::text::extract_page(&d, 0).unwrap();
        assert!(t.text.contains("DRAFT"), "watermark text missing: {:?}", t.text);
        // The original content must survive.
        assert!(t.text.contains("Quark PDF Engine"));
    }

    #[test]
    fn a_watermark_only_touches_the_pages_it_was_given() {
        let _g = testutil::pdfium_guard();
        let out = stamp_text(
            &testutil::sample_bytes(),
            &[0],
            &TextStamp::default(),
            "test.pdf",
        )
        .unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        let p2 = crate::text::extract_page(&d, 1).unwrap();
        assert!(!p2.text.contains("DRAFT"), "page 2 was stamped too");
    }

    #[test]
    fn a_header_can_carry_a_page_number_per_page() {
        let _g = testutil::pdfium_guard();
        let stamp = TextStamp {
            text: "Page <<n>> of <<N>>".into(),
            font_size: 10.0,
            anchor: Anchor::TopRight,
            opacity: 1.0,
            rotation: 0.0,
            color: [0.0, 0.0, 0.0],
            margin: 24.0,
            behind: false,
        };
        let out = stamp_text(&testutil::sample_bytes(), &[0, 1], &stamp, "f.pdf").unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        assert!(crate::text::extract_page(&d, 0).unwrap().text.contains("Page 1 of 2"));
        assert!(crate::text::extract_page(&d, 1).unwrap().text.contains("Page 2 of 2"));
    }

    #[test]
    fn bates_numbers_are_padded_and_sequential() {
        let o = BatesOptions {
            prefix: "ACME".into(),
            start: 42,
            digits: 6,
            ..Default::default()
        };
        assert_eq!(o.label(0), "ACME000042");
        assert_eq!(o.label(1), "ACME000043");
    }

    #[test]
    fn bates_numbering_stamps_every_page_in_sequence() {
        let _g = testutil::pdfium_guard();
        let opts = BatesOptions {
            prefix: "DOC".into(),
            start: 1,
            digits: 4,
            ..Default::default()
        };
        let out = apply_bates(&testutil::sample_bytes(), &[0, 1], &opts).unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        assert!(crate::text::extract_page(&d, 0).unwrap().text.contains("DOC0001"));
        assert!(crate::text::extract_page(&d, 1).unwrap().text.contains("DOC0002"));
    }

    #[test]
    fn stamping_preserves_a_pages_existing_resources() {
        // Overwriting /Resources would strip the page's own fonts and the
        // original text would stop rendering.
        let _g = testutil::pdfium_guard();
        let out = stamp_text(
            &testutil::sample_bytes(),
            &[0],
            &TextStamp::default(),
            "f.pdf",
        )
        .unwrap();
        let d = Document::from_bytes(out, None).unwrap();
        let r = crate::render::render_page(&d, &RenderRequest::default()).unwrap();
        let dark = r
            .rgba
            .chunks_exact(4)
            .filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128)
            .count();
        assert!(dark > 500, "original content stopped rendering");
    }

    #[test]
    fn text_export_writes_every_page() {
        let _g = testutil::pdfium_guard();
        let p = testutil::tmp_path("export.txt");
        export_text(&doc(), &p).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("Quark PDF Engine"));
        assert!(s.contains("Second page heading"));
        let _ = std::fs::remove_file(p);
    }
}
