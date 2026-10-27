//! One open document, as the UI sees it.

use std::collections::HashMap;
use std::path::PathBuf;

use quark_core::annot::{AnnotId, Annotation};
use quark_core::geom::{Rect, Rot, Vec2};
use quark_core::layout::{self, Layout, LayoutParams, PageMode, ZoomMode};
use quark_core::search::SearchState;
use quark_pdf::doc::DocInfo;
use quark_pdf::forms::FormField;
use quark_pdf::outline::{Attachment, Bookmark, Link};
use quark_pdf::service::DocId;
use quark_pdf::text::PageText;

/// A run of selected text on one page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSelection {
    pub page: usize,
    pub start: usize,
    pub end: usize,
}

impl TextSelection {
    pub fn range(&self) -> (usize, usize) {
        (self.start.min(self.end), self.start.max(self.end))
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// A position the reader can navigate back to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPosition {
    pub page: usize,
    pub scroll: Vec2,
    pub zoom: ZoomMode,
}

/// The in-progress geometry of a tool being dragged.
#[derive(Debug, Clone, PartialEq)]
pub enum Drafting {
    None,
    /// A rectangle being dragged out, in page space.
    Rect { page: usize, start: Vec2, current: Vec2 },
    /// A freehand stroke being drawn.
    Ink { page: usize, points: Vec<Vec2> },
    /// A sequence of clicked vertices.
    Vertices { page: usize, points: Vec<Vec2>, cursor: Vec2 },
    /// A line or arrow.
    Line { page: usize, start: Vec2, current: Vec2 },
}

impl Drafting {
    pub fn is_active(&self) -> bool {
        !matches!(self, Drafting::None)
    }

    pub fn page(&self) -> Option<usize> {
        match self {
            Drafting::None => None,
            Drafting::Rect { page, .. }
            | Drafting::Ink { page, .. }
            | Drafting::Vertices { page, .. }
            | Drafting::Line { page, .. } => Some(*page),
        }
    }

    /// The rectangle a drag has swept, normalised.
    pub fn rect(&self) -> Option<Rect> {
        match self {
            Drafting::Rect { start, current, .. } | Drafting::Line { start, current, .. } => {
                Some(Rect::new(*start, *current).normalize())
            }
            _ => None,
        }
    }
}

/// Everything the UI holds for one open document.
pub struct Tab {
    pub id: DocId,
    pub info: DocInfo,

    // --- view ---
    pub page_mode: PageMode,
    pub zoom: ZoomMode,
    pub rotation: Rot,
    pub scroll: Vec2,
    /// Viewport size in pixels, from the last frame that drew this tab.
    pub viewport: Vec2,
    /// The page the reader is considered to be on.
    pub current_page: usize,
    /// Cached layout, recomputed when any of the inputs change.
    pub layout: Layout,
    layout_key: Option<LayoutKey>,

    // --- document data, filled in asynchronously ---
    pub annotations: Vec<Annotation>,
    pub outline: Vec<Bookmark>,
    pub fields: Vec<FormField>,
    pub attachments: Vec<Attachment>,
    pub links: Vec<Link>,
    pub page_text: HashMap<usize, PageText>,
    /// Pages whose text has been asked for but not yet returned.
    pub text_pending: Vec<usize>,

    // --- interaction ---
    pub selection: Option<TextSelection>,
    /// A place lit for a moment — where SyncTeX found a line — on a page,
    /// in page space, and when it was first drawn.
    pub flash: Option<(usize, Rect, Option<f64>)>,
    /// For a LaTeX build's PDF, which build made it.
    pub pdf_version: u64,
    pub selected_annotation: Option<AnnotId>,
    pub selected_pages: Vec<usize>,
    /// The page grid is shown in place of the pages.
    pub grid: bool,
    pub drafting: Drafting,
    pub search: SearchState,
    /// An annotation being dragged, and how far it has gone: one change
    /// when it is let go.
    pub moving: Option<(AnnotId, Vec2)>,
    /// What is being written in place on the page.
    pub editing: Option<Editing>,
    /// A reply being written in the comments panel, to the comment chosen.
    pub reply_draft: String,

    // --- navigation history, for Previous/Next View ---
    back_stack: Vec<ViewPosition>,
    forward_stack: Vec<ViewPosition>,

