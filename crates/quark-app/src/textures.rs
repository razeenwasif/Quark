//! Turning rendered pages into GPU textures, and keeping the number of them
//! bounded.
//!
//! A page rasterised at 400% is roughly 25 megabytes. Uploading one per page of
//! a 500-page document would exhaust video memory long before the user noticed
//! anything was wrong, so this holds a byte budget and evicts the pages
//! furthest from the viewport when it is exceeded.

use std::collections::HashMap;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use quark_pdf::render::Raster;

/// What a cached texture was rendered for. A texture whose key no longer
/// matches the view is stale and must be re-rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageKey {
    pub page: usize,
    /// Scale, quantised to 1/64 so that a smooth zoom does not invalidate every
    /// texture on every frame. Storing the raw float would mean a new texture
    /// per frame during a pinch-zoom.
    pub scale_q: u32,
    pub rotation: u8,
    pub tint: u8,
}

impl PageKey {
    pub fn new(page: usize, scale: f32, rotation: quark_core::geom::Rot, tint: quark_core::prefs::PageTint) -> Self {
        Self {
            page,
            scale_q: (scale * 64.0).round().max(1.0) as u32,
            rotation: rotation.degrees() as u8 / 90,
            tint: tint as u8,
        }
    }

}

struct Entry {
    texture: TextureHandle,
    bytes: usize,
    /// Frame number this entry was last used on, for eviction.
    last_used: u64,
}

/// A bounded cache of page textures.
pub struct TextureCache {
    entries: HashMap<PageKey, Entry>,
    budget_bytes: usize,
    used_bytes: usize,
    frame: u64,
    /// Requests already in flight, so the same page is not asked for twice
    /// while the worker is still busy with it.
    pending: HashMap<PageKey, u64>,
}

impl TextureCache {
    pub fn new(budget_mb: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget_bytes: budget_mb.max(32) * 1024 * 1024,
            used_bytes: 0,
            frame: 0,
            pending: HashMap::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Marks a key as requested, returning false if it already was.
    pub fn mark_pending(&mut self, key: PageKey, token: u64) -> bool {
        if self.entries.contains_key(&key) {
            return false;
        }
        match self.pending.get(&key) {
            Some(&t) if t == token => false,
            _ => {
                self.pending.insert(key, token);
                true
            }
        }
    }

    pub fn is_pending(&self, key: &PageKey) -> bool {
        self.pending.contains_key(key)
    }

    /// Stores a finished raster.
    pub fn insert(&mut self, ctx: &Context, key: PageKey, raster: &Raster) {
        self.pending.remove(&key);

        let image = ColorImage::from_rgba_unmultiplied(
            [raster.width as usize, raster.height as usize],
            &raster.rgba,
        );
        let bytes = raster.rgba.len();
        let name = format!("page-{}-{}", key.page, key.scale_q);
        // Linear filtering: at anything other than 1:1 the page is being
        // scaled, and nearest-neighbour makes text shimmer while scrolling.
        let texture = ctx.load_texture(name, image, TextureOptions::LINEAR);

        if let Some(old) = self.entries.insert(key, Entry {
            texture,
            bytes,
            last_used: self.frame,
        }) {
            self.used_bytes = self.used_bytes.saturating_sub(old.bytes);
        }
        self.used_bytes += bytes;
        self.evict_if_needed();
    }

    /// Fetches a texture, marking it as used this frame.
    pub fn get(&mut self, key: &PageKey) -> Option<&TextureHandle> {
        let frame = self.frame;
        let e = self.entries.get_mut(key)?;
        e.last_used = frame;
        Some(&e.texture)
    }

    /// The best available texture for a page, even if it is the wrong scale.
    ///
    /// Showing a page at the wrong resolution for one frame while the correct
    /// one renders is far better than showing a blank rectangle, which is what
    /// makes zooming feel continuous rather than flickering.
    pub fn get_nearest(&mut self, page: usize, want: &PageKey) -> Option<&TextureHandle> {
        if self.entries.contains_key(want) {
            return self.get(want);
        }
        let best = self
            .entries
            .keys()
            .filter(|k| {
                k.page == page && k.rotation == want.rotation && k.tint == want.tint
            })
            // Prefer the closest scale, breaking ties towards the larger one so
            // the stand-in is sharp rather than blurry.
            .min_by_key(|k| {
                let d = (k.scale_q as i64 - want.scale_q as i64).abs();
                (d, -(k.scale_q as i64))
            })
            .copied()?;
        self.get(&best)
    }

    /// Drops every texture for a document, e.g. after an edit changes its pages.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
        self.used_bytes = 0;
    }

