//! Markdown in and out.
//!
//! # The file is the document
//!
//! A Quark document is a Markdown file, written the way a person would have
//! typed it: `**bold**`, `- items`, `## headings`. Any other editor can open
//! it, and a document written elsewhere opens here. What Markdown has no
//! syntax for is written as the little HTML CommonMark allows — `<u>` for
//! underline, `&nbsp;` for a deliberately empty line.
//!
//! Comments and Claude's pending suggestions are not part of the text, so
//! they go at the very end in an HTML comment, which every Markdown renderer
//! hides:
//!
//! ```text
//! <!-- quark-doc
//! {"v":1,"comments":[…],"suggestions":[…]}
//! -->
//! ```
//!
//! Each records the block and character range it was anchored to and the
//! text that was there. When the file was edited elsewhere and the range no
//! longer holds that text, the text is looked for instead, and failing that
//! the comment is kept without an anchor rather than pinned to the wrong words.
//!
//! # Reading
//!
//! pulldown-cmark parses; this turns its events into blocks. Writing is done
//! here by hand, so that what comes out is plain.

use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};

use crate::block::{Block, BlockKind, Marks, Style};
use crate::doc::{Comment, Document, Pos, Suggestion};

const TRAILER: &str = "<!-- quark-doc";

/// The deepest list nesting kept.
const MAX_DEPTH: usize = 5;

// --- reading ---

/// A whole file: its blocks, and the comments and suggestions in its trailer.
pub fn read(src: &str) -> Document {
    let (body, meta) = split_trailer(src);
    let mut doc = Document::from_blocks(read_blocks(body));
    if let Some(meta) = meta {
        restore(&mut doc, meta);
    }
    doc
}

/// Markdown as blocks: a pasted fragment, or Claude's reply.
pub fn read_blocks(src: &str) -> Vec<Block> {
    let mut b = Builder::default();
    for event in Parser::new_ext(
        src,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        b.event(event);
    }
    b.finish()
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    current: Option<Block>,
    /// Open lists, innermost last: whether each is numbered.
    lists: Vec<bool>,
    /// The kind of the list item just opened, until its text starts.
    item: Option<BlockKind>,
    /// Open block quotes, and how many blocks there were when each opened.
    quotes: Vec<usize>,
    marks: Marks,
    links: Vec<Arc<str>>,
    /// Inside a fenced or indented code block: its language and text so far.
    code: Option<(String, String)>,
    /// Inside an HTML block: its source so far. It arrives a line at a
    /// time, and only the whole of it says what it is.
    html: Option<String>,
}

impl Builder {
    fn style(&self) -> Style {
        Style {
            marks: self.marks,
            link: self.links.last().cloned(),
            anchors: Vec::new(),
        }
    }

    fn depth(&self) -> u8 {
        self.lists.len().saturating_sub(1).min(MAX_DEPTH) as u8
    }

    /// What a block of plain text is here.
    fn text_kind(&self) -> BlockKind {
        if !self.quotes.is_empty() {
            BlockKind::Quote
        } else {
            BlockKind::Paragraph
        }
    }

    fn flush(&mut self) {
        if let Some(mut b) = self.current.take() {
            // `&nbsp;` alone is how an empty line was written.
            if b.text() == "\u{a0}" {
                b.remove(0..1);
            }
            self.blocks.push(b);
        }
    }

    fn open(&mut self, kind: BlockKind) {
        self.flush();
        self.current = Some(Block::new(kind));
    }

    /// The block text goes into, opened if there is none.
    fn target(&mut self) -> &mut Block {
        if self.current.is_none() {
            let kind = self.item.take().unwrap_or_else(|| self.text_kind());
            self.current = Some(Block::new(kind));
        }
        self.current.as_mut().unwrap()
    }

    fn push_text(&mut self, text: &str, style: Style) {
        let block = self.target();
        let at = block.len();
        block.insert(at, text, &style);
    }

