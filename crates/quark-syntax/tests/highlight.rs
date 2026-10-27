//! What the colours actually land on.
//!
//! Asserting "there are some spans" would pass on almost any bug. These
//! assert which characters got which colour, by reading the text back out of
//! the span — so a range that is off by one, or counted in bytes, fails.

use quark_syntax::{Scope, Span, Syntax};
use quark_text::Buffer;

fn latex(text: &str) -> (Syntax, Buffer) {
    let syntax = Syntax::for_path(std::path::Path::new("x.tex")).expect("a LaTeX grammar");
    (syntax, Buffer::from_text(text))
}

/// A document of some `sections` sections, of the shapes a paper has:
/// headings, environments, mathematics, citations, comments.
pub fn paper(sections: usize) -> String {
    let mut text = String::from("\\documentclass[11pt]{article}\n\\usepackage{amsmath,graphicx}\n\\title{Notes on Sampling}\n\\begin{document}\n\\maketitle\n");
    for n in 0..sections {
        text.push_str(&format!(
            "\\section{{Part {n}}}\\label{{sec:{n}}}\n\
             % what this part shows\n\
             We sample $x_{n} \\sim p(x)$ as in \\cite{{ref{n}}}, and see Section~\\ref{{sec:{n}}}.\n\
             \\begin{{equation}}\n  E[f] = \\int f(x)\\, p(x)\\, dx \\label{{eq:{n}}}\n\\end{{equation}}\n\
             \\begin{{itemize}}\n  \\item a point, with \\textbf{{weight}} $w_i$\n  \\item another\n\\end{{itemize}}\n\n"
        ));
    }
    text.push_str("\\end{document}\n");
    text
}

/// The text a span covers, on its line.
fn covered(buffer: &Buffer, line: usize, span: &Span) -> String {
    buffer
        .line(line)
        .chars()
        .skip(span.start)
        .take(span.end - span.start)
        .collect()
}

/// Every coloured run on a line, as (text, scope).
fn runs(buffer: &Buffer, spans: &[Span], line: usize) -> Vec<(String, Scope)> {
    spans
        .iter()
        .map(|s| (covered(buffer, line, s), s.scope))
        .collect()
}

#[test]
fn mathematics_is_coloured_including_its_dollars() {
    let (mut syntax, buffer) = latex("Let $x^2 + 1$ be.\n");
    let spans = &syntax.spans(&buffer, 0, 1)[0];
    let strings: Vec<_> = runs(&buffer, spans, 0)
        .into_iter()
        .filter(|(_, scope)| *scope == Scope::String)
        .collect();
    assert_eq!(
        strings,
        vec![("$x^2 + 1$".to_string(), Scope::String)],
        "the dollars are part of the mathematics, not punctuation beside it"
    );
}

#[test]
fn columns_are_characters_not_bytes() {
    // The whole reason spans are converted here. Every character before the
    // command is multi-byte, so a span computed in bytes lands well past it.
    let (mut syntax, buffer) = latex("% ★★★★\n\\section{Intro}\n");
    let spans = &syntax.spans(&buffer, 1, 1)[0];
    let found = runs(&buffer, spans, 1);
    assert!(found.contains(&("\\section".to_string(), Scope::Keyword)), "{found:?}");

    let (mut syntax, buffer) = latex("Café ★ $x$.\n");
    let spans = &syntax.spans(&buffer, 0, 1)[0];
    let strings: Vec<_> = runs(&buffer, spans, 0).into_iter().filter(|(_, s)| *s == Scope::String).collect();
    assert_eq!(
        strings,
        vec![("$x$".to_string(), Scope::String)],
        "a star earlier on the line must not shift the formula's columns"
    );
}

#[test]
fn spans_never_overlap_and_are_in_order() {
    // The window walks them in one pass to build the line, so an overlap or
    // an out-of-order span produces garbled text rather than wrong colour.
    let buffer = Buffer::from_text(&paper(30));
    let mut syntax = Syntax::for_path(std::path::Path::new("paper.tex")).unwrap();
    for (line, spans) in syntax.spans(&buffer, 0, 300).iter().enumerate() {
        let width = buffer.line(line).chars().count();
        let mut previous = 0;
        for span in spans {
            assert!(span.start >= previous, "line {line}: {spans:?} overlap");
            assert!(span.start < span.end, "line {line}: empty span {span:?}");
            assert!(span.end <= width, "line {line}: {span:?} past width {width}");
            previous = span.end;
        }
    }
}

