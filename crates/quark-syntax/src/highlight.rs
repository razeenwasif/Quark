//! Parsing a buffer and asking what colour each visible character is.
//!
//! Two decisions shape everything here.
//!
//! The parser reads the rope directly, in chunks, rather than being handed a
//! `String`. Flattening a large buffer on every keystroke would cost more than
//! the parse.
//!
//! The query runs over the **visible window only**. A thousand-line file has a
//! thousand lines of syntax and fifty lines on screen, and colouring the other
//! nine hundred and fifty is work whose result is discarded. Parsing still
//! covers the whole document — it has to, since syntax at the top decides what
//! things mean at the bottom — but that is what tree-sitter is fast at.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use quark_text::Buffer;
use serde::{Deserialize, Serialize};
use tree_sitter::{
    InputEdit, Node, ParseOptions, Parser, Point, Query, QueryCursor, StreamingIterator,
    TextProvider, Tree,
};

use crate::language::{self, Language};
use crate::scope::Scope;

/// A coloured run inside one line, in **character** columns — the rope's
/// units, not tree-sitter's bytes. The conversion happens here so the front
/// end never has to know that tree-sitter counts differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub scope: Scope,
}

/// Compiled queries, kept per language for the life of the process.
///
/// Compiling Rust's `highlights.scm` measured 13 ms. Paying that every time a
/// file is opened is a visible pause in a session that opens a lot of them,
/// and the result is byte-for-byte the same each time.
static QUERIES: LazyLock<Mutex<HashMap<&'static str, Arc<Query>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct Syntax {
    /// The language id, for the statusline. A `String` rather than a
    /// `&'static str` because a grammar a plugin brought has no entry in the
    /// static table to borrow one from.
    id: String,
    parser: Parser,
    query: Arc<Query>,
    tree: Option<Tree>,
    /// The buffer version the tree was parsed from, so an unchanged buffer is
    /// not re-parsed on every cursor move.
    parsed: Option<u64>,
    /// The second grammar, for languages that hand part of themselves over.
    /// Its tree is rebuilt whole rather than incrementally: the ranges it
    /// covers move whenever a paragraph does, and a tree edited against the
    /// wrong ranges colours the file wrong rather than failing.
    inline: Option<Inline>,
    parse_budget: Option<Duration>,
    limited: bool,
}

struct Inline {
    inside: &'static str,
    parser: Parser,
    query: Arc<Query>,
    tree: Option<Tree>,
}

impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Syntax")
            .field("language", &self.id)
            .field("parsed", &self.parsed)
            .finish()
    }
}

impl Syntax {
    /// Build a highlighter for a file, if there is a grammar for it.
    ///
    /// `None` for an unknown extension is an ordinary answer — a plain-text
    /// file is a file, not a failure.
    pub fn for_path(path: &std::path::Path) -> Option<Self> {
        let language = language::for_path(path)?;
        let grammar = language.grammar();

        let mut parser = Parser::new();
        // A grammar built against an incompatible tree-sitter is a build
        // problem, not a runtime one, and there is nothing useful to do about
        // it here beyond declining to highlight.
        parser.set_language(&grammar).ok()?;

        let query = compiled(language, &grammar)?;

        // A companion that will not build is dropped rather than fatal: the
        // block half still colours headings and fences, which is more than
        // plain text.
        let inline = language.inline.as_ref().and_then(|companion| {
            let grammar = (companion.grammar)();
            let mut parser = Parser::new();
            parser.set_language(&grammar).ok()?;
            Some(Inline {
                inside: companion.inside,
                query: cached(companion.id, &grammar, companion.queries)?,
                parser,
                tree: None,
            })
        });

        Some(Self {
            id: language.id.to_string(),
            parser,
            query,
            tree: None,
            parsed: None,
            inline,
            parse_budget: None,
            limited: false,
        })
    }


    pub fn language(&self) -> &str {
        &self.id
    }

    /// A cooperative parser limit for large files. Once exceeded, keep text
    /// editing cheap until the limit is removed or this highlighter is rebuilt.
    pub fn limit_parse_time(&mut self, budget: Option<Duration>) {
        self.parse_budget = budget;
        if budget.is_none() && self.limited {
            self.limited = false;
            self.parsed = None;
        }
    }

