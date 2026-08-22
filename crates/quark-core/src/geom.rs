//! Geometry shared by the viewer and the PDF engine.
//!
//! Three coordinate spaces appear throughout Quark and confusing them is the
//! source of most viewer bugs, so they are named consistently everywhere:
//!
//! * **Page space** — PDF user units (points, 1/72"), origin at the page's
//!   bottom-left, y growing *upwards*. This is what the file stores.
//! * **Content space** — pixels in the scrolling document canvas, origin at the
//!   top-left of the whole laid-out document, y growing *downwards*. Page
//!   rectangles from [`crate::layout`] live here.
//! * **Screen space** — content space translated by the current scroll offset.
//!
//! The y-axis flip between page and content space is the part that is easy to
//! get wrong, so it lives in exactly one place: [`PageTransform`].

use serde::{Deserialize, Serialize};

/// One PDF point: 1/72 of an inch.
pub const POINTS_PER_INCH: f32 = 72.0;

/// A 2D vector / size, in whichever space the context implies.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn splat(v: f32) -> Self {
        Self { x: v, y: v }
    }

    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y))
    }

    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y))
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Self;
    fn div(self, s: f32) -> Self {
        // Dividing by zero would produce infinities that then propagate into
        // a scroll offset and strand the viewport somewhere unreachable.
        if s.abs() < f32::EPSILON {
            return Self::ZERO;
        }
        Self::new(self.x / s, self.y / s)
    }
}

/// An axis-aligned rectangle stored as two corners.
///
/// In content and screen space `min` is the top-left. In page space `min` is
/// the bottom-left, because page space has y growing upwards; [`Self::normalize`]
/// is what reconciles the two when a rect is built from a drag.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub const ZERO: Self = Self {
        min: Vec2::ZERO,
        max: Vec2::ZERO,
    };

    pub const fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self::new(Vec2::new(x, y), Vec2::new(x + w, y + h))
    }

    /// Reorders the corners so `min` really is componentwise smallest.
    ///
    /// A rectangle dragged up-and-left has `max` above `min`, and every
    /// intersection test below assumes otherwise.
    pub fn normalize(self) -> Self {
        Self::new(self.min.min(self.max), self.min.max(self.max))
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width(), self.height())
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    pub fn area(&self) -> f32 {
        (self.width() * self.height()).max(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.min.x < o.max.x && o.min.x < self.max.x && self.min.y < o.max.y && o.min.y < self.max.y
    }

    pub fn intersection(&self, o: &Rect) -> Rect {
        Rect::new(self.min.max(o.min), self.max.min(o.max))
    }

    /// Smallest rect containing both. A zero-area rect still contributes its
    /// position, which is what makes this correct for growing a bounding box
    /// from a single starting point.
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(self.min.min(o.min), self.max.max(o.max))
    }

    pub fn translate(&self, d: Vec2) -> Rect {
        Rect::new(self.min + d, self.max + d)
    }

    pub fn scale(&self, s: f32) -> Rect {
        Rect::new(self.min * s, self.max * s)
    }

    /// Grows the rect by `d` on every side (shrinks when negative).
    pub fn expand(&self, d: f32) -> Rect {
        Rect::new(
            Vec2::new(self.min.x - d, self.min.y - d),
            Vec2::new(self.max.x + d, self.max.y + d),
        )
    }
}

/// View rotation, in 90-degree steps. Distinct from a page's own `/Rotate`
/// entry: this is what the user applied on screen and is never saved unless
/// they explicitly rotate the *page*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rot {
    #[default]
    D0,
    D90,
    D180,
    D270,
}

impl Rot {
    pub fn degrees(self) -> i32 {
        match self {
            Rot::D0 => 0,
            Rot::D90 => 90,
            Rot::D180 => 180,
            Rot::D270 => 270,
        }
    }

    pub fn from_degrees(d: i32) -> Self {
        match d.rem_euclid(360) {
            90 => Rot::D90,
            180 => Rot::D180,
            270 => Rot::D270,
            _ => Rot::D0,
        }
    }

    pub fn cw(self) -> Self {
        Self::from_degrees(self.degrees() + 90)
    }

    pub fn ccw(self) -> Self {
        Self::from_degrees(self.degrees() - 90)
    }