    fn event(&mut self, event: Event) {
        if let Some((_, code)) = &mut self.code {
            match event {
                Event::Text(t) => code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, mut text) = self.code.take().unwrap();
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.flush();
                    self.blocks
                        .push(Block::with_text(BlockKind::Code { lang }, &text));
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                let style = self.style();
                self.push_text(&t, style);
            }
            Event::Code(t) => {
                let mut style = self.style();
                style.marks.insert(Marks::CODE);
                self.push_text(&t, style);
            }
            Event::SoftBreak => {
                let style = self.style();
                self.push_text(" ", style);
            }
            Event::HardBreak => {
                let style = self.style();
                self.push_text("\n", style);
            }
            Event::Rule => {
                self.flush();
                self.blocks.push(Block::new(BlockKind::Rule));
            }
            Event::TaskListMarker(checked) => {
                let depth = self.depth();
                let kind = BlockKind::Task { checked, depth };
                match &mut self.current {
                    Some(b) if b.is_empty() => b.set_kind(kind),
                    _ => self.item = Some(kind),
                }
            }
            Event::Html(html) => match &mut self.html {
                Some(source) => source.push_str(&html),
                None => self.inline_html(&html),
            },
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::FootnoteReference(name) => {
                let style = self.style();
                self.push_text(&format!("[^{name}]"), style);
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                let style = self.style();
                self.push_text(&t, style);
            }
        }
    }

    fn inline_html(&mut self, html: &str) {
        match html.trim().to_ascii_lowercase().as_str() {
            "<u>" | "<ins>" => self.marks.insert(Marks::UNDERLINE),
            "</u>" | "</ins>" => self.marks.remove(Marks::UNDERLINE),
            "<strong>" | "<b>" => self.marks.insert(Marks::BOLD),
            "</strong>" | "</b>" => self.marks.remove(Marks::BOLD),
            "<em>" | "<i>" => self.marks.insert(Marks::ITALIC),
            "</em>" | "</i>" => self.marks.remove(Marks::ITALIC),
            "<s>" | "<del>" | "<strike>" => self.marks.insert(Marks::STRIKE),
            "</s>" | "</del>" | "</strike>" => self.marks.remove(Marks::STRIKE),
            "<br>" | "<br/>" | "<br />" => {
                let style = self.style();
                self.push_text("\n", style);
            }
            _ => {
                let style = self.style();
                self.push_text(html, style);
            }
        }
    }

    /// A whole HTML block, ended.
    fn html_block(&mut self, source: &str) {
        let source = source.trim();
        let lower = source.to_ascii_lowercase();
        if matches!(lower.as_str(), "<br>" | "<br/>" | "<br />") {
            // On a line of its own: a paragraph that is one line break.
            self.blocks.push(Block::paragraph("\n"));
        } else if lower.starts_with("<!--") && lower.ends_with("-->") {
            // Comments are notes to whoever reads the source. Quark's own
            // trailer has been cut off before parsing.
        } else if !source.is_empty() {
            // Kept, visibly, as text: better seen than silently lost.
            self.blocks.push(Block::paragraph(source));
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                let kind = self.item.take().unwrap_or_else(|| self.text_kind());
                match &self.current {
                    // A loose list item: its text arrives in a paragraph the
                    // item has already opened a block for.
                    Some(b) if b.is_empty() && b.kind.is_list() => {}
                    _ => self.open(kind),
                }
            }
            Tag::Heading { level, .. } => self.open(BlockKind::Heading(level as u8)),
            Tag::BlockQuote(_) => {
                self.flush();
                self.quotes.push(self.blocks.len());
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                self.item = None;
                self.lists.push(start.is_some());
            }
            Tag::Item => {
                self.flush();
                let depth = self.depth();
                self.item = Some(if self.lists.last() == Some(&true) {
                    BlockKind::Numbered { depth }
                } else {
                    BlockKind::Bullet { depth }
                });
            }
            Tag::Emphasis => self.marks.insert(Marks::ITALIC),
            Tag::Strong => self.marks.insert(Marks::BOLD),
            Tag::Strikethrough => self.marks.insert(Marks::STRIKE),
            // An image is kept as a link to it, its description as the text:
            // Quark does not show images yet, and this loses nothing a reader
            // can see.
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push(dest_url.as_ref().into())
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.flush(),
            TagEnd::Item => {
                // An item with no text at all is still an item.
                if let Some(kind) = self.item.take() {
                    self.flush();
                    self.blocks.push(Block::new(kind));
                }
                self.flush();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                if let Some(before) = self.quotes.pop() {
                    if self.blocks.len() == before {
                        self.blocks.push(Block::new(BlockKind::Quote));
                    }
                }
            }
            TagEnd::HtmlBlock => {
                self.flush();
                if let Some(source) = self.html.take() {
                    self.html_block(&source);
                }
            }
            TagEnd::Emphasis => self.marks.remove(Marks::ITALIC),
            TagEnd::Strong => self.marks.remove(Marks::BOLD),
            TagEnd::Strikethrough => self.marks.remove(Marks::STRIKE),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        if self.blocks.is_empty() {
            self.blocks.push(Block::new(BlockKind::Paragraph));
        }
        self.blocks
    }
}

// --- writing ---