    pub fn is_limited(&self) -> bool {
        self.limited
    }

    fn pause(&mut self) {
        self.limited = true;
        self.parser.reset();
        self.tree = None;
        if let Some(inline) = &mut self.inline {
            inline.parser.reset();
            inline.tree = None;
        }
    }

    /// Re-parse if the buffer has moved since the last time.
    ///
    /// Incrementally where possible. That is not premature: a full re-parse
    /// measured 36 ms on a ten-thousand-line file, which is a visible stutter
    /// on every keystroke, while the same edit re-parsed incrementally is
    /// bounded by the size of the change instead of the size of the file. The
    /// numbers are in `tests/latency.rs`.
    fn sync(&mut self, buffer: &Buffer) {
        if self.limited {
            return;
        }
        if self.parsed == Some(buffer.version()) && self.tree.is_some() {
            return;
        }

        // Tell the old tree what moved before re-parsing against it. A tree
        // that disagrees with the text by one byte does not fail — it colours
        // the rest of the file wrong, which is much worse. So anything less
        // than a complete account of the changes falls back to starting over.
        let old = match (self.tree.take(), self.parsed) {
            (Some(mut tree), Some(from)) => match buffer.changes_since(from) {
                Some(changes) => {
                    for change in changes {
                        tree.edit(&input_edit(&change));
                    }
                    Some(tree)
                }
                None => None,
            },
            _ => None,
        };

        let deadline = self.parse_budget.map(|budget| Instant::now() + budget);
        let mut progress = |_: &tree_sitter::ParseState| {
            if deadline.is_some_and(|end| Instant::now() >= end) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let text = buffer.text();
        // Read straight out of the rope. `parse_with_options` asks for bytes
        // at an offset and gets whatever contiguous chunk lives there, which
        // is exactly how ropey stores them — no flattening the buffer into a
        // `String` on every keystroke.
        let tree = self.parser.parse_with_options(
            &mut |offset: usize, _| {
                if offset >= text.len_bytes() {
                    return &[][..];
                }
                let (chunk, chunk_start, _, _) = text.chunk_at_byte(offset);
                &chunk.as_bytes()[offset - chunk_start..]
            },
            old.as_ref(),
            deadline.map(|_| ParseOptions::new().progress_callback(&mut progress)),
        );
        if tree.is_none() && deadline.is_some() {
            self.pause();
            return;
        }
        self.parsed = Some(buffer.version());
        self.tree = tree;
        if self.sync_inline(buffer, deadline) {
            self.pause();
        }
    }

    /// Parse the ranges the first grammar handed over.
    ///
    /// One parse rather than one per range: tree-sitter's `included_ranges`
    /// takes the lot and produces a single tree covering all of them, which
    /// is also what keeps the offsets absolute — the captures come back in
    /// the document's own bytes and need no rebasing.
    ///
    /// Whole rather than incremental. The ranges themselves move whenever a
    /// paragraph does, and a tree edited against ranges that have shifted
    /// colours the file wrong rather than failing, which is the worse of the
    /// two. Markdown files are prose-sized and the second parse is over the
    /// prose only.
    fn sync_inline(&mut self, buffer: &Buffer, deadline: Option<Instant>) -> bool {
        let (Some(inline), Some(tree)) = (self.inline.as_mut(), self.tree.as_ref()) else {
            return false;
        };

        let mut ranges = Vec::new();
        let mut walk = tree.walk();
        let mut nodes = vec![tree.root_node()];
        while let Some(node) = nodes.pop() {
            if deadline.is_some_and(|end| Instant::now() >= end) {
                return true;
            }
            if node.kind() == inline.inside {
                ranges.push(tree_sitter::Range {
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    start_point: node.start_position(),
                    end_point: node.end_position(),
                });
                continue;
            }
            nodes.extend(node.children(&mut walk));
        }

        // Sorted before they are handed over: the walk above finds them in
        // whatever order it pops them, and tree-sitter refuses ranges that are
        // not in order and disjoint.
        ranges.sort_by_key(|r| r.start_byte);
        if ranges.is_empty() || inline.parser.set_included_ranges(&ranges).is_err() {
            inline.tree = None;
            return false;
        }

        let text = buffer.text();
        let mut progress = |_: &tree_sitter::ParseState| {
            if deadline.is_some_and(|end| Instant::now() >= end) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        inline.tree = inline.parser.parse_with_options(
            &mut |offset: usize, _| {
                if offset >= text.len_bytes() {
                    return &[][..];
                }
                let (chunk, chunk_start, _, _) = text.chunk_at_byte(offset);
                &chunk.as_bytes()[offset - chunk_start..]
            },
            None,
            deadline.map(|_| ParseOptions::new().progress_callback(&mut progress)),
        );
        inline.tree.is_none() && deadline.is_some()
    }

    /// The coloured runs for a window of lines, one list per line.
    ///
    /// One list per visible line, or empty when the parse budget was exceeded
    /// and the caller should render plain text.
    pub fn spans(&mut self, buffer: &Buffer, first_line: usize, count: usize) -> Vec<Vec<Span>> {
        self.sync(buffer);
        if self.limited {
            return Vec::new();
        }

        let text = buffer.text();
        let last_line = (first_line + count).min(text.len_lines());
        if first_line >= last_line {
            return Vec::new();
        }
        let lines = last_line - first_line;

        let Some(tree) = &self.tree else {
            return vec![Vec::new(); lines];
        };

        let window = text.line_to_byte(first_line)
            ..text
                .line_to_byte(
                    last_line
                        .min(text.len_lines().saturating_sub(1))
                        .max(first_line),
                )
                .max(text.line_to_byte(first_line));
        // The last line has no following line to bound it, so extend to the end.
        let window = if last_line >= text.len_lines() {
            window.start..text.len_bytes()
        } else {
            window
        };

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(window.clone());

        // Collected rather than painted as they arrive, so overlaps can be
        // resolved by a rule instead of by iteration order.
        let mut captures: Vec<(Range<usize>, usize, Scope)> = Vec::new();
        let names = self.query.capture_names();
        let mut matches = cursor.matches(&self.query, tree.root_node(), Rope(text));

        while let Some(found) = matches.next() {
            for capture in found.captures() {
                let Some(scope) = Scope::from_capture(names[capture.index as usize]) else {
                    continue;
                };
                let node = capture.node;
                captures.push((node.byte_range(), found.pattern_index, scope));
            }
        }

        // The second grammar's captures join the same list. They are inside
        // the first's ranges by construction, so they sort as the narrower
        // spans they are and paint over the block colour — which is what you
        // want: a code span inside a paragraph is code, not paragraph.
        if let Some(inline) = &self.inline {
            if let Some(tree) = &inline.tree {
                let mut cursor = QueryCursor::new();
                cursor.set_byte_range(window.clone());
                let names = inline.query.capture_names();
                let mut matches = cursor.matches(&inline.query, tree.root_node(), Rope(text));
                while let Some(found) = matches.next() {
                    for capture in found.captures() {
                        if let Some(scope) = Scope::from_capture(names[capture.index as usize]) {
                            captures.push((capture.node.byte_range(), found.pattern_index, scope));
                        }
                    }
                }
            }
        }

        // Outermost first, and among equal spans the *last* pattern first, so
        // that painting in this order leaves the narrowest span and the
        // earliest pattern on top. Depth is what makes `Vec` in `Vec<String>`
        // a type rather than whatever encloses it; pattern order is the
        // convention every `highlights.scm` is written to.
        captures.sort_by(|a, b| {
            a.0.start
                .cmp(&b.0.start)
                .then(b.0.end.cmp(&a.0.end))
                .then(b.1.cmp(&a.1))
        });

        paint(text, first_line, lines, &captures)
    }
}

/// Turn byte ranges into per-line character runs.
fn paint(
    text: &ropey::Rope,
    first_line: usize,
    lines: usize,
    captures: &[(Range<usize>, usize, Scope)],
) -> Vec<Vec<Span>> {
    let mut out = Vec::with_capacity(lines);

    for row in 0..lines {
        let line = first_line + row;
        let line_start = text.line_to_byte(line);
        let line_end = if line + 1 < text.len_lines() {
            text.line_to_byte(line + 1)
        } else {
            text.len_bytes()
        };
        let source = text.line(line);
        // A rope's line carries its terminator and a rendered line does not.
        // Left in, a comment's span runs one character past the end of the
        // line it is on, and the front end draws a coloured box into the
        // margin.
        let mut width = source.len_chars();
        while width > 0 && matches!(source.char(width - 1), '\n' | '\r') {
            width -= 1;
        }

        // One slot per character, painted in the sorted order so later writes
        // (narrower spans) win.
        let mut colour: Vec<Option<Scope>> = vec![None; width];
        for (range, _, scope) in captures {
            if range.end <= line_start || range.start >= line_end {
                continue;
            }
            let from = range.start.max(line_start) - line_start;
            let to = (range.end.min(line_end) - line_start).min(line_end - line_start);
            // Byte offsets within the line become character offsets. They
            // differ the moment a line contains anything non-ASCII, and a
            // column that is two out puts a colour on the wrong character.
            let from = byte_to_char(&source, from);
            let to = byte_to_char(&source, to).min(width);
            for slot in colour.iter_mut().take(to).skip(from) {
                *slot = Some(*scope);
            }
        }

        // Adjacent characters of the same colour become one span, because the
        // page renders one element per span and a per-character element is a
        // few thousand nodes a line.
        let mut spans: Vec<Span> = Vec::new();
        let mut start = 0;
        while start < width {
            let Some(scope) = colour[start] else {
                start += 1;
                continue;
            };
            let mut end = start + 1;
            while end < width && colour[end] == Some(scope) {
                end += 1;
            }
            spans.push(Span { start, end, scope });
            start = end;
        }
        out.push(spans);
    }
    out
}

fn byte_to_char(line: &ropey::RopeSlice, byte: usize) -> usize {
    let byte = byte.min(line.len_bytes());
    line.byte_to_char(byte)
}

/// Lets the query engine read predicate text (`#eq?`, `#match?`) straight out
/// of the rope. Without one, every predicate silently fails and whole
/// categories of highlight disappear.
struct Rope<'a>(&'a ropey::Rope);

impl<'a> TextProvider<&'a [u8]> for Rope<'a> {
    type I = Chunks<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        Chunks {
            rope: self.0,
            at: node.start_byte(),
            end: node.end_byte(),
        }
    }
}