    pub fn compose(self, o: Rot) -> Self {
        Self::from_degrees(self.degrees() + o.degrees())
    }

    /// Whether this rotation exchanges width and height.
    pub fn is_quarter_turn(self) -> bool {
        matches!(self, Rot::D90 | Rot::D270)
    }

    /// Applies the rotation to a size.
    pub fn apply_to_size(self, s: Vec2) -> Vec2 {
        if self.is_quarter_turn() {
            Vec2::new(s.y, s.x)
        } else {
            s
        }
    }
}

/// A page's dimensions in PDF points, before any view rotation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

impl PageSize {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// US Letter, the fallback when a page reports a degenerate MediaBox.
    pub const LETTER: Self = Self::new(612.0, 792.0);

    pub fn as_vec(self) -> Vec2 {
        Vec2::new(self.width, self.height)
    }

    /// Dimensions after applying a view rotation.
    pub fn rotated(self, r: Rot) -> Vec2 {
        r.apply_to_size(self.as_vec())
    }

    pub fn is_landscape(self) -> bool {
        self.width > self.height
    }
}

impl Default for PageSize {
    fn default() -> Self {
        Self::LETTER
    }
}

/// Maps between a single page's PDF coordinates and its on-screen rectangle.
///
/// Built from the page's laid-out content rect, so it already accounts for
/// zoom and position; the only thing it adds is rotation and the y-flip.
#[derive(Debug, Clone, Copy)]
pub struct PageTransform {
    /// The page's rect in content space.
    pub rect: Rect,
    /// Page dimensions in points, unrotated.
    pub size: PageSize,
    pub rotation: Rot,
}

impl PageTransform {
    pub fn new(rect: Rect, size: PageSize, rotation: Rot) -> Self {
        Self {
            rect,
            size,
            rotation,
        }
    }

    /// Pixels per PDF point currently being displayed.
    ///
    /// Derived from the rect rather than stored, so it cannot drift out of
    /// step with the layout that produced it.
    pub fn scale(&self) -> f32 {
        let rotated = self.size.rotated(self.rotation);
        if rotated.x > 0.0 {
            self.rect.width() / rotated.x
        } else {
            1.0
        }
    }

    /// Page space (points, y-up, origin bottom-left) to content space.
    pub fn page_to_content(&self, p: Vec2) -> Vec2 {
        let s = self.scale();
        let (w, h) = (self.size.width, self.size.height);
        // Normalised position within the page, y already flipped to y-down.
        let (nx, ny) = match self.rotation {
            Rot::D0 => (p.x, h - p.y),
            Rot::D90 => (p.y, p.x),
            Rot::D180 => (w - p.x, p.y),
            Rot::D270 => (h - p.y, w - p.x),
        };
        Vec2::new(self.rect.min.x + nx * s, self.rect.min.y + ny * s)
    }

    /// Content space back to page space. Exact inverse of
    /// [`Self::page_to_content`].
    pub fn content_to_page(&self, p: Vec2) -> Vec2 {
        let s = self.scale();
        if s <= 0.0 {
            return Vec2::ZERO;
        }
        let nx = (p.x - self.rect.min.x) / s;
        let ny = (p.y - self.rect.min.y) / s;
        let (w, h) = (self.size.width, self.size.height);
        match self.rotation {
            Rot::D0 => Vec2::new(nx, h - ny),
            Rot::D90 => Vec2::new(ny, nx),
            Rot::D180 => Vec2::new(w - nx, ny),
            Rot::D270 => Vec2::new(w - ny, h - nx),
        }
    }

    /// Maps a page-space rect to content space.
    ///
    /// Both corners are transformed and then renormalised: under a y-flip or a
    /// quarter turn the corner that was `min` is no longer the smallest.
    pub fn page_rect_to_content(&self, r: Rect) -> Rect {
        let a = self.page_to_content(r.min);
        let b = self.page_to_content(r.max);
        Rect::new(a, b).normalize()
    }

