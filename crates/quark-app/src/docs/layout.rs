//! Where everything on the page goes: each block laid out as a galley, the
//! blocks stacked down the sheet, and the geometry the editor needs from them
//! — where a click lands, where the caret is drawn, what a selection covers.
//!
//! # The page is paper
//!
//! The sheet is white in both themes, the way a PDF page is: it is the thing
//! being made, not part of the interface around it. So nothing here depends
//! on the theme, and a block's galley depends only on its content, the zoom
//! and what is attached to it — which is what lets it be cached by stamp.

use std::collections::HashMap;
use std::sync::Arc;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Color32, FontId, Galley, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use quark_doc::{Block, BlockKind, Document, Marks, Pos};
use quark_ui::theme;

// --- the sheet, at 100% ---

/// The sheet's width. With its margins the line is about 75 characters of
/// body text, which is what a page of a book holds.
pub(crate) const PAPER_W: f32 = 760.0;
pub(crate) const MARGIN_X: f32 = 88.0;
pub(crate) const MARGIN_TOP: f32 = 76.0;
pub(crate) const MARGIN_BOTTOM: f32 = 120.0;
/// How far each level of a list steps in.
const INDENT: f32 = 26.0;
/// Inside a quote's or code block's box.
const BOX_PAD: Vec2 = vec2(16.0, 11.0);

// --- ink ---

pub(crate) const INK: Color32 = Color32::from_rgb(0x1f, 0x1c, 0x25);
pub(crate) const INK_MUTED: Color32 = Color32::from_rgb(0x6f, 0x6a, 0x78);
pub(crate) const INK_FAINT: Color32 = Color32::from_rgb(0xb9, 0xb3, 0xc4);
pub(crate) const LINK: Color32 = Color32::from_rgb(0x5b, 0x3f, 0xc4);
pub(crate) const MARK: Color32 = Color32::from_rgb(0x6a, 0x4f, 0xd6);
const CODE_INK: Color32 = Color32::from_rgb(0x4b, 0x3a, 0x99);
pub(crate) const CODE_BG: Color32 = Color32::from_rgb(0xf1, 0xef, 0xf5);
pub(crate) const QUOTE_BG: Color32 = Color32::from_rgb(0xf4, 0xf2, 0xf8);
const QUOTE_INK: Color32 = Color32::from_rgb(0x4a, 0x46, 0x52);
pub(crate) const RULE: Color32 = Color32::from_rgb(0xdd, 0xd9, 0xe4);
pub(crate) const SELECTION: Color32 = Color32::from_rgba_premultiplied(0x1f, 0x17, 0x38, 0x3a);
pub(crate) const CARET: Color32 = Color32::from_rgb(0x6a, 0x4f, 0xd6);
/// A comment's text: amber, as a highlighter would leave it.
const COMMENT_WASH: Color32 = Color32::from_rgba_premultiplied(0x5a, 0x47, 0x28, 0x5f);
const COMMENT_ACTIVE: Color32 = Color32::from_rgba_premultiplied(0x8a, 0x66, 0x24, 0x8c);
const COMMENT_LINE: Color32 = Color32::from_rgb(0xe3, 0xa3, 0x3f);
/// Text a suggestion would replace, and what it would replace it with.
const STRUCK: Color32 = Color32::from_rgb(0xb4, 0x23, 0x4a);
const INSERTED: Color32 = Color32::from_rgb(0x4b, 0x2f, 0xb8);
const INSERTED_BG: Color32 = Color32::from_rgba_premultiplied(0x0c, 0x09, 0x16, 0x1a);

/// What is attached to the text, and how each thing is shown.
#[derive(Default, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Decor {
    /// Open comments; resolved ones are not marked on the page.
    pub(crate) comments: Vec<u32>,
    /// The comment selected in the dock, drawn stronger.
    pub(crate) active_comment: Option<u32>,
    /// Pending suggestions: their ids and their text as it would read.
    pub(crate) suggestions: Vec<(u32, String)>,
}