    /// Bumped whenever the view changes, so stale renders can be discarded.
    pub render_token: u64,
    /// The display's pixels to a point — 2 on a Retina screen — as the
    /// viewer last saw it: pages are drawn at it, so text is sharp.
    pub pixels_per_point: f32,
    /// How far the view has been pulled past the edge of the page shown,
    /// when paged: far enough, and the page turns.
    pull: f32,
    /// Redaction marks waiting to be applied: not comments of their own,
    /// so they are kept here rather than in the document. Every other
    /// annotation is a change to the document as soon as it is made.
    pub pending_annotations: Vec<Annotation>,
    next_annot_id: u64,
}

/// How far past the edge of a page, in points, the view must be pulled
/// to turn it.
const TURN_PULL: f32 = 140.0;

/// What is written in place on the page.
#[derive(Debug, Clone, PartialEq)]
pub enum EditTarget {
    /// A form's field: its text, or a choice of its `options`.
    Field {
        name: String,
        kind: quark_pdf::forms::FieldKind,
        options: Vec<String>,
        multiline: bool,
    },
    /// A note or text box being made: kept until it is written, so none is
    /// made empty.
    New(Box<Annotation>),
    /// What a note or text box already in the document says, or the
    /// comment on any other mark.
    Annotation(AnnotId),
}

/// What a click on a form's field does.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldClick {
    /// A box ticked or unticked, a button chosen: the field set to it.
    Set(String),
    /// Text or a choice, written where the field is.
    Write(Editing),
    Nothing,
}

/// What writing in place comes to.
#[derive(Debug, Clone, PartialEq)]
pub enum Kept {
    Nothing,
    /// A field's text, or its choice.
    Field { name: String, value: String, text: bool },
    /// A note or text box, made now that it says something.
    Make(Box<Annotation>),
    /// What an annotation says, changed.
    Say { id: AnnotId, contents: String },
}

/// What a click on `field` does.
pub fn clicked(field: &FormField) -> FieldClick {
    use quark_pdf::forms::FieldKind;
    if field.read_only {
        return FieldClick::Nothing;
    }
    match field.kind {
        // A box is ticked by its own "on" state, whatever the form calls it.
        FieldKind::Checkbox => {
            let on = field.on_state.clone().unwrap_or_else(|| "Yes".into());
            FieldClick::Set(if field.value == on { "Off".into() } else { on })
        }
        FieldKind::RadioButton => match &field.on_state {
            Some(on) if *on != field.value => FieldClick::Set(on.clone()),
            _ => FieldClick::Nothing,
        },
        FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox => FieldClick::Write(Editing {
            page: field.page,
            rect: field.rect,
            text: field.value.clone(),
            target: EditTarget::Field {
                name: field.name.clone(),
                kind: field.kind,
                options: field.options.clone(),
                multiline: field.multiline,
            },
            focused: false,
        }),
        _ => FieldClick::Nothing,
    }
}

/// Writing in place: what, where, and what has been written so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Editing {
    pub target: EditTarget,
    pub page: usize,
    pub rect: Rect,
    pub text: String,
    /// Whether the keyboard has been given to it yet.
    pub focused: bool,
}

/// The inputs that determine a layout. Recomputing only when these change keeps
/// scrolling free of a per-frame layout pass over every page.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayoutKey {
    mode: PageMode,
    zoom: ZoomMode,
    rotation: Rot,
    viewport: Vec2,
    page_count: usize,
}

impl Tab {
    pub fn new(id: DocId, info: DocInfo, page_mode: PageMode, zoom: ZoomMode) -> Self {
        Self {
            id,
            info,
            page_mode,
            zoom,
            rotation: Rot::D0,
            scroll: Vec2::ZERO,
            viewport: Vec2::new(800.0, 600.0),
            current_page: 0,
            layout: Layout::default(),
            layout_key: None,
            annotations: Vec::new(),
            outline: Vec::new(),
            fields: Vec::new(),
            attachments: Vec::new(),
            links: Vec::new(),
            page_text: HashMap::new(),
            text_pending: Vec::new(),
            selection: None,
            flash: None,
            pdf_version: 0,
            selected_annotation: None,
            selected_pages: Vec::new(),
            grid: false,
            drafting: Drafting::None,
            search: SearchState::default(),
            moving: None,
            editing: None,
            reply_draft: String::new(),
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            render_token: 1,
            pending_annotations: Vec::new(),
            pixels_per_point: 1.0,
            pull: 0.0,
            // Far above any object number, so a mark made here never
            // shares an id with an annotation read from the document.
            next_annot_id: 1 << 48,
        }
    }

    pub fn title(&self) -> String {
        self.info.display_name()
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.info.path.as_ref()
    }

    pub fn page_count(&self) -> usize {
        self.info.page_count
    }

    /// A fresh annotation identifier for this document.
    pub fn next_annot_id(&mut self) -> AnnotId {
        let id = AnnotId(self.next_annot_id);
        self.next_annot_id += 1;
        id
    }

    pub fn layout_params(&self) -> LayoutParams {
        LayoutParams {
            mode: self.page_mode,
            zoom: self.zoom,
            rotation: self.rotation,
            gap: quark_ui::theme::PAGE_GAP,
            margin: quark_ui::theme::PAGE_MARGIN,
            viewport: self.viewport,
        }
    }

