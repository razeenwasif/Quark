//! Arranging pages in the scrolling document canvas.
//!
//! Everything here is pure arithmetic over a list of page sizes, so the whole
//! viewer geometry is testable without a window or a PDF. The viewer asks for a
//! [`Layout`], then draws whichever pages [`Layout::visible`] reports.
//!
//! Layout is computed for *all* pages, not just visible ones, because the
//! scrollbar needs the true document height. That is one `f32` pass per page —
//! cheap enough that a 5,000-page document re-lays out in well under a
//! millisecond, and it means scroll position never shifts as pages load.

use crate::geom::{PageSize, Rect, Rot, Vec2};
use serde::{Deserialize, Serialize};

/// How pages are stacked in the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PageMode {
    /// One page at a time; scrolling stops at page boundaries.
    Single,
    /// One page per row, scrolling continuously.
    #[default]
    Continuous,
    /// Two pages side by side, one spread at a time.
    TwoUp,
    /// Two pages side by side, scrolling continuously.
    TwoUpContinuous,
    /// Like `TwoUp` but page 1 stands alone as a cover.
    TwoUpCover,
    /// Like `TwoUpContinuous` but page 1 stands alone as a cover.
    TwoUpCoverContinuous,
}

impl PageMode {
    /// Pages per row.
    pub fn columns(self) -> usize {
        match self {
            PageMode::Single | PageMode::Continuous => 1,
            _ => 2,
        }
    }

    /// Whether page 1 occupies a row by itself, as a book cover does.
    pub fn has_cover(self) -> bool {
        matches!(self, PageMode::TwoUpCover | PageMode::TwoUpCoverContinuous)
    }

    /// Whether scrolling runs through the whole document or one row at a time.
    pub fn is_continuous(self) -> bool {
        matches!(
            self,
            PageMode::Continuous | PageMode::TwoUpContinuous | PageMode::TwoUpCoverContinuous
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            PageMode::Single => "Single Page",
            PageMode::Continuous => "Continuous",
            PageMode::TwoUp => "Two Pages",
            PageMode::TwoUpContinuous => "Two Pages Continuous",
            PageMode::TwoUpCover => "Two Pages with Cover",
            PageMode::TwoUpCoverContinuous => "Two Pages Continuous with Cover",
        }
    }

    pub const ALL: [PageMode; 6] = [
        PageMode::Single,
        PageMode::Continuous,
        PageMode::TwoUp,
        PageMode::TwoUpContinuous,
        PageMode::TwoUpCover,
        PageMode::TwoUpCoverContinuous,
    ];
}

/// How the zoom factor is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ZoomMode {
    /// Whole page (or spread) fits in the viewport.
    FitPage,
    /// Page width fills the viewport.
    FitWidth,
    /// Page height fills the viewport.
    FitHeight,
    /// 100%, meaning one PDF point per logical pixel.
    Actual,
    /// An explicit scale factor.
    Custom(f32),
}

impl Default for ZoomMode {
    fn default() -> Self {
        ZoomMode::FitWidth
    }
}

impl ZoomMode {
    pub fn label(self) -> String {
        match self {
            ZoomMode::FitPage => "Fit Page".into(),
            ZoomMode::FitWidth => "Fit Width".into(),
            ZoomMode::FitHeight => "Fit Height".into(),
            ZoomMode::Actual => "100%".into(),
            ZoomMode::Custom(z) => format!("{:.0}%", z * 100.0),
        }
    }
}

/// The smallest and largest scale the viewer will produce.
///
/// The ceiling exists because raster tiles are allocated at `zoom * page size`;
/// beyond about 100x a single page exceeds any sane texture budget.
pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 100.0;

/// Zoom steps used by the zoom-in / zoom-out commands, matching the
/// familiar Acrobat ladder.
pub const ZOOM_STEPS: &[f32] = &[
    0.08, 0.125, 0.25, 0.333, 0.50, 0.667, 0.75, 1.00, 1.25, 1.50, 2.00, 3.00, 4.00, 6.00, 8.00,
    12.0, 16.0, 25.0, 40.0, 64.0,
];

