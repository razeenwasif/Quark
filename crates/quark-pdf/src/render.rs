//! Rasterising pages.

use pdfium_render::prelude::*;
use quark_core::geom::Rot;
use quark_core::prefs::PageTint;

use crate::doc::Document;
use crate::engine::EngineError;

/// A rendered page: tightly packed RGBA, top-left origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    pub fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![255; (width as usize * height as usize) * 4],
        }
    }

    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y as usize * self.width as usize) + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// What to draw and how large.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderRequest {
    pub page: usize,
    /// Pixels per PDF point.
    pub scale: f32,
    /// View rotation, on top of the page's own.
    pub rotation: Rot,
    pub tint: PageTint,
    /// Draw annotations over the page content.
    pub annotations: bool,
    /// Draw interactive form fields.
    pub forms: bool,
    /// Anti-alias text and paths. Off is faster and is what thumbnails use.
    pub smooth: bool,
}

impl Default for RenderRequest {
    fn default() -> Self {
        Self {
            page: 0,
            scale: 1.0,
            rotation: Rot::D0,
            tint: PageTint::None,
            annotations: true,
            forms: true,
            smooth: true,
        }
    }
}

/// The largest raster Quark will produce for one page, in pixels.
///
/// At high zoom a full-page raster grows quadratically; an A0 poster at 6400%
/// would be several gigabytes. The viewer clamps to this and lets the scroll
/// view scale the result, which is visually indistinguishable at that zoom.
pub const MAX_RASTER_PIXELS: u64 = 64_000_000;

/// Pixel dimensions for a page at a given scale, clamped to the budget.
pub fn raster_size(page_w: f32, page_h: f32, scale: f32, rotation: Rot) -> (u32, u32) {
    let (w, h) = if rotation.is_quarter_turn() {
        (page_h, page_w)
    } else {
        (page_w, page_h)
    };
    let mut pw = (w * scale).round().max(1.0);
    let mut ph = (h * scale).round().max(1.0);

    let total = pw as u64 * ph as u64;
    if total > MAX_RASTER_PIXELS {
        // Scale both axes by the same factor so the aspect ratio survives.
        let k = (MAX_RASTER_PIXELS as f64 / total as f64).sqrt() as f32;
        pw = (pw * k).round().max(1.0);
        ph = (ph * k).round().max(1.0);
    }
    (pw as u32, ph as u32)
}

fn to_pdfium_rotation(r: Rot) -> PdfPageRenderRotation {
    match r {
        Rot::D0 => PdfPageRenderRotation::None,
        Rot::D90 => PdfPageRenderRotation::Degrees90,
        Rot::D180 => PdfPageRenderRotation::Degrees180,
        Rot::D270 => PdfPageRenderRotation::Degrees270,
    }
}

/// Renders one page.
pub fn render_page(doc: &Document, req: &RenderRequest) -> Result<Raster, EngineError> {
    let pages = doc.inner().pages();
    let page = pages
        .get(req.page as i32)
        .map_err(|_| EngineError::Pdfium(format!("no page {}", req.page)))?;

    let size = page.page_size();
    let (w, h) = raster_size(size.width().value, size.height().value, req.scale, req.rotation);

    let mut cfg = PdfRenderConfig::new()
        .set_target_size(w as i32, h as i32)
        .rotate(to_pdfium_rotation(req.rotation), false)
        .render_annotations(req.annotations)
        .set_text_smoothing(req.smooth)
        .set_image_smoothing(req.smooth)
        .set_path_smoothing(req.smooth);
    if req.forms {
        cfg = cfg.render_form_data(true);
    }

    let bitmap = page.render_with_config(&cfg).map_err(EngineError::from)?;
    let mut rgba = bitmap.as_rgba_bytes();
    let (bw, bh) = (bitmap.width() as u32, bitmap.height() as u32);

    apply_tint(&mut rgba, req.tint);

    Ok(Raster {
        width: bw,
        height: bh,
        rgba,
    })
}