    /// Recomputes the layout if any input changed. Cheap to call every frame.
    pub fn ensure_layout(&mut self) {
        let key = LayoutKey {
            mode: self.page_mode,
            zoom: self.zoom,
            rotation: self.rotation,
            viewport: self.viewport,
            page_count: self.info.page_count,
        };
        if self.layout_key == Some(key) {
            return;
        }
        self.layout = layout::compute(&self.info.page_sizes(), &self.layout_params());
        self.layout_key = Some(key);
        self.clamp_scroll();
    }

    /// Forces a layout recomputation, e.g. after pages were added or removed.
    pub fn invalidate_layout(&mut self) {
        self.layout_key = None;
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll = self.layout.clamp_scroll(self.scroll, self.viewport);
        // A page at a time: scrolling stays within the page or spread shown.
        if let Some((top, bottom)) = self.band() {
            self.scroll.y = self.scroll.y.clamp(top, bottom);
        }
    }

    /// Whether pages are read one at a time (or one spread at a time), not
    /// scrolled through.
    pub fn paged(&self) -> bool {
        !self.page_mode.is_continuous()
    }

    /// The row the current page is in.
    fn current_row(&self) -> Option<usize> {
        self.layout.page(self.current_page).map(|pb| pb.row)
    }

    /// When paged, how far the view may scroll: the shown row and its margin.
    fn band(&self) -> Option<(f32, f32)> {
        if !self.paged() {
            return None;
        }
        let row = self.layout.rows.get(self.current_row()?)?;
        let margin = self.layout_params().margin;
        let top = (row.top - margin).max(0.0);
        let bottom = (row.bottom + margin - self.viewport.y).max(top);
        Some((top, bottom))
    }

    /// Whether a page is drawn: every page when scrolling through, only the
    /// shown page or spread when paged.
    pub fn shows(&self, page: usize) -> bool {
        if !self.paged() {
            return true;
        }
        let row = self.current_row();
        self.layout.page(page).is_some_and(|pb| Some(pb.row) == row)
    }

    /// Turn to the next page or spread, or back: by rows when paged, by pages
    /// when scrolling through. Going back by the wheel lands at the foot of
    /// the page before, as if it had been scrolled up to.
    pub fn turn(&mut self, by: i32, at_foot: bool) {
        self.ensure_layout();
        let to = if self.paged() {
            let Some(row) = self.current_row() else { return };
            let to = (row as i64 + i64::from(by)).clamp(0, self.layout.rows.len() as i64 - 1) as usize;
            if to == row {
                return;
            }
            self.layout.rows[to].first_page
        } else {
            (self.current_page as i64 + i64::from(by)).clamp(0, self.page_count() as i64 - 1).max(0) as usize
        };
        self.go_to_page(to);
        if at_foot {
            if let Some((_, bottom)) = self.band() {
                self.scroll.y = bottom;
            }
        }
    }

    /// The wheel or trackpad moved the view. Paged, a pull past the edge of
    /// the page shown — a deliberate one, not the end of a fling — turns it.
    pub fn scroll_by(&mut self, delta: Vec2) {
        let wanted = self.scroll + delta;
        self.scroll = wanted;
        self.clamp_scroll();
        if !self.paged() {
            self.sync_current_page();
            return;
        }
        let over = wanted.y - self.scroll.y;
        // Pulling the other way, or not at the edge, lets go.
        if over == 0.0 || over.signum() != self.pull.signum() {
            self.pull = 0.0;
        }
        self.pull += over;
        if self.pull.abs() >= TURN_PULL {
            let by = self.pull.signum() as i32;
            self.pull = 0.0;
            self.turn(by, by < 0);
        }
    }

    /// The visible region in content space.
    pub fn view_rect(&self) -> Rect {
        Rect::new(self.scroll, self.scroll + self.viewport)
    }

    /// The region to render, inflated so pages just off-screen are ready
    /// before they scroll in.
    pub fn prerender_rect(&self, pages_ahead: usize) -> Rect {
        let pad = self.viewport.y * pages_ahead as f32;
        let v = self.view_rect();
        Rect::new(
            Vec2::new(v.min.x, v.min.y - pad),
            Vec2::new(v.max.x, v.max.y + pad),
        )
    }