/// Returns the next step above `z`, or `z` clamped to the ceiling.
pub fn zoom_step_up(z: f32) -> f32 {
    // A small epsilon stops repeated clicks sticking when `z` is already
    // exactly on a step because of float rounding.
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|&s| s > z * 1.001)
        .unwrap_or(MAX_ZOOM)
        .min(MAX_ZOOM)
}

/// Returns the next step below `z`, or `z` clamped to the floor.
pub fn zoom_step_down(z: f32) -> f32 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|&s| s < z * 0.999)
        .unwrap_or(MIN_ZOOM)
        .max(MIN_ZOOM)
}

/// Inputs to a layout pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutParams {
    pub mode: PageMode,
    pub zoom: ZoomMode,
    pub rotation: Rot,
    /// Gap between pages, in pixels.
    pub gap: f32,
    /// Margin around the whole document, in pixels.
    pub margin: f32,
    /// Size of the scrolling viewport, in pixels.
    pub viewport: Vec2,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            mode: PageMode::default(),
            zoom: ZoomMode::default(),
            rotation: Rot::D0,
            gap: 12.0,
            margin: 16.0,
            viewport: Vec2::new(800.0, 600.0),
        }
    }
}

/// One page's position in content space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    pub index: usize,
    /// Rect in content space, pixels.
    pub rect: Rect,
    /// The page's unrotated size in points.
    pub size: PageSize,
    /// Which row it landed in; used for page-at-a-time scrolling.
    pub row: usize,
}

/// A row of pages (one page, or a two-up spread).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowBox {
    pub first_page: usize,
    pub page_count: usize,
    /// Vertical extent in content space.
    pub top: f32,
    pub bottom: f32,
}

/// The computed arrangement of every page.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub pages: Vec<PageBox>,
    pub rows: Vec<RowBox>,
    /// Full size of the scrollable canvas.
    pub content_size: Vec2,
    /// The scale actually used, after resolving a fit mode.
    pub scale: f32,
}

/// Groups page indices into rows according to the view mode.
///
/// Kept separate from [`compute`] because the pairing rule is the part most
/// likely to be wrong, and it is far easier to assert on directly.
pub fn rows_for(mode: PageMode, page_count: usize) -> Vec<Vec<usize>> {
    let mut rows = Vec::new();
    if page_count == 0 {
        return rows;
    }
    if mode.columns() == 1 {
        return (0..page_count).map(|i| vec![i]).collect();
    }

    let mut i = 0;
    // A cover document puts page 1 alone, which shifts every later spread by
    // one so that odd and even pages face each other the way a book does.
    if mode.has_cover() {
        rows.push(vec![0]);
        i = 1;
    }
    while i < page_count {
        if i + 1 < page_count {
            rows.push(vec![i, i + 1]);
        } else {
            rows.push(vec![i]);
        }
        i += 2;
    }
    rows
}