impl Decor {
    pub(crate) fn of(doc: &Document, active_comment: Option<u32>) -> Decor {
        Decor {
            comments: doc.comments.iter().filter(|c| !c.resolved).map(|c| c.id).collect(),
            active_comment,
            suggestions: doc.suggestions.iter().map(|s| (s.id, phantom_text(&s.markdown))).collect(),
        }
    }
}

/// Claude's replacement as it is shown inline: its text, blocks on lines.
fn phantom_text(markdown: &str) -> String {
    let blocks = quark_doc::markdown::read_blocks(markdown);
    blocks.iter().map(|b| b.text()).collect::<Vec<_>>().join("\n")
}

/// Text shown in a galley that is not in the block: a suggestion's
/// replacement, after the text it would replace.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Phantom {
    /// The block offset it follows.
    pub(crate) at: usize,
    pub(crate) len: usize,
    pub(crate) id: u32,
}

/// Galley index of a block offset: past any phantom text before it.
fn to_galley(phantoms: &[Phantom], offset: usize) -> usize {
    offset + phantoms.iter().filter(|p| p.at < offset).map(|p| p.len).sum::<usize>()
}

/// Block offset of a galley index. Inside phantom text, the offset it
/// follows.
fn to_block(phantoms: &[Phantom], index: usize) -> usize {
    let mut shift = 0;
    for p in phantoms {
        let start = p.at + shift;
        if index < start {
            break;
        }
        if index < start + p.len {
            return p.at;
        }
        shift += p.len;
    }
    index - shift
}

/// One block, laid out and placed on the sheet.
#[derive(Clone)]
pub(crate) struct Placed {
    pub(crate) galley: Arc<Galley>,
    /// The galley's top-left, relative to the sheet's top-left.
    pub(crate) origin: Vec2,
    /// The whole band the block occupies, sheet-relative: what a click in the
    /// margin beside it means.
    pub(crate) band: Rect,
    /// A quote's or code block's tinted box, sheet-relative.
    pub(crate) panel: Option<Rect>,
    pub(crate) marker: Marker,
    pub(crate) phantoms: Vec<Phantom>,
}

#[derive(Clone, PartialEq)]
pub(crate) enum Marker {
    None,
    Bullet { centre: Vec2, depth: u8 },
    Number { right: Vec2, text: String },
    /// A checkbox: its square, sheet-relative.
    Task { square: Rect, checked: bool },
    Rule { y: f32 },
}

impl Placed {
    pub(crate) fn to_galley(&self, offset: usize) -> usize {
        to_galley(&self.phantoms, offset)
    }

    pub(crate) fn to_block(&self, index: usize) -> usize {
        to_block(&self.phantoms, index)
    }

    /// The caret's line at a block offset, sheet-relative.
    pub(crate) fn caret(&self, offset: usize) -> Rect {
        let r = self.galley.pos_from_cursor(CCursor::new(self.to_galley(offset)));
        let r = if r.height() < 1.0 { Rect::from_min_size(r.min, vec2(0.0, self.band.height())) } else { r };
        r.translate(self.origin)
    }

    /// The block offset nearest a sheet-relative point.
    pub(crate) fn hit(&self, at: Pos2) -> usize {
        let c = self.galley.cursor_from_pos(at.to_vec2() - self.origin);
        self.to_block(c.index.0)
    }
}

/// The whole sheet as laid out this frame.
#[derive(Default)]
pub(crate) struct Sheet {
    pub(crate) blocks: Vec<Placed>,
    pub(crate) height: f32,
}

impl Sheet {
    /// The position nearest a sheet-relative point.
    pub(crate) fn hit(&self, at: Pos2) -> Pos {
        if self.blocks.is_empty() {
            return Pos::default();
        }
        let i = self
            .blocks
            .iter()
            .position(|b| at.y < b.band.bottom())
            .unwrap_or(self.blocks.len() - 1);
        Pos::new(i, self.blocks[i].hit(at))
    }