    pub fn content_rect_to_page(&self, r: Rect) -> Rect {
        let a = self.content_to_page(r.min);
        let b = self.content_to_page(r.max);
        Rect::new(a, b).normalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01, "{a} != {b}");
    }

    #[test]
    fn dividing_by_zero_yields_zero_rather_than_infinity() {
        // An infinite scroll offset strands the viewport where no clamp can
        // bring it back.
        let v = Vec2::new(10.0, 20.0) / 0.0;
        assert_eq!(v, Vec2::ZERO);
        assert_eq!(Vec2::new(10.0, 20.0) / 2.0, Vec2::new(5.0, 10.0));
    }

    #[test]
    fn rect_normalize_fixes_reversed_drag() {
        let r = Rect::new(Vec2::new(10.0, 10.0), Vec2::new(2.0, 4.0)).normalize();
        assert_eq!(r.min, Vec2::new(2.0, 4.0));
        assert_eq!(r.max, Vec2::new(10.0, 10.0));
        approx(r.width(), 8.0);
    }

    #[test]
    fn rect_intersection_and_union() {
        let a = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_xywh(5.0, 5.0, 10.0, 10.0);
        assert!(a.intersects(&b));
        approx(a.intersection(&b).area(), 25.0);
        approx(a.union(&b).area(), 225.0);

        let c = Rect::from_xywh(50.0, 50.0, 1.0, 1.0);
        assert!(!a.intersects(&c));
    }

    #[test]
    fn rotation_arithmetic_wraps() {
        assert_eq!(Rot::D270.cw(), Rot::D0);
        assert_eq!(Rot::D0.ccw(), Rot::D270);
        assert_eq!(Rot::D90.compose(Rot::D270), Rot::D0);
        assert!(Rot::D90.is_quarter_turn());
        assert!(!Rot::D180.is_quarter_turn());
    }

    #[test]
    fn quarter_turn_swaps_page_size() {
        let s = PageSize::new(612.0, 792.0);
        assert_eq!(s.rotated(Rot::D0), Vec2::new(612.0, 792.0));
        assert_eq!(s.rotated(Rot::D90), Vec2::new(792.0, 612.0));
        assert_eq!(s.rotated(Rot::D180), Vec2::new(612.0, 792.0));
    }

    #[test]
    fn page_origin_maps_to_bottom_left_on_screen() {
        // PDF origin is bottom-left; on screen that is the bottom of the rect.
        let t = PageTransform::new(
            Rect::from_xywh(100.0, 50.0, 612.0, 792.0),
            PageSize::LETTER,
            Rot::D0,
        );
        let c = t.page_to_content(Vec2::new(0.0, 0.0));
        approx(c.x, 100.0);
        approx(c.y, 50.0 + 792.0);
    }

    #[test]
    fn page_content_roundtrip_at_every_rotation() {
        for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
            let size = PageSize::new(612.0, 792.0);
            let disp = size.rotated(rot);
            // Lay the page out at 1.5x zoom at a non-zero origin, so a bug that
            // ignores either would show up.
            let rect = Rect::from_xywh(37.0, 91.0, disp.x * 1.5, disp.y * 1.5);
            let t = PageTransform::new(rect, size, rot);
            approx(t.scale(), 1.5);

            for p in [
                Vec2::new(0.0, 0.0),
                Vec2::new(612.0, 792.0),
                Vec2::new(100.0, 700.0),
                Vec2::new(306.0, 396.0),
            ] {
                let back = t.content_to_page(t.page_to_content(p));
                approx(back.x, p.x);
                approx(back.y, p.y);
            }
        }
    }

    #[test]
    fn page_rect_stays_normalized_through_the_y_flip() {
        let t = PageTransform::new(
            Rect::from_xywh(0.0, 0.0, 612.0, 792.0),
            PageSize::LETTER,
            Rot::D0,
        );
        // A page-space rect: min is bottom-left, so in content space its
        // y ordering inverts.
        let pr = Rect::new(Vec2::new(72.0, 100.0), Vec2::new(300.0, 700.0));
        let cr = t.page_rect_to_content(pr);
        assert!(cr.min.y < cr.max.y, "content rect must stay normalized");
        approx(cr.min.y, 92.0); // 792 - 700
        approx(cr.max.y, 692.0); // 792 - 100
        let back = t.content_rect_to_page(cr);
        approx(back.min.x, 72.0);
        approx(back.min.y, 100.0);
        approx(back.max.y, 700.0);
    }
}