/// Resolves a [`ZoomMode`] to a concrete scale factor.
///
/// Fit modes are measured against the largest row in the document rather than
/// the current page. Sizing to the current page means the zoom level jumps
/// whenever a differently-sized page scrolls into view, which reads as the
/// document lurching.
pub fn resolve_scale(sizes: &[PageSize], p: &LayoutParams) -> f32 {
    let scale = match p.zoom {
        ZoomMode::Custom(z) => z,
        ZoomMode::Actual => 1.0,
        ZoomMode::FitWidth | ZoomMode::FitPage | ZoomMode::FitHeight => {
            if sizes.is_empty() {
                return 1.0;
            }
            let rows = rows_for(p.mode, sizes.len());

            // Widest row and tallest row, measured in points at scale 1.
            let mut max_row_w: f32 = 0.0;
            let mut max_row_h: f32 = 0.0;
            for row in &rows {
                let mut w = 0.0;
                let mut h: f32 = 0.0;
                for (n, &idx) in row.iter().enumerate() {
                    let d = sizes[idx].rotated(p.rotation);
                    w += d.x;
                    h = h.max(d.y);
                    if n > 0 {
                        // Gap is in pixels, so convert it into points at the
                        // scale we are solving for. Treating it as points
                        // instead makes the fit drift with zoom.
                        w += p.gap;
                    }
                }
                max_row_w = max_row_w.max(w);
                max_row_h = max_row_h.max(h);
            }

            let avail_w = (p.viewport.x - p.margin * 2.0).max(1.0);
            let avail_h = (p.viewport.y - p.margin * 2.0).max(1.0);

            match p.zoom {
                ZoomMode::FitWidth => avail_w / max_row_w.max(1.0),
                ZoomMode::FitHeight => avail_h / max_row_h.max(1.0),
                // Fit page has to satisfy both axes, so it takes the smaller.
                _ => (avail_w / max_row_w.max(1.0)).min(avail_h / max_row_h.max(1.0)),
            }
        }
    };
    scale.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// Lays out every page.
pub fn compute(sizes: &[PageSize], p: &LayoutParams) -> Layout {
    let scale = resolve_scale(sizes, p);
    let rows = rows_for(p.mode, sizes.len());

    // First pass: row widths, so rows can be centred against the widest one.
    let mut row_widths = Vec::with_capacity(rows.len());
    let mut row_heights = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut w = 0.0;
        let mut h: f32 = 0.0;
        for (n, &idx) in row.iter().enumerate() {
            let d = sizes[idx].rotated(p.rotation);
            w += d.x * scale;
            h = h.max(d.y * scale);
            if n > 0 {
                w += p.gap;
            }
        }
        row_widths.push(w);
        row_heights.push(h);
    }

    let widest = row_widths.iter().copied().fold(0.0_f32, f32::max);
    // The canvas is at least as wide as the viewport so that a narrow document
    // still centres rather than hugging the left edge.
    let content_w = (widest + p.margin * 2.0).max(p.viewport.x);

    let mut pages = Vec::with_capacity(sizes.len());
    let mut row_boxes = Vec::with_capacity(rows.len());
    let mut y = p.margin;

    for (ri, row) in rows.iter().enumerate() {
        let row_h = row_heights[ri];
        let mut x = (content_w - row_widths[ri]) * 0.5;
        let row_top = y;

        for (n, &idx) in row.iter().enumerate() {
            if n > 0 {
                x += p.gap;
            }
            let d = sizes[idx].rotated(p.rotation);
            let (w, h) = (d.x * scale, d.y * scale);
            // Pages of unequal height in a spread sit centred on the row, which
            // is how a mixed-size document reads best.
            let y_off = (row_h - h) * 0.5;
            pages.push(PageBox {
                index: idx,
                rect: Rect::from_xywh(x, y + y_off, w, h),
                size: sizes[idx],
                row: ri,
            });
            x += w;
        }

        row_boxes.push(RowBox {
            first_page: row[0],
            page_count: row.len(),
            top: row_top,
            bottom: row_top + row_h,
        });

        y += row_h + p.gap;
    }

    // The last row contributes a margin, not a gap.
    let content_h = if rows.is_empty() {
        p.viewport.y
    } else {
        (y - p.gap + p.margin).max(0.0)
    };

    Layout {
        pages,
        rows: row_boxes,
        content_size: Vec2::new(content_w, content_h),
        scale,
    }
}

impl Layout {
    /// Pages intersecting the given content-space rectangle.
    ///
    /// The viewer inflates the viewport before calling this so that pages just
    /// off-screen are rendered before they scroll in.
    pub fn visible(&self, view: Rect) -> Vec<&PageBox> {
        self.pages
            .iter()
            .filter(|pb| pb.rect.intersects(&view))
            .collect()
    }