    /// Drops cached textures for one page.
    pub fn invalidate_page(&mut self, page: usize) {
        let keys: Vec<PageKey> = self
            .entries
            .keys()
            .filter(|k| k.page == page)
            .copied()
            .collect();
        for k in keys {
            if let Some(e) = self.entries.remove(&k) {
                self.used_bytes = self.used_bytes.saturating_sub(e.bytes);
            }
        }
        self.pending.retain(|k, _| k.page != page);
    }

    pub fn used_mb(&self) -> f32 {
        self.used_bytes as f32 / (1024.0 * 1024.0)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn evict_if_needed(&mut self) {
        if self.used_bytes <= self.budget_bytes {
            return;
        }
        // Least-recently-used first. Anything drawn this frame is exempt, or a
        // single page larger than the whole budget would evict itself and
        // re-render forever.
        let mut keys: Vec<(PageKey, u64, usize)> = self
            .entries
            .iter()
            .map(|(k, e)| (*k, e.last_used, e.bytes))
            .collect();
        keys.sort_by_key(|(_, used, _)| *used);

        let current_frame = self.frame;
        for (k, used, bytes) in keys {
            if self.used_bytes <= self.budget_bytes {
                break;
            }
            if used == current_frame {
                continue;
            }
            self.entries.remove(&k);
            self.used_bytes = self.used_bytes.saturating_sub(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_core::geom::Rot;
    use quark_core::prefs::PageTint;

    #[test]
    fn scale_is_quantised_so_smooth_zoom_does_not_thrash_the_cache() {
        // Two scales a hair apart must land on the same key, or every frame of
        // a pinch-zoom allocates a new texture.
        let a = PageKey::new(0, 1.0000, Rot::D0, PageTint::None);
        let b = PageKey::new(0, 1.0001, Rot::D0, PageTint::None);
        assert_eq!(a, b);

        // But a real zoom step is a different key.
        let c = PageKey::new(0, 1.25, Rot::D0, PageTint::None);
        assert_ne!(a, c);
    }

    #[test]
    fn rotation_and_tint_are_part_of_the_key() {
        let base = PageKey::new(0, 1.0, Rot::D0, PageTint::None);
        assert_ne!(base, PageKey::new(0, 1.0, Rot::D90, PageTint::None));
        assert_ne!(base, PageKey::new(0, 1.0, Rot::D0, PageTint::Night));
    }

    #[test]
    fn a_tiny_scale_never_quantises_to_zero() {
        // A zero-scale key would collide with every other tiny scale.
        let k = PageKey::new(0, 0.0001, Rot::D0, PageTint::None);
        assert!(k.scale_q >= 1);
    }

    #[test]
    fn a_page_is_only_requested_once_per_token() {
        let mut c = TextureCache::new(64);
        let k = PageKey::new(0, 1.0, Rot::D0, PageTint::None);
        assert!(c.mark_pending(k, 1), "first request should go out");
        assert!(!c.mark_pending(k, 1), "duplicate request should be skipped");
        assert!(c.is_pending(&k));
        // A new token means the view changed, so it is asked for again.
        assert!(c.mark_pending(k, 2));
    }

    #[test]
    fn invalidating_a_page_drops_its_pending_requests_too() {
        let mut c = TextureCache::new(64);
        let k = PageKey::new(3, 1.0, Rot::D0, PageTint::None);
        c.mark_pending(k, 1);
        c.invalidate_page(3);
        assert!(!c.is_pending(&k));
    }

    #[test]
    fn invalidating_one_page_leaves_the_others_alone() {
        let mut c = TextureCache::new(64);
        c.mark_pending(PageKey::new(1, 1.0, Rot::D0, PageTint::None), 1);
        c.mark_pending(PageKey::new(2, 1.0, Rot::D0, PageTint::None), 1);
        c.invalidate_page(1);
        assert!(!c.is_pending(&PageKey::new(1, 1.0, Rot::D0, PageTint::None)));
        assert!(c.is_pending(&PageKey::new(2, 1.0, Rot::D0, PageTint::None)));
    }

    #[test]
    fn clearing_empties_everything() {
        let mut c = TextureCache::new(64);
        c.mark_pending(PageKey::new(0, 1.0, Rot::D0, PageTint::None), 1);
        c.clear();
        assert_eq!(c.len(), 0);
        assert_eq!(c.used_mb(), 0.0);
    }

    #[test]
    fn the_budget_has_a_floor_so_it_can_always_hold_something() {
        // A caller passing 0 must not produce a cache that evicts everything
        // immediately and re-renders forever.
        let c = TextureCache::new(0);
        assert!(c.budget_bytes >= 32 * 1024 * 1024);
    }
}