struct Chunks<'a> {
    rope: &'a ropey::Rope,
    at: usize,
    end: usize,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.at >= self.end {
            return None;
        }
        let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(self.at);
        let from = self.at - chunk_start;
        let to = (self.end - chunk_start).min(chunk.len());
        self.at = chunk_start + to;
        Some(&chunk.as_bytes()[from..to])
    }
}

/// Translate a buffer change into the shape tree-sitter wants.
///
/// A straight renaming — which is the point. `quark-text` describes what
/// happened to some text without knowing who is listening, and the knowledge
/// that a parser is listening lives here.
fn input_edit(change: &quark_text::Change) -> InputEdit {
    let point = |p: quark_text::Point| Point {
        row: p.row,
        column: p.column,
    };
    InputEdit {
        start_byte: change.start_byte,
        old_end_byte: change.old_end_byte,
        new_end_byte: change.new_end_byte,
        start_position: point(change.start_point),
        old_end_position: point(change.old_end_point),
        new_end_position: point(change.new_end_point),
    }
}

/// The compiled highlight query for a language, built once.
fn compiled(language: &'static Language, grammar: &tree_sitter::Language) -> Option<Arc<Query>> {
    cached(language.id, grammar, language.queries)
}

fn cached(
    key: &'static str,
    grammar: &tree_sitter::Language,
    queries: &[&str],
) -> Option<Arc<Query>> {
    // A poisoned lock means some other thread panicked mid-insert. The cache
    // holds nothing that a panic could have left inconsistent, so recovering
    // is better than taking the editor down with it.
    let mut cache = QUERIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(query) = cache.get(key) {
        return Some(Arc::clone(query));
    }
    let query = Arc::new(Query::new(grammar, &queries.join("\n")).ok()?);
    cache.insert(key, Arc::clone(&query));
    Some(query)
}