/// Renders a page scaled to fit inside a square of `max_edge` pixels.
///
/// Used for thumbnails, where the exact scale does not matter but a consistent
/// bounding box does.
pub fn render_thumbnail(
    doc: &Document,
    page: usize,
    max_edge: u32,
    tint: PageTint,
) -> Result<Raster, EngineError> {
    let size = doc.info.page_size(page);
    let long = size.width.max(size.height).max(1.0);
    let scale = max_edge as f32 / long;
    render_page(doc, &RenderRequest {
        page,
        scale,
        tint,
        // Thumbnails are small enough that anti-aliasing costs little and
        // matters a lot for legibility.
        smooth: true,
        ..Default::default()
    })
}

/// Recolours a rendered page in place.
///
/// Night mode inverts *luminance* while preserving hue rather than doing a
/// straight `255 - c` inversion. A plain inversion turns every photograph into
/// a colour negative and makes a blue heading orange; keeping the hue means
/// diagrams and screenshots still read correctly.
pub fn apply_tint(rgba: &mut [u8], tint: PageTint) {
    match tint {
        PageTint::None => {}
        PageTint::Night => {
            for px in rgba.chunks_exact_mut(4) {
                let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
                let max = r.max(g).max(b);
                let min = r.min(g).min(b);
                // Lightness inversion: subtract the pixel's own lightness band
                // from white. Every channel shifts by the same amount, so the
                // differences between them — and therefore the hue — survive,
                // while white maps to black and black to white. A plain
                // `255 - c` would flip the hue too and turn every photograph
                // into a colour negative.
                let shift = 255 - max - min;
                px[0] = (r + shift).clamp(0, 255) as u8;
                px[1] = (g + shift).clamp(0, 255) as u8;
                px[2] = (b + shift).clamp(0, 255) as u8;
            }
        }
        PageTint::Sepia => {
            for px in rgba.chunks_exact_mut(4) {
                let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
                let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                // Warm paper: full white maps to a soft cream, black stays put.
                px[0] = (l * 1.00).clamp(0.0, 255.0) as u8;
                px[1] = (l * 0.94).clamp(0.0, 255.0) as u8;
                px[2] = (l * 0.82).clamp(0.0, 255.0) as u8;
            }
        }
        PageTint::Dim => {
            for px in rgba.chunks_exact_mut(4) {
                for c in 0..3 {
                    px[c] = (px[c] as f32 * 0.72) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn doc() -> Document {
        Document::open(testutil::sample_pdf(), None).unwrap()
    }

    #[test]
    fn renders_a_page_at_the_requested_scale() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let r = render_page(&d, &RenderRequest {
            page: 0,
            scale: 1.0,
            ..Default::default()
        })
        .expect("render");
        assert_eq!(r.width, 612);
        assert_eq!(r.height, 792);
        assert_eq!(r.rgba.len(), 612 * 792 * 4);
    }

    #[test]
    fn scale_multiplies_the_raster_dimensions() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let r = render_page(&d, &RenderRequest {
            page: 0,
            scale: 2.0,
            ..Default::default()
        })
        .unwrap();
        assert_eq!((r.width, r.height), (1224, 1584));
    }

    #[test]
    fn a_quarter_turn_swaps_the_raster_axes() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let r = render_page(&d, &RenderRequest {
            page: 0,
            scale: 1.0,
            rotation: Rot::D90,
            ..Default::default()
        })
        .unwrap();
        assert_eq!((r.width, r.height), (792, 612));
    }

    #[test]
    fn the_rendered_page_actually_contains_ink() {
        let _g = crate::testutil::pdfium_guard();
        // A blank raster would pass every dimension check while showing
        // nothing, so assert on the content too.
        let d = doc();
        let r = render_page(&d, &RenderRequest::default()).unwrap();
        let dark = r
            .rgba
            .chunks_exact(4)
            .filter(|p| p[0] < 200 && p[1] < 200)
            .count();
        assert!(dark > 500, "page looks blank: only {dark} dark pixels");
    }

    #[test]
    fn the_blue_rectangle_on_page_one_renders_blue() {
        let _g = crate::testutil::pdfium_guard();
        // Confirms colour survives the RGBA conversion in the right order —
        // a BGRA/RGBA mix-up would make this rectangle red.
        let d = doc();
        let r = render_page(&d, &RenderRequest::default()).unwrap();
        // The fixture fills 72,400 to 272,520 in PDF space; that is y 272..392
        // from the top. Sample the middle of it.
        let px = r.pixel(150, 330);
        assert!(
            px[2] > 150 && px[0] < 100,
            "expected blue, got {px:?}"
        );
    }

    #[test]
    fn raster_size_is_clamped_so_extreme_zoom_cannot_exhaust_memory() {
        let _g = crate::testutil::pdfium_guard();
        // A letter page at 6400% would be 1.5 gigapixels.
        let (w, h) = raster_size(612.0, 792.0, 64.0, Rot::D0);
        assert!(
            (w as u64 * h as u64) <= MAX_RASTER_PIXELS,
            "{w}x{h} exceeds the budget"
        );
        // The aspect ratio must survive the clamp.
        let ratio = w as f32 / h as f32;
        assert!((ratio - 612.0 / 792.0).abs() < 0.01, "aspect skewed to {ratio}");
    }

    #[test]
    fn raster_size_never_returns_zero() {
        let _g = crate::testutil::pdfium_guard();
        let (w, h) = raster_size(612.0, 792.0, 0.0001, Rot::D0);
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn night_mode_flips_light_and_dark() {
        let _g = crate::testutil::pdfium_guard();
        let mut px = vec![255, 255, 255, 255, 0, 0, 0, 255];
        apply_tint(&mut px, PageTint::Night);
        assert_eq!(&px[0..3], &[0, 0, 0], "white should become black");
        assert_eq!(&px[4..7], &[255, 255, 255], "black should become white");
    }

    #[test]
    fn night_mode_preserves_hue_rather_than_inverting_colour() {
        let _g = crate::testutil::pdfium_guard();
        // A saturated blue must stay blue, not turn yellow the way a naive
        // 255-c inversion would make it.
        let mut px = vec![20, 40, 200, 255];
        apply_tint(&mut px, PageTint::Night);
        let (r, g, b) = (px[0], px[1], px[2]);
        assert!(b > r && b > g, "hue was lost: {r},{g},{b}");
    }

    #[test]
    fn sepia_is_warm_and_monochrome() {
        let _g = crate::testutil::pdfium_guard();
        let mut px = vec![255, 255, 255, 255];
        apply_tint(&mut px, PageTint::Sepia);
        assert!(px[0] > px[1] && px[1] > px[2], "not warm: {px:?}");
    }

    #[test]
    fn no_tint_leaves_pixels_untouched() {
        let _g = crate::testutil::pdfium_guard();
        let orig = vec![12, 34, 56, 255];
        let mut px = orig.clone();
        apply_tint(&mut px, PageTint::None);
        assert_eq!(px, orig);
    }

    #[test]
    fn tinting_never_changes_the_alpha_channel() {
        let _g = crate::testutil::pdfium_guard();
        for t in PageTint::ALL {
            let mut px = vec![100, 150, 200, 128];
            apply_tint(&mut px, t);
            assert_eq!(px[3], 128, "{t:?} modified alpha");
        }
    }

    #[test]
    fn thumbnails_fit_inside_the_requested_box() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let r = render_thumbnail(&d, 0, 160, PageTint::None).unwrap();
        assert!(r.width <= 160 && r.height <= 160, "{}x{}", r.width, r.height);
        // A portrait page should use the full height.
        assert_eq!(r.height, 160);
    }

    #[test]
    fn rendering_a_missing_page_is_an_error_not_a_panic() {
        let _g = crate::testutil::pdfium_guard();
        let d = doc();
        let e = render_page(&d, &RenderRequest {
            page: 99,
            ..Default::default()
        });
        assert!(e.is_err());
    }
}