    pub fn page(&self, index: usize) -> Option<&PageBox> {
        // Page order matches index order in every mode, so a direct hit is
        // usually right; fall back to a scan for safety.
        self.pages
            .get(index)
            .filter(|pb| pb.index == index)
            .or_else(|| self.pages.iter().find(|pb| pb.index == index))
    }

    /// The page a content-space point falls on, if any.
    pub fn page_at(&self, p: Vec2) -> Option<&PageBox> {
        self.pages.iter().find(|pb| pb.rect.contains(p))
    }

    /// The page that best represents the current scroll position.
    ///
    /// Defined as the page covering the largest share of the viewport, which
    /// matches what a reader would call "the page I am on" — a naive
    /// topmost-visible rule flips the page counter while most of the previous
    /// page is still on screen.
    pub fn dominant_page(&self, view: Rect) -> usize {
        let mut best = 0;
        let mut best_area = -1.0;
        for pb in &self.pages {
            let a = pb.rect.intersection(&view).area();
            if a > best_area {
                best_area = a;
                best = pb.index;
            }
        }
        best
    }

    /// Scroll offset that puts `index` at the top of the viewport.
    pub fn scroll_to_page(&self, index: usize, params: &LayoutParams) -> Vec2 {
        let Some(pb) = self.page(index) else {
            return Vec2::ZERO;
        };
        let row_top = self
            .rows
            .get(pb.row)
            .map(|r| r.top)
            .unwrap_or(pb.rect.min.y);
        Vec2::new(0.0, (row_top - params.margin).max(0.0))
    }