    /// Records the current position so Previous View can return to it.
    pub fn push_history(&mut self) {
        let pos = ViewPosition {
            page: self.current_page,
            scroll: self.scroll,
            zoom: self.zoom,
        };
        // Consecutive duplicates would make Previous View appear to do nothing.
        if self.back_stack.last() == Some(&pos) {
            return;
        }
        self.back_stack.push(pos);
        if self.back_stack.len() > 64 {
            self.back_stack.remove(0);
        }
        self.forward_stack.clear();
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    /// Returns to the previous view, if any.
    pub fn go_back(&mut self) -> Option<ViewPosition> {
        let target = self.back_stack.pop()?;
        self.forward_stack.push(ViewPosition {
            page: self.current_page,
            scroll: self.scroll,
            zoom: self.zoom,
        });
        self.apply_position(target);
        Some(target)
    }

    pub fn go_forward(&mut self) -> Option<ViewPosition> {
        let target = self.forward_stack.pop()?;
        self.back_stack.push(ViewPosition {
            page: self.current_page,
            scroll: self.scroll,
            zoom: self.zoom,
        });
        self.apply_position(target);
        Some(target)
    }

    fn apply_position(&mut self, p: ViewPosition) {
        self.zoom = p.zoom;
        self.current_page = p.page;
        self.invalidate_layout();
        self.ensure_layout();
        self.scroll = p.scroll;
        self.clamp_scroll();
        self.bump_token();
    }

    /// Scrolls so `page` is at the top of the viewport.
    pub fn go_to_page(&mut self, page: usize) {
        if self.info.page_count == 0 {
            return;
        }
        let page = page.min(self.info.page_count - 1);
        self.push_history();
        self.ensure_layout();
        let params = self.layout_params();
        self.scroll = self.layout.scroll_to_page(page, &params);
        self.current_page = page;
        self.clamp_scroll();
        self.bump_token();
    }

    /// Scrolls the given page-space rectangle into view, centred if it is not
    /// already visible.
    pub fn reveal(&mut self, page: usize, rect: Rect) {
        self.ensure_layout();
        let Some(pb) = self.layout.page(page) else {
            return;
        };
        let t = quark_core::geom::PageTransform::new(pb.rect, pb.size, self.rotation);
        let content = t.page_rect_to_content(rect);
        let view = self.view_rect();
        if view.contains(content.min) && view.contains(content.max) {
            // Already on screen; scrolling would be a distraction.
            self.current_page = page;
            return;
        }
        self.push_history();
        // A third from the top rather than centred: a hit near the bottom of
        // the window is easy to miss, and this keeps following context visible.
        self.scroll = Vec2::new(
            self.scroll.x,
            content.center().y - self.viewport.y / 3.0,
        );
        self.current_page = page;
        self.clamp_scroll();
        self.bump_token();
    }

    /// Updates `current_page` from the scroll position. Paged, the page is
    /// the one turned to, whatever the view shows of it.
    pub fn sync_current_page(&mut self) {
        if self.info.page_count == 0 || self.paged() {
            return;
        }
        self.current_page = self.layout.dominant_page(self.view_rect());
    }

    pub fn bump_token(&mut self) {
        self.render_token = self.render_token.wrapping_add(1);
    }

    /// Zooms about a fixed point in the viewport, so the content under the
    /// pointer stays put.
    pub fn zoom_to(&mut self, new_scale: f32, anchor_screen: Vec2) {
        let old = self.layout.scale.max(0.0001);
        let new_scale = new_scale.clamp(layout::MIN_ZOOM, layout::MAX_ZOOM);
        // The content point under the anchor, before the zoom.
        let content = self.scroll + anchor_screen;
        let ratio = new_scale / old;

        self.zoom = ZoomMode::Custom(new_scale);
        self.invalidate_layout();
        self.ensure_layout();

        // Put the same content point back under the anchor.
        self.scroll = content * ratio - anchor_screen;
        self.clamp_scroll();
        self.bump_token();
    }

    /// The text of a page, if it has been extracted.
    pub fn text(&self, page: usize) -> Option<&PageText> {
        self.page_text.get(&page)
    }

    /// The currently selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        let t = self.page_text.get(&sel.page)?;
        let (a, b) = sel.range();
        Some(t.slice(a, b))
    }

    /// Every annotation on a page, including ones not yet written to the file.
    pub fn annotations_on(&self, page: usize) -> impl Iterator<Item = &Annotation> {
        self.annotations
            .iter()
            .chain(self.pending_annotations.iter())
            .filter(move |a| a.page == page && !a.hidden)
    }

    /// All annotations, saved and pending, for the comments panel.
    pub fn all_annotations(&self) -> Vec<&Annotation> {
        let mut v: Vec<&Annotation> = self
            .annotations
            .iter()
            .chain(self.pending_annotations.iter())
            .collect();
        // Reading order: down the document, then down each page.
        v.sort_by(|a, b| {
            a.page
                .cmp(&b.page)
                .then(b.rect.max.y.total_cmp(&a.rect.max.y))
        });
        v
    }

    pub fn find_annotation(&self, id: AnnotId) -> Option<&Annotation> {
        self.annotations
            .iter()
            .chain(self.pending_annotations.iter())
            .find(|a| a.id == id)
    }

    pub fn find_annotation_mut(&mut self, id: AnnotId) -> Option<&mut Annotation> {
        self.annotations
            .iter_mut()
            .chain(self.pending_annotations.iter_mut())
            .find(|a| a.id == id)
    }

    /// The scale pages are drawn at: the layout's, in the display's own
    /// pixels. What a page's texture is kept under, too, so a window moved to
    /// a screen of another density draws its pages again.
    pub fn raster_scale(&self) -> f32 {
        self.layout.scale * self.pixels_per_point
    }