/// A whole file: the blocks, then the trailer when there is anything for it.
pub fn write(doc: &Document) -> String {
    let mut out = write_blocks(&doc.blocks);
    if let Some(meta) = trailer(doc) {
        out.push('\n');
        out.push_str(&meta);
    }
    out
}

/// Blocks as Markdown, ending in a newline.
pub fn write_blocks(blocks: &[Block]) -> String {
    // Empty paragraphs at the very end are where the caret was, not lines
    // anyone meant to keep.
    let mut end = blocks.len();
    while end > 0 && blocks[end - 1].kind == BlockKind::Paragraph && blocks[end - 1].is_empty() {
        end -= 1;
    }
    let blocks = &blocks[..end];

    let mut out = String::new();
    // For each open list depth: the column its items' text starts at, and
    // the number its last numbered item had.
    let mut columns: Vec<usize> = Vec::new();
    let mut numbers: Vec<Option<u64>> = Vec::new();
    let mut prev_list = false;
    // Whether the outermost list so far is numbered: a numbered list right
    // after a bulleted one is a new list, and reads as one with a gap.
    let mut numbered_top = None;

    for (i, block) in blocks.iter().enumerate() {
        let list = block.kind.is_list();
        // An item cannot be deeper than one below the item above it, so its
        // depth as written may be less than its own.
        let written = block
            .kind
            .depth()
            .map(|d| (d as usize).min(if prev_list { columns.len() } else { 0 }));
        let top = (written == Some(0)).then_some(matches!(block.kind, BlockKind::Numbered { .. }));
        let new_list = top.is_some() && numbered_top.is_some() && top != numbered_top;
        if top.is_some() {
            numbered_top = top;
        } else if !list {
            numbered_top = None;
        }
        if i > 0 {
            out.push_str(if list && prev_list && !new_list {
                "\n"
            } else {
                "\n\n"
            });
        }
        if new_list {
            columns.clear();
            numbers.clear();
        }
        if !list {
            columns.clear();
            numbers.clear();
        }
        prev_list = list;

        match &block.kind {
            BlockKind::Paragraph => {
                if block.is_empty() {
                    out.push_str("&nbsp;");
                } else {
                    out.push_str(&inline(block, ""));
                }
            }
            BlockKind::Heading(level) => {
                out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
                // A heading is one line: its breaks become spaces.
                let mut flat = Block::new(block.kind.clone());
                flat.insert_styled(0, &block.text().replace('\n', " "), block.styles());
                let mut text = inline(&flat, "");
                // `## Item #` would be read as the heading "Item", the `#` taken
                // for an optional closing sequence.
                let body = text.trim_end_matches('#');
                if body.len() < text.len() && (body.is_empty() || body.ends_with(' ')) {
                    text.insert(body.len(), '\\');
                }
                if !text.is_empty() {
                    out.push(' ');
                    out.push_str(&text);
                }
            }
            BlockKind::Quote => {
                out.push('>');
                let text = inline(block, "> ");
                if !text.is_empty() {
                    out.push(' ');
                    out.push_str(&text);
                }
            }
            BlockKind::Code { lang } => {
                let longest = longest_run(block.text(), '`');
                let fence = "`".repeat((longest + 1).max(3));
                out.push_str(&fence);
                out.push_str(lang);
                out.push('\n');
                if !block.is_empty() {
                    out.push_str(block.text());
                    out.push('\n');
                }
                out.push_str(&fence);
            }
            BlockKind::Rule => out.push_str("---"),
            kind @ (BlockKind::Bullet { depth }
            | BlockKind::Numbered { depth }
            | BlockKind::Task { depth, .. }) => {
                // An item cannot be deeper than one below the item above it.
                let depth = (*depth as usize).min(columns.len());
                columns.truncate(depth);
                numbers.truncate(depth + 1);
                numbers.resize(depth + 1, None);
                let indent = if depth == 0 { 0 } else { columns[depth - 1] };
                let marker = match kind {
                    BlockKind::Numbered { .. } => {
                        let n = numbers[depth].map(|n| n + 1).unwrap_or(1);
                        numbers[depth] = Some(n);
                        format!("{n}. ")
                    }
                    BlockKind::Task { checked, .. } => {
                        numbers[depth] = None;
                        if *checked {
                            "- [x] ".to_string()
                        } else {
                            "- [ ] ".to_string()
                        }
                    }
                    _ => {
                        numbers[depth] = None;
                        "- ".to_string()
                    }
                };
                // Text in a task item starts after the box, but a list nested
                // under it lines up with the box: the box is part of the text.
                let width = if matches!(kind, BlockKind::Task { .. }) {
                    2
                } else {
                    marker.len()
                };
                columns.push(indent + width);
                let pad = " ".repeat(indent);
                out.push_str(&pad);
                let text = inline(block, &" ".repeat(indent + marker.len()));
                if text.is_empty() {
                    // A bare `1.` cannot start a list under a line of text,
                    // and `- [ ]` alone is a bullet reading "[ ]": an empty
                    // item holds an `&nbsp;`, which reads back as nothing.
                    out.push_str(&marker);
                    out.push_str("&nbsp;");
                } else {
                    out.push_str(&marker);
                    out.push_str(&text);
                }
            }
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn longest_run(text: &str, c: char) -> usize {
    let mut best = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == c {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// One character as it is written: what it is, and the marks and link on it.
#[derive(Clone)]
struct Glyph {
    c: char,
    marks: Marks,
    link: Option<Arc<str>>,
}

/// Marks written as delimiters must hug their text: `**word** ` and never
/// `**word **`, which CommonMark does not read as bold at all. So whitespace
/// at either end of a marked stretch is moved outside it.
fn hug(glyphs: &mut [Glyph]) {
    for mark in [Marks::BOLD, Marks::ITALIC, Marks::STRIKE] {
        let mut i = 0;
        while i < glyphs.len() {
            if !glyphs[i].marks.contains(mark) {
                i += 1;
                continue;
            }
            let start = i;
            while i < glyphs.len() && glyphs[i].marks.contains(mark) {
                i += 1;
            }
            let end = i;
            let mut s = start;
            while s < end && glyphs[s].c.is_whitespace() {
                glyphs[s].marks.remove(mark);
                s += 1;
            }
            let mut e = end;
            while e > s && glyphs[e - 1].c.is_whitespace() {
                glyphs[e - 1].marks.remove(mark);
                e -= 1;
            }
        }
    }
}

/// How a mark is opened: as Markdown's delimiters, or, with `tags`, as HTML.
fn open_delim(m: Marks, tags: bool) -> &'static str {
    match (m, tags) {
        (Marks::BOLD, false) => "**",
        (Marks::ITALIC, false) => "*",
        (Marks::STRIKE, false) => "~~",
        (Marks::BOLD, true) => "<strong>",
        (Marks::ITALIC, true) => "<em>",
        (Marks::STRIKE, true) => "<s>",
        (Marks::UNDERLINE, _) => "<u>",
        _ => "",
    }
}

fn close_delim(m: Marks, tags: bool) -> &'static str {
    match (m, tags) {
        (Marks::BOLD, true) => "</strong>",
        (Marks::ITALIC, true) => "</em>",
        (Marks::STRIKE, true) => "</s>",
        (Marks::UNDERLINE, _) => "</u>",
        (m, false) => open_delim(m, false),
        _ => "",
    }
}

/// A block's text with its formatting, as Markdown. `continuation` is what
/// starts each line after a hard break: indentation in a list, `> ` in a quote.
///
/// # Why it checks itself
///
/// Emphasis in CommonMark is decided by a page of rules about which `*`
/// can open and which can close, and some perfectly reasonable formatting —
/// italic running straight into bold italic mid-word — has no spelling in
/// delimiters that reads back as itself. Rather than reimplementing those
/// rules to predict it, the Markdown is read back, and where it does not say
/// what was meant, that block is written with `<strong>` and `<em>` instead,
/// which cannot be misread. Ordinary text never needs it.
fn inline(block: &Block, continuation: &str) -> String {
    let mut glyphs: Vec<Glyph> = block
        .text()
        .chars()
        .zip(block.styles())
        .map(|(c, s)| Glyph {
            c,
            marks: s.marks,
            link: s.link.clone(),
        })
        .collect();
    hug(&mut glyphs);
    // A code span cannot hold a line break; the break is written as one,
    // outside the code on either side of it.
    for g in glyphs.iter_mut().filter(|g| g.c == '\n') {
        g.marks.remove(Marks::CODE);
    }
    // Whitespace at the very end is written as entities (see `render`), so
    // formatting on it — which nobody can see — goes.
    for g in glyphs
        .iter_mut()
        .rev()
        .take_while(|g| matches!(g.c, ' ' | '\t' | '\n'))
    {
        g.marks = Marks::NONE;
        g.link = None;
    }

    let plain = render(&glyphs, continuation, false);
    let probe = if continuation.is_empty() {
        plain.clone()
    } else {
        render(&glyphs, "", false)
    };
    if reads_back(&glyphs, &probe) {
        plain
    } else {
        render(&glyphs, continuation, true)
    }
}

/// Whether Markdown written for `glyphs` reads back as exactly them.
fn reads_back(glyphs: &[Glyph], md: &str) -> bool {
    if glyphs.is_empty() {
        return true;
    }
    let blocks = read_blocks(md);
    let [block] = blocks.as_slice() else {
        return false;
    };
    block.len() == glyphs.len()
        && block
            .text()
            .chars()
            .zip(block.styles())
            .zip(glyphs)
            .all(|((c, s), g)| c == g.c && s.marks == g.marks && s.link == g.link)
}

fn render(glyphs: &[Glyph], continuation: &str, tags: bool) -> String {
    // Whitespace at the end of a block would be dropped on reading, and a
    // break there is not a break at all, so they are spelled out.
    let end = glyphs
        .iter()
        .rposition(|g| !matches!(g.c, ' ' | '\t' | '\n'))
        .map_or(0, |i| i + 1);
    let (glyphs, tail) = glyphs.split_at(end);
    let mut out = String::new();
    let mut i = 0;
    let mut line_start = true;
    while i < glyphs.len() {
        // A link groups everything that shares its address.
        let link = glyphs[i].link.clone();
        let mut j = i;
        while j < glyphs.len() && glyphs[j].link == link {
            j += 1;
        }
        if link.is_some() {
            out.push('[');
        }
        marked(&glyphs[i..j], continuation, tags, &mut out, &mut line_start);
        if let Some(url) = link {
            out.push_str("](");
            out.push_str(&url_text(&url));
            out.push(')');
        }
        i = j;
    }
    for (i, g) in tail.iter().enumerate() {
        out.push_str(match g.c {
            ' ' => "&#32;",
            '\t' => "&#9;",
            // A `<br>` that starts a block is an HTML block, not this
            // block's text; the character itself, as an entity, is not.
            '\n' if i == 0 && glyphs.is_empty() => "&#10;",
            _ => "<br>",
        });
    }
    out
}

/// A stretch with one link (or none), its marks opened and closed as they
/// change, innermost closed first.
fn marked(
    glyphs: &[Glyph],
    continuation: &str,
    tags: bool,
    out: &mut String,
    line_start: &mut bool,
) {
    let order = [Marks::BOLD, Marks::ITALIC, Marks::STRIKE, Marks::UNDERLINE];
    let mut open: Vec<Marks> = Vec::new();
    let mut i = 0;
    while i < glyphs.len() {
        let marks = glyphs[i].marks;
        let mut j = i;
        while j < glyphs.len() && glyphs[j].marks == marks {
            j += 1;
        }
        // Close down to the deepest mark that is still on.
        let keep = open.iter().take_while(|m| marks.contains(**m)).count();
        while open.len() > keep {
            out.push_str(close_delim(open.pop().unwrap(), tags));
        }
        for m in order {
            if marks.contains(m) && !open.contains(&m) {
                out.push_str(open_delim(m, tags));
                open.push(m);
            }
        }
        let text: String = glyphs[i..j].iter().map(|g| g.c).collect();
        if marks.contains(Marks::CODE) {
            out.push_str(&code_span(&text));
            *line_start = false;
        } else {
            escape(&text, continuation, out, line_start);
        }
        i = j;
    }
    while let Some(m) = open.pop() {
        out.push_str(close_delim(m, tags));
    }
}

fn code_span(text: &str) -> String {
    let fence = "`".repeat(longest_run(text, '`') + 1);
    let pad = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && !text.trim().is_empty());
    let text = text.replace('\n', " ");
    if pad {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

fn url_text(url: &str) -> String {
    if url.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

/// Text written so it reads back as itself, escaping only what Markdown
/// would otherwise take as syntax.
fn escape(text: &str, continuation: &str, out: &mut String, line_start: &mut bool) {
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };
        let next = chars.get(i + 1).copied();
        if c == '\n' {
            out.push_str("\\\n");
            out.push_str(continuation);
            *line_start = true;
            continue;
        }
        if *line_start {
            match c {
                ' ' | '\t' => {
                    // Leading spaces would be dropped, or four of them would
                    // make the line code.
                    out.push_str("&#32;");
                    continue;
                }
                '#' | '>' | '+' | '-' | '=' | '|' => out.push('\\'),
                d if d.is_ascii_digit() => {
                    // `1.` at the start of a line would be a list.
                    let digits: String = chars[i..]
                        .iter()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if matches!(chars.get(i + digits.chars().count()), Some('.') | Some(')')) {
                        out.push_str(&digits);
                        out.push('\\');
                        let rest: String = chars[i + digits.len()..].iter().collect();
                        *line_start = false;
                        escape(&rest, continuation, out, line_start);
                        return;
                    }
                }
                _ => {}
            }
        }
        *line_start = false;
        let needs = match c {
            '\\' | '*' | '`' | '[' | ']' => true,
            '~' => next != Some('/'),
            '_' => {
                !(prev.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric))
            }
            '<' => next.is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')),
            '&' => looks_like_entity(&chars[i + 1..]),
            '!' => next == Some('['),
            _ => false,
        };
        if needs {
            out.push('\\');
        }
        out.push(c);
    }
}

/// `&amp;`, `&#32;`, `&x27;` — text after an ampersand that HTML would read.
fn looks_like_entity(rest: &[char]) -> bool {
    let name: String = rest
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '#')
        .collect();
    !name.is_empty() && rest.get(name.chars().count()) == Some(&';')
}

// --- the trailer ---

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Span {
    from: (usize, usize),
    to: (usize, usize),
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredComment {
    #[serde(flatten)]
    comment: Comment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<Span>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSuggestion {
    #[serde(flatten)]
    suggestion: Suggestion,
    at: Span,
    /// The text the suggestion replaces, to find it again.
    quote: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    v: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    comments: Vec<StoredComment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    suggestions: Vec<StoredSuggestion>,
}

fn span(doc: &Document, id: u32) -> Option<Span> {
    doc.anchor_range(id).map(|(a, b)| Span {
        from: (a.block, a.offset),
        to: (b.block, b.offset),
    })
}

fn trailer(doc: &Document) -> Option<String> {
    let comments: Vec<StoredComment> = doc
        .comments
        .iter()
        .map(|c| StoredComment {
            comment: c.clone(),
            at: span(doc, c.id),
        })
        .collect();
    let suggestions: Vec<StoredSuggestion> = doc
        .suggestions
        .iter()
        .filter_map(|s| {
            let at = span(doc, s.id)?;
            let quote =
                doc.text_between(Pos::new(at.from.0, at.from.1), Pos::new(at.to.0, at.to.1));
            Some(StoredSuggestion {
                suggestion: s.clone(),
                at,
                quote,
            })
        })
        .collect();
    if comments.is_empty() && suggestions.is_empty() {
        return None;
    }
    let json = serde_json::to_string_pretty(&Meta {
        v: 1,
        comments,
        suggestions,
    })
    .ok()?;
    // `-->` inside a string would end the HTML comment early.
    let json = json.replace("-->", "--\\u003e");
    Some(format!("{TRAILER}\n{json}\n-->\n"))
}

/// The Markdown, and the trailer's JSON if the file ends in one.
fn split_trailer(src: &str) -> (&str, Option<Meta>) {
    let Some(at) = src.rfind(TRAILER) else {
        return (src, None);
    };
    if at > 0 && !src[..at].ends_with('\n') {
        return (src, None);
    }
    let rest = src[at + TRAILER.len()..].trim_end();
    let Some(json) = rest.strip_suffix("-->") else {
        return (src, None);
    };
    match serde_json::from_str::<Meta>(json.trim()) {
        Ok(meta) => (&src[..at], Some(meta)),
        Err(_) => (src, None),
    }
}

/// Where `quote` is now: at `span` if it is still there, otherwise its first
/// appearance within one block.
fn locate(doc: &Document, span: Option<Span>, quote: &str) -> Option<(Pos, Pos)> {
    if quote.is_empty() {
        return None;
    }
    if let Some(s) = span {
        let (a, b) = (Pos::new(s.from.0, s.from.1), Pos::new(s.to.0, s.to.1));
        if s.from.0 < doc.len()
            && s.to.0 < doc.len()
            && doc.clamp(a) == a
            && doc.clamp(b) == b
            && doc.text_between(a, b) == quote
        {
            return Some((a, b));
        }
    }
    for (i, block) in doc.blocks.iter().enumerate() {
        if let Some(byte) = block.text().find(quote) {
            let start = block.text()[..byte].chars().count();
            return Some((
                Pos::new(i, start),
                Pos::new(i, start + quote.chars().count()),
            ));
        }
    }
    None
}

fn restore(doc: &mut Document, meta: Meta) {
    for stored in meta.comments {
        let c = stored.comment;
        if let Some((a, b)) = locate(doc, stored.at, &c.quote) {
            doc.restyle(a, b, |s| s.add_anchor(c.id));
        }
        doc.comments.push(c);
    }
    for stored in meta.suggestions {
        // A suggestion whose text has gone is about nothing; it is dropped.
        if let Some((a, b)) = locate(doc, Some(stored.at), &stored.quote) {
            doc.restyle(a, b, |s| s.add_anchor(stored.suggestion.id));
            doc.suggestions.push(stored.suggestion);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Author, Note};

    /// Reads and writes back: the text should come out as it went in.
    fn round(src: &str) -> String {
        write(&read(src))
    }

    #[track_caller]
    fn same(src: &str) {
        assert_eq!(round(src), src);
    }

    #[test]
    fn plain_structure_survives() {
        same(
            "# Title\n\nA paragraph.\n\n## Section\n\n- one\n- two\n\n1. first\n2. second\n\n> quoted\n\n---\n\n```rust\nfn main() {}\n```\n",
        );
    }

    #[test]
    fn inline_formatting_survives() {
        same(
            "Some **bold**, *italic*, ~~gone~~, <u>under</u>, `code` and a [link](https://example.com).\n",
        );
        same("***both*** and **bold *then italic***\n");
        same("[**bold link**](https://quark.app/a_b)\n");
    }

    #[test]
    fn nested_and_task_lists() {
        same("- a\n  - b\n    - c\n- d\n");
        same("1. one\n   - sub\n2. two\n");
        same("- [ ] todo\n- [x] done\n  - [ ] nested\n");
    }

    #[test]
    fn numbering_restarts_after_other_blocks() {
        let blocks = vec![
            Block::with_text(BlockKind::Numbered { depth: 0 }, "a"),
            Block::with_text(BlockKind::Numbered { depth: 0 }, "b"),
            Block::paragraph("between"),
            Block::with_text(BlockKind::Numbered { depth: 0 }, "c"),
        ];
        assert_eq!(write_blocks(&blocks), "1. a\n2. b\n\nbetween\n\n1. c\n");
    }

    #[test]
    fn text_that_looks_like_markdown_is_escaped() {
        for text in [
            "# not a heading",
            "- not a list",
            "1. not a list",
            "2) nor this",
            "*stars* and _under_ and [brackets]",
            "a < b and <div>",
            "snake_case stays",
            "~/Documents and ~~x~~",
            "R&D, &amp; literally",
            "    four spaces",
        ] {
            let doc = Document::from_blocks(vec![Block::paragraph(text)]);
            let md = write(&doc);
            let back = read(&md);
            assert_eq!(back.blocks.len(), 1, "{text:?} → {md:?}");
            assert_eq!(back.blocks[0].text(), text, "{text:?} → {md:?}");
            assert_eq!(
                back.blocks[0].kind,
                BlockKind::Paragraph,
                "{text:?} → {md:?}"
            );
            assert!(
                back.blocks[0].styles().iter().all(|s| s.marks.is_empty()),
                "{text:?} → {md:?}"
            );
        }
        assert_eq!(
            write(&Document::from_blocks(vec![Block::paragraph("snake_case")])),
            "snake_case\n"
        );
    }

    #[test]
    fn formatting_delimiters_cannot_spell_falls_back_to_tags() {
        let mut b = Block::paragraph("break[ ]");
        b.restyle(0..5, |s| s.marks.insert(Marks::ITALIC));
        b.restyle(5..8, |s| s.marks = Marks::BOLD.with(Marks::ITALIC, true));
        let md = write_blocks(std::slice::from_ref(&b));
        assert_eq!(md, "<em>break<strong>\\[ \\]</strong></em>\n");
        let back = read_blocks(&md);
        assert_eq!(back[0].styles(), b.styles());
    }

    #[test]
    fn whitespace_at_the_edge_of_bold_moves_outside() {
        let mut b = Block::paragraph("make this bold");
        b.restyle(4..10, |s| s.marks.insert(Marks::BOLD));
        assert_eq!(write_blocks(&[b]), "make **this** bold\n");
    }

    #[test]
    fn a_heading_may_end_in_a_hash() {
        same("## Item \\#\n");
        same("# C#\n");
        assert_eq!(read("## Item \\#\n").blocks[0].text(), "Item #");
    }

    #[test]
    fn line_breaks_inside_blocks() {
        same("first\\\nsecond\n");
        same("ends in spaces&#32;&#32;\n");
        same("ends in a break<br>\n");
        same("- &#10;\n- &#10;<br>\n");
        same("> quote\\\n> &#32;then space\n");
        same("- item\\\n  more\n");
        same("> quote\\\n> more\n");
    }

    #[test]
    fn empty_lines_are_kept_but_a_trailing_one_is_not() {
        let doc = Document::from_blocks(vec![
            Block::paragraph("a"),
            Block::paragraph(""),
            Block::paragraph("b"),
            Block::paragraph(""),
        ]);
        let md = write(&doc);
        assert_eq!(md, "a\n\n&nbsp;\n\nb\n");
        let back = read(&md);
        assert_eq!(back.blocks.len(), 3);
        assert!(back.blocks[1].is_empty());
    }

    #[test]
    fn empty_items_and_quotes_stay() {
        let doc = Document::from_blocks(vec![
            Block::new(BlockKind::Bullet { depth: 0 }),
            Block::paragraph("x"),
            Block::new(BlockKind::Quote),
            Block::paragraph("y"),
            Block::new(BlockKind::Heading(2)),
        ]);
        let back = read(&write(&doc));
        let kinds: Vec<_> = back.blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(
            kinds,
            doc.blocks
                .iter()
                .map(|b| b.kind.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn code_with_backticks_inside() {
        same(
            "```\nlet s = \"```\";\n```\n"
                .replace("```\n", "````\n")
                .as_str(),
        );
        same("Use ``a`b`` here\n");
        assert_eq!(round("Use `` `tick` `` here"), "Use `` `tick` `` here\n");
    }

    #[test]
    fn markdown_from_elsewhere_is_read() {
        let doc = read(
            "Title\n=====\n\n* star list\n+ plus list\n\n__strong__ and _em_\n\n    indented code\n\n![a cat](cat.png)\n",
        );
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds[0], BlockKind::Heading(1));
        assert_eq!(kinds[1], BlockKind::Bullet { depth: 0 });
        assert_eq!(kinds[2], BlockKind::Bullet { depth: 0 });
        assert!(doc.blocks[3].styles()[0].marks.contains(Marks::BOLD));
        assert_eq!(
            kinds[4],
            BlockKind::Code {
                lang: String::new()
            }
        );
        assert_eq!(doc.blocks[5].text(), "a cat");
        assert_eq!(doc.blocks[5].styles()[0].link.as_deref(), Some("cat.png"));
    }

    #[test]
    fn comments_and_suggestions_ride_in_the_trailer() {
        let mut doc = read("Intro.\n\nThe figure that carries it.\n");
        doc.restyle(Pos::new(1, 4), Pos::new(1, 10), |s| s.add_anchor(1));
        doc.comments.push(Comment {
            id: 1,
            quote: "figure".into(),
            notes: vec![Note::now(Author::You, "Which -->?")],
            resolved: false,
        });
        doc.restyle(Pos::new(0, 0), Pos::new(0, 5), |s| s.add_anchor(2));
        doc.suggestions.push(Suggestion {
            id: 2,
            markdown: "**Opening**".into(),
            why: "stronger".into(),
        });
        let md = write(&doc);
        assert!(
            md.starts_with("Intro.\n\nThe figure that carries it.\n\n<!-- quark-doc\n"),
            "{md}"
        );
        assert_eq!(md.matches("-->").count(), 1, "{md}");

        let back = read(&md);
        assert_eq!(back.blocks.len(), 2);
        assert_eq!(
            back.anchor_range(1),
            Some((Pos::new(1, 4), Pos::new(1, 10)))
        );
        assert_eq!(back.comment(1).unwrap().notes[0].text, "Which -->?");
        assert_eq!(back.anchor_range(2), Some((Pos::new(0, 0), Pos::new(0, 5))));
        assert_eq!(back.suggestions[0].markdown, "**Opening**");
        // And again, unchanged.
        assert_eq!(write(&back), md);
    }

    #[test]
    fn a_comment_finds_its_text_after_an_edit_elsewhere() {
        let mut doc = read("one\n\ntwo words here\n");
        doc.restyle(Pos::new(1, 4), Pos::new(1, 9), |s| s.add_anchor(3));
        doc.comments.push(Comment {
            id: 3,
            quote: "words".into(),
            notes: vec![],
            resolved: false,
        });
        let md = write(&doc).replacen("one\n\n", "one\n\nadded above\n\n", 1);
        let back = read(&md);
        assert_eq!(back.anchor_range(3), Some((Pos::new(2, 4), Pos::new(2, 9))));
        // Gone entirely: the comment stays, unanchored.
        let md = write(&doc).replace("two words here", "rewritten");
        let back = read(&md);
        assert_eq!(back.comments.len(), 1);
        assert_eq!(back.anchor_range(3), None);
    }

    #[test]
    fn a_broken_trailer_is_left_as_text() {
        let doc = read("text\n\n<!-- quark-doc\n{not json\n-->\n");
        assert_eq!(doc.blocks.len(), 1);
        assert!(doc.comments.is_empty());
    }
}