    pub(crate) fn caret(&self, at: Pos) -> Rect {
        match self.blocks.get(at.block) {
            Some(b) => b.caret(at.offset),
            None => Rect::NOTHING,
        }
    }

    /// One visual line up or down from `at`, keeping to `x` (sheet-relative)
    /// across blocks. `None` past the first or last line.
    pub(crate) fn vertical(&self, at: Pos, down: bool, x: f32) -> Option<Pos> {
        let block = self.blocks.get(at.block)?;
        let g = &block.galley;
        let here = CCursor::new(block.to_galley(at.offset));
        let row = g.layout_from_cursor(here).row;
        let local_x = x - block.origin.x;
        let within = if down { row + 1 < g.rows.len() } else { row > 0 };
        if within {
            let (c, _) = if down { g.cursor_down_one_row(&here, Some(local_x)) } else { g.cursor_up_one_row(&here, Some(local_x)) };
            return Some(Pos::new(at.block, block.to_block(c.index.0)));
        }
        let next = if down { at.block + 1 } else { at.block.checked_sub(1)? };
        let target = self.blocks.get(next)?;
        let rows = &target.galley.rows;
        let y = if down { rows.first().map_or(0.0, |r| r.rect().center().y) } else { rows.last().map_or(0.0, |r| r.rect().center().y) };
        let c = target.galley.cursor_from_pos(vec2(x - target.origin.x, y));
        Some(Pos::new(next, target.to_block(c.index.0)))
    }

    /// The start or end of the visual line `at` is on.
    pub(crate) fn line_end(&self, at: Pos, end: bool) -> Pos {
        let Some(block) = self.blocks.get(at.block) else { return at };
        let here = CCursor::new(block.to_galley(at.offset));
        let c = if end { block.galley.cursor_end_of_row(&here) } else { block.galley.cursor_begin_of_row(&here) };
        Pos::new(at.block, block.to_block(c.index.0))
    }

    /// Rectangles covering a selection, sheet-relative.
    pub(crate) fn selection(&self, a: Pos, b: Pos) -> Vec<Rect> {
        let mut out = Vec::new();
        for i in a.block..=b.block.min(self.blocks.len().saturating_sub(1)) {
            let block = &self.blocks[i];
            let len = block.galley.text().chars().count();
            let from = if i == a.block { block.to_galley(a.offset) } else { 0 };
            let to = if i == b.block { block.to_galley(b.offset) } else { len };
            // A selection that runs on into the next block shows the line
            // break it takes, as a sliver after the text.
            let past_end = i < b.block;
            range_rects(&block.galley, from, to, past_end, &mut out, block.origin);
        }
        out
    }
}

/// Rectangles covering galley chars `from..to`, offset by `origin`.
fn range_rects(g: &Galley, from: usize, to: usize, past_end: bool, out: &mut Vec<Rect>, origin: Vec2) {
    let mut base = 0;
    let last = g.rows.len().saturating_sub(1);
    for (ri, placed) in g.rows.iter().enumerate() {
        let n = placed.row.char_count_excluding_newline().0;
        let start = from.max(base);
        let end = to.min(base + n);
        let reaches_end = to > base + n || (past_end && ri == last);
        if start <= end && (start < end || reaches_end) && from <= base + n && to >= base {
            let x0 = placed.pos.x + placed.row.x_offset(egui::text::CharIndex(start - base));
            let mut x1 = placed.pos.x + placed.row.x_offset(egui::text::CharIndex(end - base));
            if reaches_end {
                x1 += 6.0;
            }
            let r = placed.rect();
            out.push(Rect::from_min_max(pos2(x0, r.top()), pos2(x1, r.bottom())).translate(origin));
        }
        base += placed.char_count_including_newline().0;
    }
}

// --- fonts ---

