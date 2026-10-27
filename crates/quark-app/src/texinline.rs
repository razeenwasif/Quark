//! The visual editor's text: a paragraph's or a heading's LaTeX as pieces
//! drawn the way a document is — words in their marks, and everything else
//! a locked chip that goes back exactly as it came — and what typing in
//! them does to the LaTeX. From Proton's `ui/tex-visual.js`.
//!
//! The rule it keeps: a block is edited here only when its pieces write it
//! back exactly as it is ([`parse`] says `None` otherwise), so nothing is
//! rewritten that was not typed.

/// The commands drawn as marks.
pub(crate) const MARKS: [&str; 5] = ["textbf", "emph", "textit", "texttt", "underline"];
/// What `\x` escapes.
const ESCAPED: &str = "%&#_${}";
/// Commands a chip names by what they point at.
const POINTING: [&str; 16] = [
    "cite",
    "citep",
    "citet",
    "parencite",
    "textcite",
    "autocite",
    "ref",
    "eqref",
    "cref",
    "Cref",
    "autoref",
    "pageref",
    "label",
    "url",
    "footnote",
    "href",
];
/// The headings, as `blocks` knows them.
const HEADINGS: [&str; 7] = [
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
];
/// A command's chip, past this many characters, says only its name.
const LONG: usize = 24;

/// A mark: the command it is written as, and which `\cmd{…}` it is, so two
/// marks side by side go back as two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    pub cmd: String,
    pub id: u32,
}

/// What a chip holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chip {
    /// A command, drawn as itself.
    Command,
    /// A command drawn as what it points at: `cite k`.
    Pointing,
    /// `$…$` or `\(…\)`, drawn as its source.
    Math,
    /// `%` to the end of its line.
    Comment,
    /// `~`, drawn as the space it is.
    Tie,
    /// `{…}` on its own.
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Body {
    /// Words as they read: `\%` is `%`; a newline is kept, drawn as a space.
    Text(String),
    Chip {
        src: String,
        label: String,
        chip: Chip,
    },
}

/// A run of words or a chip, in its marks, outermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece {
    pub body: Body,
    pub marks: Vec<Mark>,
}

impl Piece {
    fn text(text: &str, marks: &[Mark]) -> Piece {
        Piece {
            body: Body::Text(text.to_string()),
            marks: marks.to_vec(),
        }
    }

    /// How many characters it is, drawn.
    pub(crate) fn len(&self) -> usize {
        match &self.body {
            Body::Text(t) => t.chars().count(),
            Body::Chip { label, .. } => label.chars().count(),
        }
    }

    pub(crate) fn has(&self, cmd: &str) -> bool {
        self.marks.iter().any(|m| m.cmd == cmd)
    }

    /// Whether it is drawn bold, italic, in type, underlined.
    pub(crate) fn bold(&self) -> bool {
        self.has("textbf")
    }

    pub(crate) fn italic(&self) -> bool {
        self.has("emph") || self.has("textit")
    }

    pub(crate) fn mono(&self) -> bool {
        self.has("texttt")
    }

    pub(crate) fn underline(&self) -> bool {
        self.has("underline")
    }
}

/* ── LaTeX → pieces ──────────────────────────────────────────────────── */

struct Parser {
    src: Vec<char>,
    next_id: u32,
    out: Vec<Piece>,
}

impl Parser {
    fn slice(&self, from: usize, to: usize) -> String {
        self.src[from..to.min(self.src.len())].iter().collect()
    }

    fn find(&self, needle: &str, from: usize) -> Option<usize> {
        let needle: Vec<char> = needle.chars().collect();
        (from..self.src.len().saturating_sub(needle.len() - 1))
            .find(|&i| self.src[i..].starts_with(&needle))
    }

    fn chip(&mut self, from: usize, to: usize, label: String, chip: Chip, marks: &[Mark]) {
        let src = self.slice(from, to);
        let label = label.replace('\n', " ");
        self.out.push(Piece {
            body: Body::Chip { src, label, chip },
            marks: marks.to_vec(),
        });
    }