    /// Whether there is anything unsaved.
    pub fn has_unsaved_changes(&self) -> bool {
        self.info.dirty || !self.pending_annotations.is_empty()
    }

    /// Writing in an annotation already on the page: a text box says its
    /// text, anything else its comment. None for a locked one.
    pub fn writing_in(&self, id: AnnotId) -> Option<Editing> {
        use quark_core::annot::AnnotKind;
        let a = self.find_annotation(id).filter(|a| !a.locked)?;
        let text = match &a.kind {
            AnnotKind::FreeText { text, .. } if !text.is_empty() => text.clone(),
            _ => a.contents.clone(),
        };
        Some(Editing { page: a.page, rect: a.rect, text, target: EditTarget::Annotation(id), focused: false })
    }

    /// What writing `text` in `target` comes to: nothing when nothing
    /// changed, and no note or text box made empty.
    pub fn kept(&self, target: EditTarget, text: String) -> Kept {
        use quark_core::annot::AnnotKind;
        match target {
            EditTarget::Field { name, kind, .. } => {
                if !self.fields.iter().any(|f| f.name == name && f.value != text) {
                    return Kept::Nothing;
                }
                Kept::Field { name, value: text, text: kind == quark_pdf::forms::FieldKind::Text }
            }
            EditTarget::New(mut ann) => {
                if text.trim().is_empty() {
                    return Kept::Nothing;
                }
                if let AnnotKind::FreeText { text: words, .. } = &mut ann.kind {
                    *words = text.clone();
                }
                ann.contents = text;
                Kept::Make(ann)
            }
            EditTarget::Annotation(id) => {
                let Some(a) = self.find_annotation(id) else { return Kept::Nothing };
                let was = match &a.kind {
                    AnnotKind::FreeText { text, .. } if !text.is_empty() => text,
                    _ => &a.contents,
                };
                if *was == text { Kept::Nothing } else { Kept::Say { id, contents: text } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_core::annot::AnnotKind;
    use quark_core::geom::PageSize;
    use quark_pdf::doc::PageInfo;

    fn info(pages: usize) -> DocInfo {
        DocInfo {
            page_count: pages,
            pages: (0..pages)
                .map(|_| PageInfo {
                    size: PageSize::LETTER,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn tab(pages: usize) -> Tab {
        let mut t = Tab::new(1, info(pages), PageMode::Continuous, ZoomMode::Custom(1.0));
        t.viewport = Vec2::new(900.0, 700.0);
        t.ensure_layout();
        t
    }

    #[test]
    fn the_layout_is_only_recomputed_when_an_input_changes() {
        let mut t = tab(50);
        let first = t.layout.content_size;
        t.ensure_layout();
        assert_eq!(t.layout.content_size, first);
        // Changing the zoom must produce a different layout.
        t.zoom = ZoomMode::Custom(2.0);
        t.ensure_layout();
        assert!(t.layout.content_size.y > first.y);
    }

    #[test]
    fn going_to_a_page_scrolls_there_and_records_it() {
        let mut t = tab(10);
        t.go_to_page(5);
        assert_eq!(t.current_page, 5);
        assert!(t.scroll.y > 0.0);
        assert!(t.can_go_back());
    }

    #[test]
    fn going_past_the_last_page_clamps() {
        let mut t = tab(3);
        t.go_to_page(99);
        assert_eq!(t.current_page, 2);
    }

    #[test]
    fn navigating_an_empty_document_is_harmless() {
        let mut t = tab(0);
        t.go_to_page(3);
        t.sync_current_page();
        assert_eq!(t.current_page, 0);
    }

    #[test]
    fn previous_and_next_view_retrace_the_path() {
        let mut t = tab(20);
        t.go_to_page(4);
        let at_four = t.scroll;
        t.go_to_page(12);
        assert_eq!(t.current_page, 12);

        t.go_back();
        assert_eq!(t.current_page, 4);
        assert!((t.scroll.y - at_four.y).abs() < 1.0);
        assert!(t.can_go_forward());

        t.go_forward();
        assert_eq!(t.current_page, 12);
    }

    #[test]
    fn history_does_not_record_the_same_position_twice() {
        // Otherwise Previous View appears to do nothing on the first press.
        let mut t = tab(10);
        t.push_history();
        t.push_history();
        t.push_history();
        t.go_back();
        assert!(!t.can_go_back(), "duplicate positions were recorded");
    }

    #[test]
    fn a_new_jump_clears_the_forward_history() {
        let mut t = tab(20);
        t.go_to_page(5);
        t.go_to_page(10);
        t.go_back();
        assert!(t.can_go_forward());
        t.go_to_page(15);
        assert!(!t.can_go_forward());
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_still() {
        // This is what makes ctrl-scroll feel like zooming rather than jumping.
        let mut t = tab(10);
        t.scroll = Vec2::new(0.0, 400.0);
        let anchor = Vec2::new(450.0, 350.0);
        let content_before = t.scroll + anchor;

        t.zoom_to(2.0, anchor);

        let content_after = (t.scroll + anchor) / t.layout.scale;
        let expected = content_before / 1.0;
        assert!(
            (content_after.y - expected.y).abs() < 2.0,
            "content drifted: {} vs {}",
            content_after.y,
            expected.y
        );
    }

    #[test]
    fn zoom_is_clamped_to_the_supported_range() {
        let mut t = tab(3);
        t.zoom_to(1e6, Vec2::ZERO);
        assert!(t.layout.scale <= layout::MAX_ZOOM);
        t.zoom_to(0.0, Vec2::ZERO);
        assert!(t.layout.scale >= layout::MIN_ZOOM);
    }

    #[test]
    fn the_render_token_changes_whenever_the_view_moves() {
        // Stale renders are discarded by comparing this; if it does not move,
        // the viewer keeps showing the old page.
        let mut t = tab(10);
        let before = t.render_token;
        t.go_to_page(3);
        assert_ne!(t.render_token, before);
    }

    #[test]
    fn the_prerender_region_extends_beyond_the_viewport() {
        let t = tab(10);
        let view = t.view_rect();
        let pre = t.prerender_rect(2);
        assert!(pre.min.y < view.min.y);
        assert!(pre.max.y > view.max.y);
    }

    #[test]
    fn the_current_page_follows_the_scroll_position() {
        let mut t = tab(10);
        t.scroll = Vec2::new(0.0, 850.0);
        t.sync_current_page();
        assert_eq!(t.current_page, 1);
    }

    #[test]
    fn revealing_something_already_on_screen_does_not_scroll() {
        let mut t = tab(10);
        let before = t.scroll;
        // Near the top of page 1, which is already visible.
        t.reveal(0, Rect::from_xywh(72.0, 700.0, 100.0, 20.0));
        assert_eq!(t.scroll, before);
    }

    #[test]
    fn revealing_something_off_screen_scrolls_to_it() {
        let mut t = tab(10);
        t.reveal(6, Rect::from_xywh(72.0, 700.0, 100.0, 20.0));
        assert!(t.scroll.y > 0.0);
        assert_eq!(t.current_page, 6);
    }

    #[test]
    fn selected_text_comes_from_the_extracted_page() {
        let mut t = tab(2);
        t.page_text.insert(0, PageText {
            page: 0,
            text: "hello world".into(),
            chars: Vec::new(),
        });
        t.selection = Some(TextSelection {
            page: 0,
            start: 6,
            end: 11,
        });
        assert_eq!(t.selected_text().as_deref(), Some("world"));
    }

    #[test]
    fn a_backwards_selection_still_yields_the_right_text() {
        let mut t = tab(2);
        t.page_text.insert(0, PageText {
            page: 0,
            text: "hello world".into(),
            chars: Vec::new(),
        });
        t.selection = Some(TextSelection {
            page: 0,
            start: 11,
            end: 6,
        });
        assert_eq!(t.selected_text().as_deref(), Some("world"));
    }

    #[test]
    fn annotation_ids_are_unique_within_a_tab() {
        let mut t = tab(1);
        let a = t.next_annot_id();
        let b = t.next_annot_id();
        assert_ne!(a, b);
    }

    #[test]
    fn pending_annotations_are_visible_before_they_are_saved() {
        let mut t = tab(2);
        let id = t.next_annot_id();
        t.pending_annotations.push(Annotation::new(
            id,
            1,
            AnnotKind::Square,
            Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
            "me",
        ));
        assert_eq!(t.annotations_on(1).count(), 1);
        assert_eq!(t.annotations_on(0).count(), 0);
        assert!(t.find_annotation(id).is_some());
        assert!(t.has_unsaved_changes());
    }

    #[test]
    fn hidden_annotations_are_not_drawn() {
        let mut t = tab(1);
        let id = t.next_annot_id();
        let mut a = Annotation::new(id, 0, AnnotKind::Square, Rect::ZERO, "me");
        a.hidden = true;
        t.pending_annotations.push(a);
        assert_eq!(t.annotations_on(0).count(), 0);
    }

    #[test]
    fn the_comments_list_is_in_reading_order() {
        let mut t = tab(3);
        // Deliberately out of order: page 2 first, then two on page 1.
        for (page, y) in [(2usize, 700.0f32), (1, 200.0), (1, 600.0)] {
            let id = t.next_annot_id();
            t.pending_annotations.push(Annotation::new(
                id,
                page,
                AnnotKind::Square,
                Rect::from_xywh(10.0, y, 20.0, 20.0),
                "me",
            ));
        }
        let all = t.all_annotations();
        assert_eq!(all[0].page, 1);
        // Within a page, higher on the page comes first.
        assert!(all[0].rect.min.y > all[1].rect.min.y);
        assert_eq!(all[2].page, 2);
    }

    #[test]
    fn a_clean_tab_reports_no_unsaved_changes() {
        let t = tab(1);
        assert!(!t.has_unsaved_changes());
    }

    #[test]
    fn drafting_reports_the_page_it_belongs_to() {
        let d = Drafting::Rect {
            page: 3,
            start: Vec2::ZERO,
            current: Vec2::new(10.0, 10.0),
        };
        assert!(d.is_active());
        assert_eq!(d.page(), Some(3));
        assert_eq!(d.rect().unwrap().width(), 10.0);
        assert!(!Drafting::None.is_active());
        assert_eq!(Drafting::None.page(), None);
    }

    #[test]
    fn a_backwards_drag_still_yields_a_positive_rectangle() {
        let d = Drafting::Rect {
            page: 0,
            start: Vec2::new(100.0, 100.0),
            current: Vec2::new(20.0, 30.0),
        };
        let r = d.rect().unwrap();
        assert!(r.width() > 0.0 && r.height() > 0.0);
    }

    fn field(kind: quark_pdf::forms::FieldKind, value: &str, on: Option<&str>) -> FormField {
        FormField {
            name: "f".into(),
            kind,
            page: 0,
            rect: Rect::new(Vec2::new(10.0, 10.0), Vec2::new(110.0, 30.0)),
            value: value.into(),
            options: vec!["One".into(), "Two".into()],
            read_only: false,
            required: false,
            multiline: false,
            password: false,
            annot_index: 0,
            on_state: on.map(Into::into),
        }
    }

    #[test]
    fn a_box_ticks_by_its_own_on_and_unticks_to_off() {
        use quark_pdf::forms::FieldKind;
        // A form that calls "on" something other than Yes is ticked by that.
        assert_eq!(clicked(&field(FieldKind::Checkbox, "Off", Some("Agreed"))), FieldClick::Set("Agreed".into()));
        assert_eq!(clicked(&field(FieldKind::Checkbox, "Agreed", Some("Agreed"))), FieldClick::Set("Off".into()));
        // A button already chosen stays so; another is chosen by its own state.
        assert_eq!(clicked(&field(FieldKind::RadioButton, "Left", Some("Left"))), FieldClick::Nothing);
        assert_eq!(clicked(&field(FieldKind::RadioButton, "Left", Some("Right"))), FieldClick::Set("Right".into()));
        let mut locked = field(FieldKind::Checkbox, "Off", Some("Yes"));
        locked.read_only = true;
        assert_eq!(clicked(&locked), FieldClick::Nothing);
    }

    #[test]
    fn text_and_choices_are_written_where_the_field_is_and_kept_only_when_changed() {
        use quark_pdf::forms::FieldKind;
        let mut t = tab(1);
        let f = field(FieldKind::Text, "Ada", None);
        t.fields.push(f.clone());
        let FieldClick::Write(e) = clicked(&f) else { panic!("a text field is written in") };
        assert_eq!((e.page, e.rect, e.text.as_str()), (0, f.rect, "Ada"));
        assert_eq!(t.kept(e.target.clone(), "Ada".into()), Kept::Nothing);
        assert_eq!(
            t.kept(e.target, "Grace".into()),
            Kept::Field { name: "f".into(), value: "Grace".into(), text: true }
        );
        let combo = field(FieldKind::ComboBox, "One", None);
        t.fields[0] = combo.clone();
        let FieldClick::Write(e) = clicked(&combo) else { panic!("a choice is written in") };
        assert_eq!(t.kept(e.target, "Two".into()), Kept::Field { name: "f".into(), value: "Two".into(), text: false });
    }

    #[test]
    fn a_note_or_text_box_is_made_only_once_it_says_something() {
        use quark_core::annot::{NoteIcon, TextAlign};
        let t = tab(1);
        let r = Rect::new(Vec2::new(50.0, 50.0), Vec2::new(70.0, 70.0));
        let note = Annotation::new(AnnotId(1 << 48), 0, AnnotKind::Note { icon: NoteIcon::Comment }, r, "me");
        assert_eq!(t.kept(EditTarget::New(Box::new(note.clone())), "  \n".into()), Kept::Nothing);
        let Kept::Make(made) = t.kept(EditTarget::New(Box::new(note)), "Check this".into()) else {
            panic!("a note that says something is made")
        };
        assert_eq!(made.contents, "Check this");
        let boxed = Annotation::new(
            AnnotId((1 << 48) + 1),
            0,
            AnnotKind::FreeText {
                text: String::new(),
                font_size: 12.0,
                font: "Helvetica".into(),
                align: TextAlign::Left,
                callout: None,
            },
            r,
            "me",
        );
        let Kept::Make(made) = t.kept(EditTarget::New(Box::new(boxed)), "Typed".into()) else {
            panic!("a text box that says something is made")
        };
        // A text box shows its words, and says them as its contents too.
        assert!(matches!(&made.kind, AnnotKind::FreeText { text, .. } if text == "Typed"));
        assert_eq!(made.contents, "Typed");
    }

    #[test]
    fn writing_in_a_mark_starts_from_what_it_says_and_keeps_only_a_change() {
        use quark_core::annot::NoteIcon;
        let mut t = tab(1);
        let r = Rect::new(Vec2::new(50.0, 50.0), Vec2::new(200.0, 62.0));
        let mut hl = Annotation::new(AnnotId(7), 0, AnnotKind::Highlight { quads: vec![r] }, r, "me");
        hl.contents = "Why?".into();
        t.annotations.push(hl);
        let e = t.writing_in(AnnotId(7)).expect("a highlight's comment is written in");
        assert_eq!((e.text.as_str(), &e.target), ("Why?", &EditTarget::Annotation(AnnotId(7))));
        assert_eq!(t.kept(e.target.clone(), "Why?".into()), Kept::Nothing);
        assert_eq!(t.kept(e.target, "Because".into()), Kept::Say { id: AnnotId(7), contents: "Because".into() });
        let mut locked = Annotation::new(AnnotId(8), 0, AnnotKind::Note { icon: NoteIcon::Comment }, r, "me");
        locked.locked = true;
        t.annotations.push(locked);
        assert_eq!(t.writing_in(AnnotId(8)), None);
    }

    fn tab_in(mode: PageMode, pages: usize) -> Tab {
        let mut t = Tab::new(1, info(pages), mode, ZoomMode::Custom(1.0));
        t.viewport = Vec2::new(900.0, 700.0);
        t.ensure_layout();
        t
    }

    /// The wheel moved down by `dy` points, `times` over.
    fn wheel(t: &mut Tab, dy: f32, times: usize) {
        for _ in 0..times {
            t.scroll_by(Vec2::new(0.0, dy));
        }
    }

    #[test]
    fn one_page_at_a_time_scrolls_within_the_page_and_a_pull_past_it_turns() {
        let mut t = tab_in(PageMode::Single, 5);
        // A letter page at 100 % is taller than the window: it scrolls, but
        // never into the next.
        let (_, foot) = t.band().unwrap();
        assert!(foot > 40.0, "{foot}");
        wheel(&mut t, 40.0, 1);
        assert_eq!((t.current_page, t.scroll.y), (0, 40.0));
        let to_foot = foot - t.scroll.y;
        wheel(&mut t, to_foot, 1);
        assert_eq!((t.current_page, t.scroll.y), (0, foot));
        assert!(t.shows(0) && !t.shows(1), "only the page turned to is drawn");
        // At its foot, a short pull is the end of a fling, not a turn.
        wheel(&mut t, 40.0, 3);
        assert_eq!(t.current_page, 0);
        wheel(&mut t, 40.0, 1);
        assert_eq!(t.current_page, 1, "pulled 160 points past the foot, it turns");
        let (top, _) = t.band().unwrap();
        assert_eq!(t.scroll.y, top, "the next page is shown from its top");
        // Back: pulled past the top, the page before, shown from its foot.
        wheel(&mut t, -40.0, 4);
        assert_eq!(t.current_page, 0);
        assert_eq!(t.scroll.y, t.band().unwrap().1);
    }

    #[test]
    fn turning_goes_a_spread_at_a_time_and_stops_at_the_ends() {
        let mut t = tab_in(PageMode::TwoUp, 5);
        t.turn(1, false);
        assert_eq!(t.current_page, 2, "the next spread's first page");
        assert!(t.shows(2) && t.shows(3) && !t.shows(1) && !t.shows(4));
        t.turn(1, false);
        t.turn(1, false);
        assert_eq!(t.current_page, 4, "the last spread, and no further");
        t.turn(-5, false);
        assert_eq!(t.current_page, 0);
        // Scrolling through, a turn is a page.
        let mut c = tab_in(PageMode::Continuous, 5);
        c.turn(1, false);
        assert_eq!(c.current_page, 1);
        assert!(c.shows(4), "every page is drawn when scrolling through");
    }

    #[test]
    fn scrolling_through_still_follows_the_page_on_screen() {
        let mut t = tab_in(PageMode::Continuous, 5);
        wheel(&mut t, 400.0, 6);
        assert!(t.current_page >= 2, "{}", t.current_page);
    }

    #[test]
    fn pages_are_drawn_in_the_displays_own_pixels() {
        let mut t = tab(2);
        let one = t.raster_scale();
        assert_eq!(one, t.layout.scale);
        t.pixels_per_point = 2.0;
        assert_eq!(t.raster_scale(), 2.0 * one, "a Retina screen gets twice the pixels");
        let key = |s: f32| crate::textures::PageKey::new(0, s, t.rotation, quark_core::prefs::PageTint::None);
        assert_ne!(key(one), key(t.raster_scale()), "moved to another screen, a page is drawn again");
    }
}