struct Face {
    size: f32,
    line: f32,
    heavy: bool,
    ink: Color32,
    italics: bool,
    mono: bool,
}

fn face(kind: &BlockKind) -> Face {
    let body = Face { size: 15.5, line: 25.0, heavy: false, ink: INK, italics: false, mono: false };
    match kind {
        BlockKind::Heading(1) => Face { size: 29.0, line: 37.0, heavy: true, ..body },
        BlockKind::Heading(2) => Face { size: 21.5, line: 30.0, heavy: true, ..body },
        BlockKind::Heading(3) => Face { size: 17.5, line: 26.0, heavy: true, ..body },
        BlockKind::Heading(_) => Face { size: 15.5, line: 25.0, heavy: true, ink: QUOTE_INK, ..body },
        BlockKind::Quote => Face { ink: QUOTE_INK, ..body },
        BlockKind::Code { .. } => Face { size: 13.5, line: 21.0, mono: true, ink: INK, ..body },
        _ => body,
    }
}

/// Space before a block, given what came before it.
fn space_before(kind: &BlockKind, prev: Option<&BlockKind>) -> f32 {
    let Some(prev) = prev else { return 0.0 };
    match (kind, prev) {
        (BlockKind::Heading(1), _) => 30.0,
        (BlockKind::Heading(2), _) => 26.0,
        (BlockKind::Heading(_), _) => 20.0,
        (k, BlockKind::Heading(_)) if !matches!(k, BlockKind::Heading(_)) => 8.0,
        (k, p) if k.is_list() && p.is_list() => 3.0,
        (BlockKind::Quote, BlockKind::Quote) => 2.0,
        _ => 13.0,
    }
}

// --- laying out ---

/// Galleys by what they show: a block's stamp, the width, the zoom and what
/// is attached to its text. A frame lays out only what changed.
/// A block's stamp, its wrap width and zoom as bits, and what is attached.
type Key = (u64, u32, u32, u64);

#[derive(Default)]
pub(crate) struct Cache {
    galleys: HashMap<Key, (Arc<Galley>, Vec<Phantom>)>,
    /// Anchors each block carries, by stamp.
    anchors: HashMap<u64, Vec<u32>>,
    used: Vec<Key>,
}

impl Cache {
    fn anchors(&mut self, block: &Block) -> &[u32] {
        self.anchors.entry(block.stamp()).or_insert_with(|| {
            let mut ids: Vec<u32> = block.styles().iter().flat_map(|s| s.anchors.iter().copied()).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        })
    }

    /// Drops what the last frame did not use.
    fn sweep(&mut self) {
        let used: std::collections::HashSet<_> = self.used.drain(..).collect();
        self.galleys.retain(|k, _| used.contains(k));
        let stamps: std::collections::HashSet<u64> = used.iter().map(|k| k.0).collect();
        self.anchors.retain(|s, _| stamps.contains(s));
    }
}