    /// The groups — `[…]` and `{…}` — that follow a command at `i`, and
    /// where they end; `None` when one is not closed.
    fn groups(&self, mut i: usize) -> Option<(Vec<String>, usize)> {
        let mut args = Vec::new();
        while let Some(&open) = self.src.get(i).filter(|c| **c == '[' || **c == '{') {
            let close = if open == '[' { ']' } else { '}' };
            let mut depth = 0;
            let mut j = i;
            loop {
                let c = *self.src.get(j)?;
                if c == '\\' {
                    j += 2;
                    continue;
                }
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            args.push(self.slice(i + 1, j));
            i = j + 1;
        }
        Some((args, i))
    }

    /// From `i` to a closing brace when `in_group`, or to the end: where it
    /// stopped, or `None` when it cannot be drawn faithfully.
    fn inline(&mut self, mut i: usize, in_group: bool, marks: &[Mark]) -> Option<usize> {
        while i < self.src.len() {
            let c = self.src[i];
            match c {
                '}' => return in_group.then_some(i),
                '\\' => {
                    let next = self.src.get(i + 1).copied().unwrap_or(' ');
                    if ESCAPED.contains(next) {
                        self.out.push(Piece::text(&next.to_string(), marks));
                        i += 2;
                    } else if next == '(' {
                        let close = self.find("\\)", i + 2)?;
                        let body = self.slice(i + 2, close);
                        self.chip(i, close + 2, math_label(body.trim()), Chip::Math, marks);
                        i = close + 2;
                    } else if next.is_ascii_alphabetic() {
                        let name: String = self.src[i + 1..]
                            .iter()
                            .take_while(|c| c.is_ascii_alphabetic())
                            .collect();
                        let mut at = i + 1 + name.chars().count();
                        if MARKS.contains(&name.as_str()) && self.src.get(at) == Some(&'{') {
                            let mut inner = marks.to_vec();
                            inner.push(Mark {
                                cmd: name,
                                id: self.next_id,
                            });
                            self.next_id += 1;
                            i = self.inline(at + 1, true, &inner)? + 1;
                            continue;
                        }
                        if self.src.get(at) == Some(&'*') {
                            at += 1;
                        }
                        let (args, end) = self.groups(at)?;
                        let whole = self.slice(i, end);
                        let (label, chip) = match args.last() {
                            Some(last) if POINTING.contains(&name.as_str()) => {
                                (format!("{name} {last}"), Chip::Pointing)
                            }
                            _ if whole.chars().count() > LONG => {
                                (format!("\\{name}…"), Chip::Command)
                            }
                            _ => (whole, Chip::Command),
                        };
                        self.chip(i, end, label, chip, marks);
                        i = end;
                    } else {
                        // `\\`, `\,`, `\'e` and their kind: kept as they are.
                        let end = self
                            .groups(i + 2)
                            .map_or(i + 2, |(_, end)| end)
                            .min(self.src.len());
                        let whole = self.slice(i, end);
                        self.chip(i, end, whole, Chip::Command, marks);
                        i = end;
                    }
                }
                '$' => {
                    let display = self.src.get(i + 1) == Some(&'$');
                    let fence = if display { "$$" } else { "$" };
                    let close = self.find(fence, i + fence.len())?;
                    let body = self.slice(i + fence.len(), close);
                    self.chip(
                        i,
                        close + fence.len(),
                        math_label(body.trim()),
                        Chip::Math,
                        marks,
                    );
                    i = close + fence.len();
                }
                '%' => {
                    let end = self.find("\n", i).map_or(self.src.len(), |n| n + 1);
                    self.chip(i, end, "%".into(), Chip::Comment, marks);
                    i = end;
                }
                '~' => {
                    self.chip(i, i + 1, "\u{a0}".into(), Chip::Tie, marks);
                    i += 1;
                }
                '{' => {
                    let (_, end) = self.groups(i)?;
                    let whole = self.slice(i, end);
                    self.chip(i, end, whole, Chip::Group, marks);
                    i = end;
                }
                _ => {
                    let mut j = i;
                    while j < self.src.len() && !"\\$%~{}".contains(self.src[j]) {
                        j += 1;
                    }
                    let text = self.slice(i, j);
                    self.out.push(Piece::text(&text, marks));
                    i = j;
                }
            }
        }
        (!in_group).then_some(i)
    }
}

/// Symbols as they are typeset, for inline mathematics drawn as words —
/// those the fonts have.
pub(crate) const SYMBOLS: [(&str, &str); 54] = [
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("phi", "φ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("sim", "~"),
    ("approx", "≈"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("times", "×"),
    ("cdot", "·"),
    ("pm", "±"),
    ("infty", "∞"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("partial", "∂"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("ldots", "…"),
];

/// Inline mathematics as a label: the commands for symbols drawn as them,
/// the rest as written.
fn math_label(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let name: String = after
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        match SYMBOLS.iter().find(|(n, _)| *n == name) {
            Some((_, symbol)) => {
                out.push_str(symbol);
                rest = &after[name.len()..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A paragraph's LaTeX as pieces, or `None` when they would not write it
/// back exactly.
pub(crate) fn parse(src: &str) -> Option<Vec<Piece>> {
    let mut parser = Parser {
        src: src.chars().collect(),
        next_id: 1,
        out: Vec::new(),
    };
    parser.inline(0, false, &[])?;
    (to_latex(&parser.out) == src).then_some(parser.out)
}

/// A heading's source in three: the command up to its `{`, its title as
/// pieces, and the rest from its `}` — or `None`.
pub(crate) fn heading(src: &str) -> Option<(String, Vec<Piece>, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = chars.iter().take_while(|c| c.is_whitespace()).count();
    if chars.get(i) != Some(&'\\') {
        return None;
    }
    i += 1;
    let name: String = chars[i..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if !HEADINGS.contains(&name.as_str()) {
        return None;
    }
    i += name.len();
    if chars.get(i) == Some(&'*') {
        i += 1;
    }
    if chars.get(i) == Some(&'[') {
        i += chars[i..].iter().position(|c| *c == ']')? + 1;
    }
    if chars.get(i) != Some(&'{') {
        return None;
    }
    let head: String = chars[..=i].iter().collect();
    let mut parser = Parser {
        src: chars,
        next_id: 1,
        out: Vec::new(),
    };
    let end = parser.inline(i + 1, true, &[])?;
    let tail: String = parser.src[end..].iter().collect();
    let title = parser.out;
    (format!("{head}{}{tail}", to_latex(&title)) == src).then_some((head, title, tail))
}

/* ── pieces → LaTeX ──────────────────────────────────────────────────── */

/// Words as LaTeX writes them.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            c if ESCAPED.contains(c) => {
                out.push('\\');
                out.push(c);
            }
            '\\' => out.push_str("\\textbackslash{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            c => out.push(c),
        }
    }
}

/// The pieces written back, and how far into it, in characters, the place
/// `caret` characters into what is drawn is.
fn write(pieces: &[Piece], caret: Option<usize>) -> (String, Option<usize>) {
    let mut out = String::new();
    let mut open: Vec<&Mark> = Vec::new();
    let mut at = 0;
    let mut found = None;
    let here = |out: &String| Some(out.chars().count());
    for piece in pieces {
        let common = open
            .iter()
            .zip(&piece.marks)
            .take_while(|(a, b)| **a == *b)
            .count();
        while open.len() > common {
            open.pop();
            out.push('}');
        }
        if found.is_none() && caret == Some(at) {
            found = here(&out);
        }
        for mark in &piece.marks[common..] {
            out.push('\\');
            out.push_str(&mark.cmd);
            out.push('{');
            open.push(mark);
        }
        let len = piece.len();
        match &piece.body {
            Body::Text(text) => {
                if let Some(c) = caret.filter(|c| found.is_none() && at < *c && *c < at + len) {
                    let before: String = text.chars().take(c - at).collect();
                    escape(&before, &mut out);
                    found = here(&out);
                    let after: String = text.chars().skip(c - at).collect();
                    escape(&after, &mut out);
                } else {
                    escape(text, &mut out);
                }
            }
            Body::Chip { src, .. } => {
                if caret.is_some_and(|c| found.is_none() && at < c && c < at + len) {
                    found = here(&out);
                }
                out.push_str(src);
            }
        }
        at += len;
    }
    for _ in open {
        out.push('}');
    }
    if found.is_none() && caret.is_some() {
        found = here(&out);
    }
    (out, found)
}

/// The pieces as LaTeX.
pub(crate) fn to_latex(pieces: &[Piece]) -> String {
    write(pieces, None).0
}

/// Where in the LaTeX, in characters, the place `caret` characters into
/// what is drawn is.
pub(crate) fn source_at(pieces: &[Piece], caret: usize) -> usize {
    write(pieces, Some(caret)).1.unwrap_or(0)
}

/// What is drawn: the words, newlines as the spaces TeX makes of them, and
/// each chip's label — one character for one, so a place in it is a place
/// in the pieces.
pub(crate) fn display(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match &piece.body {
            Body::Text(t) => out.extend(t.chars().map(|c| if c == '\n' { ' ' } else { c })),
            Body::Chip { label, .. } => out.push_str(label),
        }
    }
    out
}

/* ── typing ──────────────────────────────────────────────────────────── */

/// What a change to what is drawn did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typed {
    Same,
    Changed(Vec<Piece>),
    /// Typed inside a chip, which is not typed in.
    Refused,
}

/// The pieces after what is drawn became `new`, with the caret `caret`
/// characters into it. What changed is what lies between the parts that
/// did not; where that is ambiguous — a letter typed by the same letter —
/// the caret says, as it is just after what was typed. A chip any of whose
/// label went goes whole; words typed by words take their marks.
pub(crate) fn typed(pieces: &[Piece], new: &str, caret: usize) -> Typed {
    let old: Vec<char> = display(pieces).chars().collect();
    let new: Vec<char> = new.chars().collect();
    if old == new {
        return Typed::Same;
    }
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let kept = (prefix + suffix).min(old.len()).min(new.len());
    let lowest = kept.saturating_sub(suffix);
    let highest = prefix.min(kept);
    let p = (caret + kept)
        .saturating_sub(new.len())
        .clamp(lowest, highest);
    let s = kept - p;
    let (cut_from, cut_to) = (p, old.len() - s);
    let put: String = new[p..new.len() - s].iter().collect();

    // What went, and where what was typed goes.
    let mut out: Vec<Piece> = Vec::new();
    let mut at = 0;
    let mut put_at = cut_from;
    for piece in pieces {
        let (a, b) = (at, at + piece.len());
        at = b;
        match &piece.body {
            Body::Chip { .. } => {
                if cut_from < cut_to && a < cut_to && b > cut_from {
                    put_at = put_at.min(a);
                    continue;
                }
                if a < cut_from && cut_from < b {
                    return Typed::Refused;
                }
                out.push(piece.clone());
            }
            Body::Text(text) => {
                let kept: String = text
                    .chars()
                    .enumerate()
                    .filter(|(i, _)| a + i < cut_from || a + i >= cut_to)
                    .map(|(_, c)| c)
                    .collect();
                if !kept.is_empty() {
                    out.push(Piece {
                        body: Body::Text(kept),
                        marks: piece.marks.clone(),
                    });
                }
            }
        }
    }
    if !put.is_empty() {
        insert(&mut out, put_at, &put);
    }
    Typed::Changed(out)
}

/// Words put in at `at`: into the words there, or after those before — a
/// word typed after a bold one is bold — or as words of their own in the
/// marks of what is beside them.
fn insert(pieces: &mut Vec<Piece>, at: usize, put: &str) {
    let mut start = 0;
    for i in 0..pieces.len() {
        let len = pieces[i].len();
        if start < at && at < start + len {
            if let Body::Text(text) = &mut pieces[i].body {
                let k = text
                    .char_indices()
                    .nth(at - start)
                    .map_or(text.len(), |(k, _)| k);
                text.insert_str(k, put);
            }
            return;
        }
        if at == start + len {
            if let Body::Text(text) = &mut pieces[i].body {
                text.push_str(put);
                return;
            }
            if let Some(Body::Text(text)) = pieces.get_mut(i + 1).map(|p| &mut p.body) {
                text.insert_str(0, put);
                return;
            }
            let marks = pieces[i].marks.clone();
            pieces.insert(
                i + 1,
                Piece {
                    body: Body::Text(put.into()),
                    marks,
                },
            );
            return;
        }
        start += len;
    }
    match pieces.first_mut().map(|p| &mut p.body) {
        Some(Body::Text(text)) => text.insert_str(0, put),
        _ => {
            let marks = pieces.first().map(|p| p.marks.clone()).unwrap_or_default();
            pieces.insert(
                0,
                Piece {
                    body: Body::Text(put.into()),
                    marks,
                },
            );
        }
    }
}

/// The pieces cut so one begins at `at`.
fn split_at(pieces: &mut Vec<Piece>, at: usize) {
    let mut start = 0;
    for i in 0..pieces.len() {
        let len = pieces[i].len();
        if start < at && at < start + len {
            if let Body::Text(text) = &pieces[i].body {
                let after: String = text.chars().skip(at - start).collect();
                let before: String = text.chars().take(at - start).collect();
                let marks = pieces[i].marks.clone();
                pieces[i].body = Body::Text(before);
                pieces.insert(
                    i + 1,
                    Piece {
                        body: Body::Text(after),
                        marks,
                    },
                );
            }
            return;
        }
        start += len;
    }
}

/// ⌘B or ⌘I over what is drawn from `from` to `to`: the mark taken off
/// when all of it has it, else put on all of it, as one.
pub(crate) fn toggle(pieces: &[Piece], from: usize, to: usize, cmd: &str) -> Vec<Piece> {
    let mut out = pieces.to_vec();
    if from >= to {
        return out;
    }
    split_at(&mut out, from);
    split_at(&mut out, to);
    let fresh = out
        .iter()
        .flat_map(|p| p.marks.iter().map(|m| m.id))
        .max()
        .unwrap_or(0)
        + 1;
    let mut at = 0;
    let inside: Vec<bool> = out
        .iter()
        .map(|p| {
            let (a, b) = (at, at + p.len());
            at = b;
            b > from && a < to && p.len() > 0
        })
        .collect();
    let all = out
        .iter()
        .zip(&inside)
        .filter(|(_, i)| **i)
        .all(|(p, _)| p.has(cmd));
    for (piece, _) in out.iter_mut().zip(&inside).filter(|(_, i)| **i) {
        piece.marks.retain(|m| m.cmd != cmd);
        if !all {
            piece.marks.push(Mark {
                cmd: cmd.into(),
                id: fresh,
            });
        }
    }
    out
}

/* ── the other blocks ────────────────────────────────────────────────── */

/// The source of a `{…}` at `from` in `text` (bytes), without its braces.
fn group_at(text: &str, from: usize) -> Option<&str> {
    let inner = text.get(from..)?.strip_prefix('{')?;
    let mut depth = 1;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&inner[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// An equation as TeX typesets it on a page of its own: an `equation` or a
/// fenced one as displayed mathematics, an `align` and its kind starred —
/// unnumbered, as the page shows only it — and its labels left out.
pub(crate) fn display_math(src: &str) -> String {
    let body = src.trim();
    let shown = if let Some(rest) = body.strip_prefix("\\begin{") {
        let (env, inner) = rest.split_once('}').unwrap_or((rest, ""));
        let close = format!("\\end{{{env}}}");
        let inner = inner.strip_suffix(close.as_str()).unwrap_or(inner);
        if ["equation", "equation*", "displaymath"].contains(&env) {
            format!("\\[{inner}\\]")
        } else {
            let starred = if env.ends_with('*') {
                env.to_string()
            } else {
                format!("{env}*")
            };
            // `alignat{2}` keeps its count.
            format!("\\begin{{{starred}}}{inner}\\end{{{starred}}}")
        }
    } else if let Some(inner) = body.strip_prefix("$$").and_then(|b| b.strip_suffix("$$")) {
        format!("\\[{inner}\\]")
    } else {
        body.to_string()
    };
    let mut out = String::with_capacity(shown.len());
    let mut rest = shown.as_str();
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        if after.starts_with("\\label{") {
            match group_at(after, "\\label".len()) {
                Some(label) => rest = &after["\\label".len() + label.len() + 2..],
                None => {
                    out.push_str(after);
                    rest = "";
                }
            }
            continue;
        }
        let word = |w: &str| {
            after
                .strip_prefix(w)
                .filter(|r| !r.starts_with(|c: char| c.is_ascii_alphabetic()))
        };
        if let Some(r) = word("\\nonumber").or_else(|| word("\\notag")) {
            rest = r;
            continue;
        }
        out.push('\\');
        rest = &after[1..];
    }
    out.push_str(rest);
    // A line left blank is a paragraph, which mathematics cannot hold.
    let kept: Vec<&str> = out.split('\n').filter(|l| !l.trim().is_empty()).collect();
    kept.join("\n")
}

/// A list's items, and whether they are numbered.
pub(crate) fn list_items(src: &str) -> (bool, Vec<String>) {
    let numbered = src.trim_start().starts_with("\\begin{enumerate}");
    let mut items = Vec::new();
    let mut rest = src;
    let item_at = |s: &str| {
        let mut from = 0;
        while let Some(at) = s[from..].find("\\item") {
            let end = from + at + "\\item".len();
            if !s[end..].starts_with(|c: char| c.is_ascii_alphabetic()) {
                return Some((from + at, end));
            }
            from = end;
        }
        None
    };
    let Some((_, first)) = item_at(rest) else {
        return (numbered, items);
    };
    rest = &rest[first..];
    loop {
        let (item, next) = match item_at(rest) {
            Some((at, end)) => (&rest[..at], Some(end)),
            None => (rest, None),
        };
        let mut item = item.to_string();
        for env in ["itemize", "enumerate", "description"] {
            item = item
                .replace(&format!("\\begin{{{env}}}"), "")
                .replace(&format!("\\end{{{env}}}"), "");
        }
        items.push(item.trim().to_string());
        match next {
            Some(end) => rest = &rest[end..],
            None => break,
        }
    }
    (numbered, items)
}

/// A figure's caption, as its source says it.
pub(crate) fn caption(src: &str) -> Option<String> {
    let at = src.find("\\caption")?;
    let mut from = at + "\\caption".len();
    // `\caption[short]{long}`: the long one.
    if src[from..].starts_with('[') {
        from += src[from..].find(']')? + 1;
    }
    group_at(src, from).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pieces typed into as an edit box types: `new` drawn, caret after.
    fn type_in(src: &str, new: &str, caret: usize) -> String {
        let pieces = parse(src).expect("faithful");
        match typed(&pieces, new, caret) {
            Typed::Changed(p) => to_latex(&p),
            Typed::Same => src.to_string(),
            Typed::Refused => "refused".into(),
        }
    }

    #[test]
    fn a_paragraph_comes_back_exactly_and_draws_as_it_reads() {
        for src in [
            "Plain words over\ntwo lines.",
            "Some \\emph{text}, \\textbf{bold \\emph{and both}} and \\texttt{code}.",
            "Citing~\\cite{knuth84} and \\ref{sec:intro}, see \\url{https://x.org}.",
            "Inline $x^2 + y$ and \\(a_b\\) and $$c$$.",
            "Escapes: 100\\% \\& \\$5 \\# \\_ \\{ \\}.",
            "A comment % to the end\nof a line.",
            "Spaces\\, and \\\\ breaks, \\'e, {\\bf grouped}.",
            "\\textbf{a}\\textbf{b} stay two.",
            "A \\footnote{with \\emph{marks} inside} note.",
            "",
        ] {
            let pieces = parse(src).unwrap_or_else(|| panic!("{src:?} is drawn"));
            assert_eq!(to_latex(&pieces), src);
            assert_eq!(
                display(&pieces).chars().count(),
                pieces.iter().map(Piece::len).sum::<usize>()
            );
        }
        let pieces = parse("Some \\emph{text} citing~\\cite{k}, $x^2$ 100\\%.").unwrap();
        assert_eq!(display(&pieces), "Some text citing\u{a0}cite k, x^2 100%.");
        assert_eq!(
            display(&parse("$x_i \\sim q$ and \\(\\alpha\\neq\\hat{b}\\)").unwrap()),
            "x_i ~ q and α≠\\hat{b}",
            "symbols as they are typeset"
        );
        assert!(pieces[1].italic() && !pieces[0].italic());
        assert_eq!(
            display(&parse("over\ntwo").unwrap()),
            "over two",
            "a newline is drawn as the space it is"
        );
    }

    #[test]
    fn what_cannot_come_back_exactly_is_not_drawn_to_be_edited() {
        for src in [
            "\\textbf{}",
            "a^b",
            "unclosed {group",
            "a } too many",
            "$open",
            "\\cite{open",
            "x \\(y",
        ] {
            assert_eq!(parse(src), None, "{src:?}");
        }
    }

    #[test]
    fn typing_changes_only_what_was_typed_and_keeps_its_marks() {
        // In a bold word, it stays bold.
        assert_eq!(
            type_in("a \\textbf{bold} b", "a boXld b", 5),
            "a \\textbf{boXld} b"
        );
        // After a bold word, it goes on being bold, as a word processor does.
        assert_eq!(type_in("\\textbf{x} y", "xZ y", 2), "\\textbf{xZ} y");
        // A deletion.
        assert_eq!(type_in("one two", "one wo", 4), "one wo");
        // A sign that LaTeX escapes is written escaped.
        assert_eq!(type_in("cost", "cost 5%", 7), "cost 5\\%");
        // A newline in the source stays one when the words round it change.
        assert_eq!(type_in("one\ntwo", "one twoX", 8), "one\ntwoX");
        // The same letter typed beside a chip whose label ends in it goes where the caret says.
        assert_eq!(type_in("\\cite{k}k", "cite kXk", 7), "\\cite{k}Xk");
        assert_eq!(
            type_in("a~b", "a\u{a0}\u{a0}b", 3),
            "a~\u{a0}b",
            "after the tie, not in it"
        );
    }

    #[test]
    fn a_chip_goes_whole_or_not_at_all() {
        // Backspace at its end takes all of it.
        assert_eq!(
            type_in("see \\cite{knuth} now", "see cite knut now", 13),
            "see  now"
        );
        // Typed inside it: refused.
        assert_eq!(
            type_in("see \\cite{knuth} now", "see citXe knuth now", 8),
            "refused"
        );
        // Words typed over a selection that takes a chip and words.
        assert_eq!(type_in("a $x$ b", "a Y", 3), "a Y");
    }

    #[test]
    fn bold_and_italic_go_on_and_come_off_a_selection() {
        let pieces = parse("one two three").unwrap();
        let bold = toggle(&pieces, 4, 7, "textbf");
        assert_eq!(to_latex(&bold), "one \\textbf{two} three");
        assert_eq!(to_latex(&toggle(&bold, 4, 7, "textbf")), "one two three");
        // Off the middle of a bold run: two runs either side.
        let whole = parse("\\textbf{one two three}").unwrap();
        assert_eq!(
            to_latex(&toggle(&whole, 4, 7, "textbf")),
            "\\textbf{one }two\\textbf{ three}"
        );
        // Over a run some of which is bold: all of it, as one.
        let some = parse("a \\textbf{b} c").unwrap();
        assert_eq!(to_latex(&toggle(&some, 0, 5, "textbf")), "\\textbf{a b c}");
        // Italic inside bold.
        assert_eq!(
            to_latex(&toggle(&bold, 5, 6, "emph")),
            "one \\textbf{t\\emph{w}o} three"
        );
    }

    #[test]
    fn a_place_in_what_is_drawn_is_a_place_in_the_source() {
        let pieces = parse("Hi \\textbf{bold} 5\\% \\cite{k} end").unwrap();
        let src = to_latex(&pieces);
        let at = |caret: usize| {
            src.chars()
                .take(source_at(&pieces, caret))
                .collect::<String>()
        };
        assert_eq!(at(0), "");
        assert_eq!(at(5), "Hi \\textbf{bo");
        assert_eq!(at(10), "Hi \\textbf{bold} 5\\%");
        assert_eq!(at(display(&pieces).chars().count()), src);
    }

    #[test]
    fn a_heading_s_title_is_edited_and_its_command_and_label_kept() {
        let src = "\\section*[Short]{The \\emph{Title}}\n\\label{sec:t}\n";
        let (head, title, tail) = heading(src).expect("a heading");
        assert_eq!(head, "\\section*[Short]{");
        assert_eq!(display(&title), "The Title");
        assert_eq!(tail, "}\n\\label{sec:t}\n");
        let Typed::Changed(title) = typed(&title, "The New Title", 8) else {
            panic!()
        };
        assert_eq!(
            format!("{head}{}{tail}", to_latex(&title)),
            "\\section*[Short]{The New \\emph{Title}}\n\\label{sec:t}\n"
        );
        assert!(heading("\\paragraphs{x}").is_none() && heading("\\section x").is_none());
        assert!(heading("\\subsection{a}").is_some());
    }

    #[test]
    fn an_equation_is_typeset_unnumbered_without_its_label() {
        assert_eq!(
            display_math("\\begin{equation}\n  x \\label{eq:x} = 1\n\\end{equation}\n"),
            "\\[\n  x  = 1\n\\]"
        );
        assert_eq!(
            display_math("\\begin{align}\na &= b \\nonumber\\\\\nc &= d\n\\end{align}"),
            "\\begin{align*}\na &= b \\\\\nc &= d\n\\end{align*}"
        );
        assert_eq!(display_math("$$ y $$"), "\\[ y \\]");
        assert_eq!(
            display_math("\\begin{equation}\n  y = 2\n  \\label{eq:y}\n\\end{equation}"),
            "\\[\n  y = 2\n\\]",
            "the label's line goes with it"
        );
        assert_eq!(display_math("\\[ z \\notag \\]"), "\\[ z  \\]");
        assert_eq!(
            display_math("\\[ \\notation \\]"),
            "\\[ \\notation \\]",
            "only the word itself"
        );
    }

    #[test]
    fn a_list_s_items_and_a_figure_s_caption_are_read() {
        let (numbered, items) = list_items(
            "\\begin{enumerate}\n  \\item one\n  \\item two \\emph{x}\n  \\begin{itemize}\n    \\item nested\n  \\end{itemize}\n\\end{enumerate}\n",
        );
        assert!(numbered);
        assert_eq!(items, ["one", "two \\emph{x}", "nested"]);
        assert_eq!(
            list_items("\\begin{itemize}\\itemsep0pt\\item a\\end{itemize}").1,
            ["a"],
            "\\itemsep is not an item"
        );
        assert_eq!(
            caption("\\begin{figure}\n\\caption[S]{A \\emph{plot} {of} x.}\n\\end{figure}")
                .as_deref(),
            Some("A \\emph{plot} {of} x.")
        );
        assert_eq!(caption("no caption"), None);
    }

    #[test]
    fn each_block_edited_in_place_leaves_every_other_block_as_it_was() {
        let doc = "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\n\\label{s}\nSome \\emph{text}\nover two lines~\\cite{k}.\n\n\\begin{equation}\n  x = 1\n\\end{equation}\nA second paragraph, 5\\% of it.\n\\subsection*{More \\textbf{bold}}\n\\begin{itemize}\n  \\item one\n\\end{itemize}\nLast words.\n\\end{document}\n";
        let before = quark_tex::blocks::blocks(doc);
        let editable: Vec<usize> = before
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == "paragraph" || b.kind == "heading")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(editable.len(), 5);
        for &i in &editable {
            let block = &before[i];
            // An X typed at the end of what is drawn.
            let latex = if block.kind == "heading" {
                let (head, title, tail) = heading(&block.text).expect("drawn");
                let shown = format!("{}X", display(&title));
                let Typed::Changed(title) = typed(&title, &shown, shown.chars().count()) else {
                    panic!()
                };
                format!("{head}{}{tail}", to_latex(&title))
            } else {
                let pieces = parse(&block.text).expect("drawn");
                let shown = display(&pieces);
                let trimmed = shown.trim_end();
                let shown = format!("{trimmed}X{}", &shown[trimmed.len()..]);
                let caret = trimmed.chars().count() + 1;
                let Typed::Changed(pieces) = typed(&pieces, &shown, caret) else {
                    panic!()
                };
                to_latex(&pieces)
            };
            let chars: Vec<char> = doc.chars().collect();
            let edited: String = chars[..block.start as usize]
                .iter()
                .chain(latex.chars().collect::<Vec<_>>().iter())
                .chain(chars[block.end as usize..].iter())
                .collect();
            let after = quark_tex::blocks::blocks(&edited);
            assert_eq!(after.len(), before.len(), "{edited}");
            for (j, (a, b)) in before.iter().zip(&after).enumerate() {
                assert_eq!(a.kind, b.kind);
                if j == i {
                    assert_eq!(b.text, latex);
                    assert_eq!(b.text.matches('X').count(), 1);
                } else {
                    assert_eq!(a.text, b.text, "block {j} untouched by an edit to {i}");
                }
            }
        }
    }

    #[test]
    fn every_symbol_is_in_the_fonts() {
        let ctx = egui::Context::default();
        quark_ui::theme::install_fonts(&ctx);
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        let missing: Vec<&str> = SYMBOLS
            .iter()
            .filter(|(_, s)| {
                !ctx.fonts_mut(|f| {
                    s.chars()
                        .all(|c| f.has_glyph(&egui::FontId::proportional(15.0), c))
                })
            })
            .map(|(n, _)| *n)
            .collect();
        assert!(missing.is_empty(), "no glyph for {missing:?}");
    }
}