#[test]
fn only_the_visible_window_is_coloured() {
    // The reason the query is scoped. Asking for three lines out of a large
    // file must not cost the whole file's worth of spans.
    let buffer = Buffer::from_text(&paper(30));
    let mut syntax = Syntax::for_path(std::path::Path::new("paper.tex")).unwrap();
    let window = syntax.spans(&buffer, 40, 3);
    assert_eq!(window.len(), 3);
    // And the same lines get the same colours whether asked for alone or as
    // part of a larger window — the scoping must not change the answer.
    let wider = syntax.spans(&buffer, 0, 60);
    assert_eq!(window[..], wider[40..43]);
}

#[test]
fn asking_past_the_end_of_the_file_is_not_an_error() {
    let (mut syntax, buffer) = latex("\\section{One}\n");
    assert!(syntax.spans(&buffer, 900, 10).is_empty());
    // Two lines exist: the heading, and the empty one after the newline.
    assert_eq!(syntax.spans(&buffer, 0, 500).len(), 2);
}

#[test]
fn a_file_with_no_grammar_is_plain_text_not_a_failure() {
    assert!(Syntax::for_path(std::path::Path::new("notes.txt")).is_none());
    assert!(Syntax::for_path(std::path::Path::new("Makefile")).is_none());
    assert!(Syntax::for_path(std::path::Path::new("refs.bib")).is_none(), "BibTeX is plain for now");
    for name in ["main.tex", "style.sty", "thesis.cls", "MAIN.TEX"] {
        assert!(Syntax::for_path(std::path::Path::new(name)).is_some(), "{name}");
    }
}

#[test]
fn an_unchanged_buffer_is_not_reparsed() {
    // Highlighting is asked for on every cursor move, and re-parsing a large
    // file each time would make a bare motion cost more than an edit.
    let (mut syntax, mut buffer) = latex("\\section{One} and $x$\n");
    syntax.spans(&buffer, 0, 1);
    let after_first = format!("{syntax:?}");
    syntax.spans(&buffer, 0, 1);
    assert_eq!(format!("{syntax:?}"), after_first, "version should not move");
    buffer.apply(
        vec![quark_text::Edit { start: 0, end: 0, text: "% changed\n".into() }],
        quark_text::Cursor::at(0),
    );
    syntax.spans(&buffer, 0, 1);
    assert_ne!(format!("{syntax:?}"), after_first, "an edit must be picked up");
}

/* ── incremental parsing ──────────────────────────────────────────── */

/// Type `text` at `at`, one character at a time, the way a person would.
fn type_at(buffer: &mut Buffer, at: usize, text: &str) {
    for (offset, c) in (at..).zip(text.chars()) {
        buffer.apply(
            vec![quark_text::Edit { start: offset, end: offset, text: c.to_string() }],
            quark_text::Cursor::at(offset + 1),
        );
    }
}

/// The colours a highlighter that has never seen this buffer would produce.
fn from_scratch(buffer: &Buffer, first: usize, count: usize) -> Vec<Vec<Span>> {
    let mut fresh = Syntax::for_path(std::path::Path::new("x.tex")).unwrap();
    fresh.spans(buffer, first, count)
}

#[test]
fn an_incrementally_updated_tree_colours_the_same_as_a_fresh_one() {
    // The property the whole optimisation rests on. A tree that disagrees
    // with the text does not fail loudly — it quietly colours the rest of the
    // file wrong, which is why this compares against the ground truth after
    // every edit rather than only at the end.
    let mut buffer = Buffer::from_text(&paper(10));
    let mut syntax = Syntax::for_path(std::path::Path::new("x.tex")).unwrap();
    syntax.spans(&buffer, 0, 80);

    // An edit near the top, which invalidates everything below it.
    type_at(&mut buffer, 0, "% a new comment\n");
    assert_eq!(syntax.spans(&buffer, 0, 80), from_scratch(&buffer, 0, 80));

    // One inside a formula, which changes what the dollars after it mean.
    let dollar = buffer.contents().find('$').unwrap();
    type_at(&mut buffer, dollar + 1, "unclosed $ ");
    assert_eq!(syntax.spans(&buffer, 0, 80), from_scratch(&buffer, 0, 80));

    // An environment opened and never closed.
    let begin = buffer.contents().find("\\begin{itemize}").unwrap();
    type_at(&mut buffer, begin, "\\begin{quote}\n");
    assert_eq!(syntax.spans(&buffer, 0, 80), from_scratch(&buffer, 0, 80));

    // And a deletion.
    buffer.apply(
        vec![quark_text::Edit { start: 0, end: 16, text: String::new() }],
        quark_text::Cursor::at(0),
    );
    assert_eq!(syntax.spans(&buffer, 0, 80), from_scratch(&buffer, 0, 80));
}