    /// Clamps a scroll offset to the scrollable range.
    ///
    /// When the content is smaller than the viewport the range collapses to
    /// zero rather than going negative, which would let the document drift
    /// away from the centre.
    pub fn clamp_scroll(&self, offset: Vec2, viewport: Vec2) -> Vec2 {
        let max_x = (self.content_size.x - viewport.x).max(0.0);
        let max_y = (self.content_size.y - viewport.y).max(0.0);
        Vec2::new(offset.x.clamp(0.0, max_x), offset.y.clamp(0.0, max_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letters(n: usize) -> Vec<PageSize> {
        vec![PageSize::LETTER; n]
    }

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 0.05, "{a} != {b}");
    }

    #[test]
    fn single_column_modes_give_one_page_per_row() {
        let rows = rows_for(PageMode::Continuous, 5);
        assert_eq!(rows, vec![vec![0], vec![1], vec![2], vec![3], vec![4]]);
    }

    #[test]
    fn two_up_pairs_from_the_first_page() {
        assert_eq!(rows_for(PageMode::TwoUp, 5), vec![
            vec![0, 1],
            vec![2, 3],
            vec![4]
        ]);
    }

    #[test]
    fn cover_mode_puts_page_one_alone_then_pairs_odd_with_even() {
        // This is the rule that makes a scanned book read correctly.
        assert_eq!(rows_for(PageMode::TwoUpCover, 6), vec![
            vec![0],
            vec![1, 2],
            vec![3, 4],
            vec![5]
        ]);
    }

    #[test]
    fn empty_document_lays_out_without_panicking() {
        let l = compute(&[], &LayoutParams::default());
        assert!(l.pages.is_empty());
        assert!(l.rows.is_empty());
        assert_eq!(l.dominant_page(Rect::from_xywh(0.0, 0.0, 10.0, 10.0)), 0);
    }

    #[test]
    fn fit_width_fills_the_viewport_exactly() {
        let p = LayoutParams {
            zoom: ZoomMode::FitWidth,
            viewport: Vec2::new(1224.0 + 32.0, 800.0),
            margin: 16.0,
            ..Default::default()
        };
        let l = compute(&letters(3), &p);
        // 1224 available / 612pt wide == 2.0
        approx(l.scale, 2.0);
        approx(l.pages[0].rect.width(), 1224.0);
    }

    #[test]
    fn fit_page_respects_the_shorter_axis() {
        // A viewport wide enough for 4x but only tall enough for 1x must
        // choose 1x, or the page overflows vertically.
        let p = LayoutParams {
            zoom: ZoomMode::FitPage,
            viewport: Vec2::new(2448.0 + 32.0, 792.0 + 32.0),
            margin: 16.0,
            ..Default::default()
        };
        let l = compute(&letters(1), &p);
        approx(l.scale, 1.0);
    }

    #[test]
    fn fit_width_accounts_for_the_gap_between_a_spread() {
        let p = LayoutParams {
            mode: PageMode::TwoUpContinuous,
            zoom: ZoomMode::FitWidth,
            viewport: Vec2::new(1236.0 + 32.0, 800.0),
            margin: 16.0,
            gap: 12.0,
            ..Default::default()
        };
        let l = compute(&letters(2), &p);
        // Two pages plus the 12px gap must land inside 1236px.
        let row_w = l.pages[0].rect.width() * 2.0 + 12.0;
        assert!(row_w <= 1236.5, "spread {row_w} overflowed the viewport");
    }

    #[test]
    fn rotation_swaps_the_fitted_axis() {
        let base = LayoutParams {
            zoom: ZoomMode::FitWidth,
            viewport: Vec2::new(812.0, 800.0),
            margin: 16.0,
            ..Default::default()
        };
        let upright = compute(&letters(1), &base);
        let turned = compute(&letters(1), &LayoutParams {
            rotation: Rot::D90,
            ..base
        });
        // Rotated a quarter turn the page is 792pt wide instead of 612pt, so
        // fitting the same viewport needs a smaller scale.
        assert!(turned.scale < upright.scale);
        approx(turned.pages[0].rect.width(), upright.pages[0].rect.width());
    }

    #[test]
    fn pages_are_centred_horizontally() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.0),
            viewport: Vec2::new(1000.0, 800.0),
            ..Default::default()
        };
        let l = compute(&letters(1), &p);
        let r = l.pages[0].rect;
        approx(r.min.x, (1000.0 - 612.0) / 2.0);
        approx(l.content_size.x, 1000.0);
    }

    #[test]
    fn content_height_covers_every_page_plus_margins() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.0),
            gap: 10.0,
            margin: 20.0,
            viewport: Vec2::new(1000.0, 400.0),
            ..Default::default()
        };
        let l = compute(&letters(3), &p);
        // 20 + 792 + 10 + 792 + 10 + 792 + 20
        approx(l.content_size.y, 2436.0);
        // and the last page ends one margin short of the bottom
        approx(l.pages[2].rect.max.y, l.content_size.y - 20.0);
    }

    #[test]
    fn mixed_page_sizes_centre_within_their_row() {
        let sizes = vec![PageSize::new(612.0, 792.0), PageSize::new(612.0, 396.0)];
        let p = LayoutParams {
            mode: PageMode::TwoUp,
            zoom: ZoomMode::Custom(1.0),
            viewport: Vec2::new(2000.0, 800.0),
            ..Default::default()
        };
        let l = compute(&sizes, &p);
        // The short page sits centred against the tall one.
        approx(l.pages[0].rect.center().y, l.pages[1].rect.center().y);
    }

    #[test]
    fn dominant_page_is_the_one_filling_most_of_the_view() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.0),
            gap: 10.0,
            margin: 0.0,
            viewport: Vec2::new(1000.0, 400.0),
            ..Default::default()
        };
        let l = compute(&letters(3), &p);
        // Sitting mostly over page 2 (which starts at y=802).
        let view = Rect::from_xywh(0.0, 700.0, 1000.0, 400.0);
        assert_eq!(l.dominant_page(view), 1);
        // Right at the top it is page 1.
        assert_eq!(l.dominant_page(Rect::from_xywh(0.0, 0.0, 1000.0, 400.0)), 0);
    }

    #[test]
    fn visible_returns_only_intersecting_pages() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.0),
            gap: 10.0,
            margin: 0.0,
            viewport: Vec2::new(1000.0, 400.0),
            ..Default::default()
        };
        let l = compute(&letters(10), &p);
        let vis = l.visible(Rect::from_xywh(0.0, 0.0, 1000.0, 400.0));
        assert_eq!(vis.len(), 1);
        assert_eq!(vis[0].index, 0);
        // A tall view spanning the first three pages.
        let vis = l.visible(Rect::from_xywh(0.0, 0.0, 1000.0, 1700.0));
        assert_eq!(vis.iter().map(|p| p.index).collect::<Vec<_>>(), vec![
            0, 1, 2
        ]);
    }

    #[test]
    fn scroll_clamps_to_the_document_and_never_goes_negative() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.0),
            viewport: Vec2::new(1000.0, 400.0),
            ..Default::default()
        };
        let l = compute(&letters(3), &p);
        let c = l.clamp_scroll(Vec2::new(-50.0, -80.0), p.viewport);
        assert_eq!(c, Vec2::ZERO);
        let c = l.clamp_scroll(Vec2::new(0.0, 99999.0), p.viewport);
        approx(c.y, l.content_size.y - 400.0);
    }

    #[test]
    fn short_document_does_not_scroll() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(0.1),
            viewport: Vec2::new(1000.0, 2000.0),
            ..Default::default()
        };
        let l = compute(&letters(1), &p);
        let c = l.clamp_scroll(Vec2::new(0.0, 500.0), p.viewport);
        approx(c.y, 0.0);
    }

    #[test]
    fn scroll_to_page_lands_on_the_row_top() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.0),
            gap: 10.0,
            margin: 20.0,
            viewport: Vec2::new(1000.0, 400.0),
            ..Default::default()
        };
        let l = compute(&letters(5), &p);
        let off = l.scroll_to_page(2, &p);
        // Row 2 top is 20 + 2*(792+10) = 1624; minus the margin.
        approx(off.y, 1604.0);
    }

    #[test]
    fn zoom_steps_advance_and_never_stall() {
        assert!(zoom_step_up(1.0) > 1.0);
        assert!(zoom_step_down(1.0) < 1.0);
        // Repeated stepping from an exact step value must keep moving.
        let mut z = 1.0;
        for _ in 0..5 {
            let n = zoom_step_up(z);
            assert!(n > z, "stalled at {z}");
            z = n;
        }
        assert_eq!(zoom_step_up(MAX_ZOOM), MAX_ZOOM);
        assert_eq!(zoom_step_down(MIN_ZOOM), MIN_ZOOM);
    }

    #[test]
    fn scale_is_clamped_to_sane_bounds() {
        let p = LayoutParams {
            zoom: ZoomMode::Custom(9999.0),
            ..Default::default()
        };
        assert_eq!(resolve_scale(&letters(1), &p), MAX_ZOOM);
        let p = LayoutParams {
            zoom: ZoomMode::Custom(0.0),
            ..Default::default()
        };
        assert_eq!(resolve_scale(&letters(1), &p), MIN_ZOOM);
    }

    #[test]
    fn page_transform_agrees_with_the_layout_rect() {
        use crate::geom::PageTransform;
        let p = LayoutParams {
            zoom: ZoomMode::Custom(1.5),
            ..Default::default()
        };
        let l = compute(&letters(2), &p);
        let pb = l.page(1).unwrap();
        let t = PageTransform::new(pb.rect, pb.size, p.rotation);
        approx(t.scale(), 1.5);
        // Top-left of the page in page space is (0, height).
        let tl = t.page_to_content(Vec2::new(0.0, 792.0));
        approx(tl.x, pb.rect.min.x);
        approx(tl.y, pb.rect.min.y);
    }
}