fn hash_of(t: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// Lays out the whole sheet.
pub(crate) fn lay_out(ctx: &egui::Context, doc: &Document, decor: &Decor, zoom: f32, cache: &mut Cache) -> Sheet {
    let text_w = (PAPER_W - 2.0 * MARGIN_X) * zoom;
    let mut y = MARGIN_TOP * zoom;
    let mut blocks = Vec::with_capacity(doc.blocks.len());
    // The number each depth of a numbered list has reached.
    let mut numbers: Vec<u64> = Vec::new();
    let mut prev: Option<&BlockKind> = None;

    for block in &doc.blocks {
        let kind = &block.kind;
        y += space_before(kind, prev) * zoom;

        // Numbering, as Markdown will write it.
        let number = match kind {
            BlockKind::Numbered { depth } => {
                let d = *depth as usize;
                numbers.truncate(d + 1);
                numbers.resize(d + 1, 0);
                numbers[d] += 1;
                Some(numbers[d])
            }
            k if k.is_list() => {
                let d = k.depth().unwrap_or(0) as usize;
                numbers.truncate(d + 1);
                numbers.resize(d + 1, 0);
                numbers[d] = 0;
                None
            }
            _ => {
                numbers.clear();
                None
            }
        };

        let indent = kind.depth().map_or(0.0, |d| (d as f32 + 1.0) * INDENT) * zoom;
        let boxed = matches!(kind, BlockKind::Quote | BlockKind::Code { .. });
        let pad = if boxed { BOX_PAD * zoom } else { Vec2::ZERO };
        let wrap = (text_w - indent - 2.0 * pad.x).max(40.0);

        let relevant: Vec<u32> = cache.anchors(block).to_vec();
        let decor_key = hash_of(&(
            decor.comments.iter().filter(|c| relevant.contains(c)).collect::<Vec<_>>(),
            decor.active_comment.filter(|c| relevant.contains(c)),
            decor.suggestions.iter().filter(|(id, _)| relevant.contains(id)).collect::<Vec<_>>(),
        ));
        let key = (block.stamp(), wrap.to_bits(), zoom.to_bits(), decor_key);
        cache.used.push(key);
        let (galley, phantoms) = match cache.galleys.get(&key) {
            Some(hit) => hit.clone(),
            None => {
                let (job, phantoms) = job_of(block, decor, wrap, zoom);
                let galley = ctx.fonts_mut(|f| f.layout_job(job));
                cache.galleys.insert(key, (galley.clone(), phantoms.clone()));
                (galley, phantoms)
            }
        };

        let f = face(kind);
        let height = if kind == &BlockKind::Rule { 24.0 * zoom } else { galley.size().y.max(f.line * zoom) };
        let origin = vec2(MARGIN_X * zoom + indent + pad.x, y + pad.y);
        let band = Rect::from_min_size(pos2(0.0, y), vec2(PAPER_W * zoom, height + 2.0 * pad.y));
        let panel = boxed.then(|| Rect::from_min_size(pos2(MARGIN_X * zoom + indent, y), vec2(text_w - indent, height + 2.0 * pad.y)));

        let first_row_mid = galley.rows.first().map_or(f.line * zoom / 2.0, |r| r.rect().center().y);
        let marker = match kind {
            BlockKind::Bullet { depth } => Marker::Bullet { centre: vec2(origin.x - 13.0 * zoom, origin.y + first_row_mid), depth: *depth },
            BlockKind::Numbered { .. } => Marker::Number { right: vec2(origin.x - 7.0 * zoom, origin.y + first_row_mid), text: format!("{}.", number.unwrap_or(1)) },
            BlockKind::Task { checked, .. } => Marker::Task {
                square: Rect::from_center_size(pos2(origin.x - 14.0 * zoom, origin.y + first_row_mid), Vec2::splat(16.0 * zoom)),
                checked: *checked,
            },
            BlockKind::Rule => Marker::Rule { y: y + height / 2.0 },
            _ => Marker::None,
        };

        blocks.push(Placed { galley, origin, band, panel, marker, phantoms });
        y += height + 2.0 * pad.y;
        prev = Some(kind);
    }
    cache.sweep();
    Sheet { blocks, height: y + MARGIN_BOTTOM * zoom }
}

/// A block's text as a layout job: its marks as fonts and strokes, what is
/// attached to it as washes, and any suggestion's replacement after the text
/// it would replace.
fn job_of(block: &Block, decor: &Decor, wrap: f32, zoom: f32) -> (LayoutJob, Vec<Phantom>) {
    let f = face(&block.kind);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let mut phantoms = Vec::new();

    let base = |marks: Marks| -> TextFormat {
        let size = f.size * zoom;
        let mono = f.mono || marks.contains(Marks::CODE);
        let font = if mono {
            FontId::monospace(if f.mono { size } else { size * 0.88 })
        } else if f.heavy || marks.contains(Marks::BOLD) {
            theme::heavy(size)
        } else {
            theme::body(size)
        };
        TextFormat {
            font_id: font,
            line_height: Some(f.line * zoom),
            color: f.ink,
            italics: f.italics || marks.contains(Marks::ITALIC),
            ..Default::default()
        }
    };

    let checked = matches!(block.kind, BlockKind::Task { checked: true, .. });
    let ends: Vec<(usize, u32)> = decor
        .suggestions
        .iter()
        .filter_map(|(id, _)| {
            let last = block.styles().iter().rposition(|s| s.has_anchor(*id))?;
            Some((last + 1, *id))
        })
        .collect();

    for (range, text, style) in block.runs() {
        let mut fmt = base(style.marks);
        if checked {
            fmt.color = INK_MUTED;
            fmt.strikethrough = Stroke::new(1.2 * zoom, INK_MUTED);
        }
        if style.marks.contains(Marks::CODE) && !f.mono {
            fmt.color = CODE_INK;
            fmt.background = CODE_BG;
        }
        if style.marks.contains(Marks::UNDERLINE) {
            fmt.underline = Stroke::new(1.0 * zoom, fmt.color);
        }
        if style.marks.contains(Marks::STRIKE) {
            fmt.strikethrough = Stroke::new(1.2 * zoom, fmt.color);
        }
        if style.link.is_some() {
            fmt.color = LINK;
            fmt.underline = Stroke::new(1.0 * zoom, LINK);
        }
        if let Some(id) = style.anchors.iter().find(|id| decor.comments.contains(id)) {
            fmt.background = if decor.active_comment == Some(*id) { COMMENT_ACTIVE } else { COMMENT_WASH };
            fmt.underline = Stroke::new(1.6 * zoom, COMMENT_LINE);
        }
        if style.anchors.iter().any(|id| decor.suggestions.iter().any(|(s, _)| s == id)) {
            fmt.color = STRUCK;
            fmt.strikethrough = Stroke::new(1.4 * zoom, STRUCK);
        }
        job.append(text, 0.0, fmt);

        // A suggestion's replacement, right after the last of its text.
        for &(at, id) in ends.iter().filter(|(at, _)| *at == range.end) {
            let Some((_, shown)) = decor.suggestions.iter().find(|(s, _)| *s == id) else { continue };
            let mut fmt = base(Marks::NONE);
            fmt.color = INSERTED;
            fmt.background = INSERTED_BG;
            fmt.underline = Stroke::new(1.6 * zoom, INSERTED);
            let shown = format!(" {shown}");
            phantoms.push(Phantom { at, len: shown.chars().count(), id });
            job.append(&shown, 0.0, fmt);
        }
    }
    if block.is_empty() {
        // An empty block still has a line to put the caret on.
        job.append("", 0.0, base(Marks::NONE));
    }
    (job, phantoms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phantom_text_is_stepped_over_both_ways() {
        // Block "abcdef", with five phantom characters shown after offset 3.
        let p = [Phantom { at: 3, len: 5, id: 1 }];
        assert_eq!(to_galley(&p, 2), 2);
        assert_eq!(to_galley(&p, 3), 3);
        assert_eq!(to_galley(&p, 4), 9);
        assert_eq!(to_block(&p, 3), 3);
        assert_eq!(to_block(&p, 5), 3);
        assert_eq!(to_block(&p, 7), 3);
        assert_eq!(to_block(&p, 8), 3);
        assert_eq!(to_block(&p, 9), 4);
    }

    #[test]
    fn headings_open_more_space_than_paragraphs() {
        assert!(space_before(&BlockKind::Heading(1), Some(&BlockKind::Paragraph)) > space_before(&BlockKind::Paragraph, Some(&BlockKind::Paragraph)));
        assert_eq!(space_before(&BlockKind::Paragraph, None), 0.0);
        assert!(space_before(&BlockKind::Bullet { depth: 0 }, Some(&BlockKind::Bullet { depth: 0 })) < 5.0);
    }
}