#[test]
fn undo_is_journalled_like_any_other_change() {
    // Undo moves the text without going through `apply`, so a journal that
    // only watched edits would miss it — and the tree would then describe
    // text that had been taken back.
    let mut buffer = Buffer::from_text("\\section{One} $x$\n");
    let mut syntax = Syntax::for_path(std::path::Path::new("x.tex")).unwrap();
    syntax.spans(&buffer, 0, 2);
    type_at(&mut buffer, 0, "% gone\n");
    syntax.spans(&buffer, 0, 2);
    assert!(buffer.undo(), "there should be something to undo");
    assert_eq!(syntax.spans(&buffer, 0, 2), from_scratch(&buffer, 0, 2));
}

#[test]
fn a_change_older_than_the_journal_falls_back_to_a_full_parse() {
    // The journal is bounded, so it can run out. Running out has to mean
    // "start over", not "carry on with a tree that is missing some edits".
    let mut buffer = Buffer::from_text("\\section{One}\n");
    let mut syntax = Syntax::for_path(std::path::Path::new("x.tex")).unwrap();
    syntax.spans(&buffer, 0, 2);
    type_at(&mut buffer, 0, &"\n".repeat(900));
    assert!(buffer.changes_since(1).is_none(), "a version that far back should no longer be answerable");
    assert_eq!(
        syntax.spans(&buffer, 900, 2),
        from_scratch(&buffer, 900, 2),
        "falling back must produce the right answer, not an approximate one"
    );
}

#[test]
fn an_exhausted_parse_budget_stays_plain_and_can_be_removed() {
    let buffer = Buffer::from_text(&paper(400));
    let mut syntax = Syntax::for_path(std::path::Path::new("budget.tex")).unwrap();
    syntax.limit_parse_time(Some(std::time::Duration::ZERO));
    assert!(syntax.spans(&buffer, 0, 10).is_empty());
    assert!(syntax.is_limited());
    assert!(syntax.spans(&buffer, 100, 10).is_empty());
    syntax.limit_parse_time(None);
    assert!(!syntax.is_limited());
    assert_eq!(syntax.spans(&buffer, 0, 10).len(), 10);
}

#[test]
fn latex_colours_its_structure_its_environments_labels_mathematics_and_comments() {
    let mut syntax = Syntax::for_path(std::path::Path::new("main.tex")).expect("a LaTeX grammar");
    let text = "\\documentclass{article}\n\\section{Search}\n\\begin{itemize}\nSee \\cite{knuth84} and $x^2$.\n\\label{sec:one} % note\n\\end{itemize}\n";
    let buffer = Buffer::from_text(text);
    let lines = syntax.spans(&buffer, 0, 6);
    let all: Vec<(String, Scope)> = (0..6).flat_map(|l| runs(&buffer, &lines[l], l)).collect();
    let has = |text: &str, scope: Scope| all.iter().any(|(t, s)| t == text && *s == scope);
    assert!(has("\\documentclass", Scope::Keyword) && has("{article}", Scope::Type), "{all:?}");
    assert!(has("\\section", Scope::Keyword) && has("{Search}", Scope::Heading), "{all:?}");
    assert!(has("\\begin", Scope::Keyword) && has("{itemize}", Scope::Type), "{all:?}");
    assert!(has("\\cite", Scope::Keyword) && has("{knuth84}", Scope::Attribute), "{all:?}");
    assert!(has("$x^2$", Scope::String), "mathematics: {all:?}");
    assert!(has("% note", Scope::Comment), "{all:?}");
}
