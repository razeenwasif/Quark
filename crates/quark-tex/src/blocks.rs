//! A `.tex` file cut into the blocks the visual editor draws: the preamble,
//! headings, paragraphs, displayed mathematics, lists, figures, tables,
//! and everything else as it is — each with where it is in the source.
//!
//! The one rule: the blocks' texts, joined, are the source exactly. Blank
//! lines between blocks are blocks too (`space`), so an edit to one block
//! is an edit to exactly its span and nothing round it.

use serde::{Deserialize, Serialize};

/// One block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// `preamble`, `heading`, `paragraph`, `math`, `list`, `figure`,
    /// `table`, `raw`, `space` or `end` (`\end{document}` and after).
    pub kind: String,
    /// Where it is, in characters — as the editor counts — from and to.
    pub start: u32,
    pub end: u32,
    /// Its source, line endings and all.
    pub text: String,
    /// A heading's depth: 0 a part, 1 a chapter, 2 a section…
    pub level: Option<u8>,
    /// A figure's image, as `\includegraphics` names it.
    pub image: Option<String>,
}

const HEADINGS: &[(&str, u8)] = &[
    ("part", 0),
    ("chapter", 1),
    ("section", 2),
    ("subsection", 3),
    ("subsubsection", 4),
    ("paragraph", 5),
    ("subparagraph", 6),
];
const MATH: &[&str] = &[
    "equation",
    "align",
    "gather",
    "multline",
    "displaymath",
    "eqnarray",
    "flalign",
    "alignat",
];
const LISTS: &[&str] = &["itemize", "enumerate", "description"];
/// Commands that stand on a line of their own and are not prose.
const STANDING: &[&str] = &[
    "maketitle",
    "tableofcontents",
    "listoffigures",
    "listoftables",
    "bibliography",
    "bibliographystyle",
    "printbibliography",
    "label",
    "input",
    "include",
    "newpage",
    "clearpage",
    "cleardoublepage",
    "appendix",
    "frontmatter",
    "mainmatter",
    "backmatter",
    "centering",
    "noindent",
    "vspace",
    "hspace",
    "bigskip",
    "medskip",
    "smallskip",
    "pagebreak",
    "linebreak",
    "newcommand",
    "renewcommand",
    "setlength",
    "addcontentsline",
    "includeonly",
    "pagestyle",
    "thispagestyle",
];

/// A heading line's command and depth: `\section*{…}` is a section.
fn heading(line: &str) -> Option<u8> {
    let rest = line.trim_start().strip_prefix('\\')?;
    HEADINGS.iter().find_map(|(name, level)| {
        let after = rest.strip_prefix(name)?;
        let after = after.strip_prefix('*').unwrap_or(after);
        after.trim_start().starts_with(['{', '[']).then_some(*level)
    })
}

/// The environment a line begins, if it begins with one.
fn begins(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("\\begin{")?;
    rest.split_once('}').map(|(env, _)| env.to_string())
}

/// Whether a line is only a comment.
fn comment(line: &str) -> bool {
    line.trim_start().starts_with('%')
}

/// Commands that lead into a paragraph rather than standing apart from
/// one: `\noindent\textbf{Keywords:} …` is a paragraph that begins without
/// an indent, and `\smallskip` on the line before it is space above it.
const LEAD_IN: &[&str] = &["noindent", "indent", "smallskip", "medskip", "bigskip", "vspace", "par"];

/// Whether a line begins with lead-in commands only — `Some(true)` when text
/// follows them on the line, `Some(false)` when they are all it holds.
fn leads(line: &str) -> Option<bool> {
    let mut rest = line.trim_start();
    let mut any = false;
    while let Some(after) = rest.strip_prefix('\\') {
        let name: String = after.chars().take_while(char::is_ascii_alphabetic).collect();
        if !LEAD_IN.contains(&name.as_str()) {
            break;
        }
        let mut tail = after[name.len()..].strip_prefix('*').unwrap_or(&after[name.len()..]);
        // `\vspace{1em}` takes its length with it.
        if name == "vspace" {
            let t = tail.trim_start();
            let inner = t.strip_prefix('{')?;
            tail = &inner[inner.find('}')? + 1..];
        }
        any = true;
        rest = tail.trim_start();
    }
    any.then(|| !rest.trim().is_empty() && !rest.trim_start().starts_with('%'))
}

/// Where a paragraph that begins with lead-in commands at line `i` ends:
/// lead-in lines first, then a line of text, then text up to a break. None
/// when the lead-ins lead into no text — before a heading, say — and so
/// stand alone after all.
fn led_paragraph(lines: &[&str], i: usize, end: usize) -> Option<usize> {
    leads(lines[i])?;
    let mut j = i;
    loop {
        if j >= end {
            return None;
        }
        match leads(lines[j]) {
            Some(false) => j += 1,
            Some(true) => {
                j += 1;
                break;
            }
            None if lines[j].trim().is_empty() || breaks(lines[j]) => return None,
            None => {
                j += 1;
                break;
            }
        }
    }
    while j < end && !breaks(lines[j]) {
        j += 1;
    }
    Some(j)
}

/// Whether a line is a command that stands alone.
fn standing(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix('\\') else {
        return false;
    };
    let name: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
    STANDING.contains(&name.as_str())
}

/// Whether a line starts a block of its own rather than going on with a
/// paragraph.
fn breaks(line: &str) -> bool {
    let t = line.trim_start();
    t.is_empty()
        || heading(line).is_some()
        || begins(line).is_some()
        || t.starts_with("\\[")
        || t.starts_with("$$")
        || comment(line)
        || standing(line)
        || t.starts_with("\\end{")
}

/// What kind of block an environment is.
fn kind_of(env: &str) -> &'static str {
    let bare = env.trim_end_matches('*');
    if MATH.contains(&bare) {
        "math"
    } else if LISTS.contains(&bare) {
        "list"
    } else if bare == "figure" || bare == "wrapfigure" {
        "figure"
    } else if bare == "abstract" {
        "abstract"
    } else if bare == "table" || bare == "tabular" || bare == "longtable" {
        "table"
    } else {
        "raw"
    }
}

/// The image a figure's source includes.
fn image_of(text: &str) -> Option<String> {
    let at = text.find("\\includegraphics")?;
    let rest = &text[at + "\\includegraphics".len()..];
    let rest = rest.trim_start();
    let rest = match rest.strip_prefix('[') {
        Some(options) => options.split_once(']')?.1,
        None => rest,
    };
    let inner = rest.trim_start().strip_prefix('{')?;
    inner
        .split_once('}')
        .map(|(path, _)| path.trim().to_string())
}

/// Cut `source` into blocks.
pub fn blocks(source: &str) -> Vec<Block> {
    // Lines with their endings, and where each starts, in characters.
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut at = 0u32;
    for line in &lines {
        starts.push(at);
        at += line.chars().count() as u32;
    }
    starts.push(at);
    let mut out: Vec<Block> = Vec::new();
    let push = |out: &mut Vec<Block>, kind: &str, from: usize, to: usize, level: Option<u8>| {
        if from >= to {
            return;
        }
        let text: String = lines[from..to].concat();
        let image = if kind == "figure" {
            image_of(&text)
        } else {
            None
        };
        out.push(Block {
            kind: kind.into(),
            start: starts[from],
            end: starts[to],
            text,
            level,
            image,
        });
    };

    let body_from = lines
        .iter()
        .position(|l| l.trim_start().starts_with("\\begin{document}"))
        .map(|i| i + 1);
    let mut i = 0;
    if let Some(from) = body_from {
        push(&mut out, "preamble", 0, from, None);
        i = from;
    }
    let end_at = lines
        .iter()
        .rposition(|l| l.trim_start().starts_with("\\end{document}"))
        .filter(|&e| e >= i)
        .unwrap_or(lines.len());

    while i < end_at {
        let line = lines[i];
        let t = line.trim_start();
        let from = i;
        if t.trim().is_empty() {
            while i < end_at && lines[i].trim().is_empty() {
                i += 1;
            }
            push(&mut out, "space", from, i, None);
        } else if let Some(level) = heading(line) {
            i += 1;
            // A label on the line after names it: the heading's.
            if i < end_at && lines[i].trim_start().starts_with("\\label") {
                i += 1;
            }
            push(&mut out, "heading", from, i, Some(level));
        } else if let Some(env) = begins(line) {
            let open = format!("\\begin{{{env}}}");
            let close = format!("\\end{{{env}}}");
            let mut depth = 0i32;
            let mut j = i;
            while j < end_at {
                depth += lines[j].matches(&open).count() as i32;
                depth -= lines[j].matches(&close).count() as i32;
                j += 1;
                if depth <= 0 {
                    break;
                }
            }
            i = j;
            push(&mut out, kind_of(&env), from, i, None);
        } else if t.starts_with("\\[") || t.starts_with("$$") {
            let fence = if t.starts_with("\\[") { "\\]" } else { "$$" };
            // Closed on its own line, or on the first line after that closes it.
            let closed_here = t[2..].contains(fence);
            i += 1;
            if !closed_here {
                while i < end_at {
                    let closes = lines[i].contains(fence);
                    i += 1;
                    if closes {
                        break;
                    }
                }
            }
            push(&mut out, "math", from, i, None);
        } else if let Some(end) = led_paragraph(&lines, i, end_at) {
            i = end;
            push(&mut out, "paragraph", from, i, None);
        } else if comment(line) || standing(line) || t.starts_with("\\end{") {
            i += 1;
            while i < end_at && (comment(lines[i]) || standing(lines[i])) {
                i += 1;
            }
            push(&mut out, "raw", from, i, None);
        } else {
            i += 1;
            while i < end_at && !breaks(lines[i]) {
                i += 1;
            }
            push(&mut out, "paragraph", from, i, None);
        }
    }
    push(&mut out, "end", end_at, lines.len(), None);
    out
}

/// The macros a preamble defines, for KaTeX: `\newcommand{\R}{\mathbb{R}}`,
/// `\newcommand\abs[1]{|#1|}`, `\DeclareMathOperator{\tr}{tr}`, `\def\x{y}`.
pub fn macros(preamble: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let group = |text: &str| -> Option<(String, usize)> {
        let t = text.trim_start();
        let skipped = text.len() - t.len();
        let inner = t.strip_prefix('{')?;
        let mut depth = 1;
        for (i, c) in inner.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((inner[..i].to_string(), skipped + 1 + i + 1));
                    }
                }
                _ => {}
            }
        }
        None
    };
    for command in [
        "\\newcommand",
        "\\renewcommand",
        "\\providecommand",
        "\\DeclareMathOperator",
        "\\def",
    ] {
        let mut rest = preamble;
        while let Some(at) = rest.find(command) {
            let mut after = &rest[at + command.len()..];
            rest = after;
            let star = after.starts_with('*');
            if star {
                after = &after[1..];
            }
            if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            // The name: `{\R}` or `\R`.
            let (name, used) = match group(after) {
                Some((name, used)) => (name.trim().to_string(), used),
                None => {
                    let t = after.trim_start();
                    let Some(bare) = t.strip_prefix('\\') else {
                        continue;
                    };
                    let name: String = bare.chars().take_while(char::is_ascii_alphabetic).collect();
                    (format!("\\{name}"), after.len() - t.len() + 1 + name.len())
                }
            };
            let mut after = &after[used..];
            // `[1]` arguments, and a `[default]` for the first.
            while let Some(options) = after.trim_start().strip_prefix('[') {
                let Some((_, r)) = options.split_once(']') else {
                    break;
                };
                after = r;
            }
            // `\def\x#1{…}`: its parameters, which KaTeX reads as `#1`.
            let after = after.trim_start_matches(|c: char| c == '#' || c.is_ascii_digit());
            let Some((body, _)) = group(after) else {
                continue;
            };
            let body = if command == "\\DeclareMathOperator" {
                format!("\\operatorname{}{{{body}}}", if star { "*" } else { "" })
            } else {
                body
            };
            if name.starts_with('\\') && !out.iter().any(|(n, _)| *n == name) {
                out.push((name, body));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\\documentclass{article}\n\\newcommand{\\R}{\\mathbb{R}}\n\\begin{document}\n\\maketitle\n\n\\section{Introduction}\n\\label{sec:intro}\nSome \\emph{text} over\ntwo lines, citing~\\cite{k}.\n\n% a note\n\\begin{equation}\n  x \\in \\R\n\\end{equation}\nThen more.\n\\[ y = 2 \\]\n\\begin{itemize}\n  \\item one\n  \\begin{itemize}\n    \\item nested\n  \\end{itemize}\n\\end{itemize}\n\\begin{figure}[ht]\n  \\includegraphics[width=0.5\\linewidth]{figures/plot.png}\n  \\caption{A plot.}\n\\end{figure}\n\\end{document}\n";

    /// The kinds of a body's blocks, blank lines left out.
    fn body_kinds(body: &str) -> Vec<(String, String)> {
        let doc = format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\\end{{document}}\n");
        let all = blocks(&doc);
        assert_eq!(all.iter().map(|b| b.text.as_str()).collect::<String>(), doc, "the blocks join back");
        all.into_iter()
            .filter(|b| !matches!(b.kind.as_str(), "space" | "preamble" | "end"))
            .map(|b| (b.kind, b.text.trim().to_string()))
            .collect()
    }

    #[test]
    fn a_paragraph_may_begin_with_lead_in_commands() {
        let k = body_kinds("\\noindent\\textbf{Keywords:} TinyML; quantisation.\n\n\\smallskip\n\\noindent\\textbf{ACM CCS:} Computing\nmethodologies.\n\n\\vspace{1em}\nText after space.\n");
        assert_eq!(
            k,
            vec![
                ("paragraph".into(), "\\noindent\\textbf{Keywords:} TinyML; quantisation.".into()),
                ("paragraph".into(), "\\smallskip\n\\noindent\\textbf{ACM CCS:} Computing\nmethodologies.".into()),
                ("paragraph".into(), "\\vspace{1em}\nText after space.".into()),
            ]
        );
    }

    #[test]
    fn lead_ins_before_no_text_still_stand_alone() {
        let k = body_kinds("\\smallskip\n\\section{Next}\n\\maketitle\n\\noindent % only a note\n\n");
        let kinds: Vec<&str> = k.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["raw", "heading", "raw"]);
    }

    #[test]
    fn an_abstract_is_a_block_of_its_own() {
        let k = body_kinds("\\begin{abstract}\nWords.\n\\end{abstract}\n");
        assert_eq!(k, vec![("abstract".into(), "\\begin{abstract}\nWords.\n\\end{abstract}".into())]);
    }

    #[test]
    fn a_file_is_cut_into_blocks_that_join_back_into_it_exactly() {
        let blocks = blocks(DOC);
        let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "preamble",
                "raw",
                "space",
                "heading",
                "paragraph",
                "space",
                "raw",
                "math",
                "paragraph",
                "math",
                "list",
                "figure",
                "end"
            ]
        );
        assert_eq!(
            blocks.iter().map(|b| b.text.as_str()).collect::<String>(),
            DOC,
            "joined, they are the source"
        );
        let chars: Vec<char> = DOC.chars().collect();
        for b in &blocks {
            assert_eq!(
                chars[b.start as usize..b.end as usize]
                    .iter()
                    .collect::<String>(),
                b.text,
                "{b:?}"
            );
        }
        assert_eq!(
            blocks[3].text,
            "\\section{Introduction}\n\\label{sec:intro}\n"
        );
        assert_eq!(blocks[3].level, Some(2));
        assert_eq!(
            blocks[4].text,
            "Some \\emph{text} over\ntwo lines, citing~\\cite{k}.\n"
        );
        assert_eq!(
            blocks[10].text.matches("\\end{itemize}").count(),
            2,
            "the nested list is the outer one's"
        );
        assert_eq!(blocks[11].image.as_deref(), Some("figures/plot.png"));
    }

    #[test]
    fn a_chapter_without_a_preamble_or_a_last_newline_still_joins_back() {
        for text in [
            "Just words",
            "\\chapter{One}\nWords.",
            "",
            "\\begin{equation}\nx\n",
            "$$\nx\n$$\nafter",
            "\\[\nx\n\\]",
            "a\n\n\n",
        ] {
            let cut = blocks(text);
            assert_eq!(
                cut.iter().map(|b| b.text.as_str()).collect::<String>(),
                text,
                "{text:?}: {cut:?}"
            );
        }
        let cut = blocks("$$\nx\n$$\nafter");
        assert_eq!(
            cut.iter().map(|b| b.kind.as_str()).collect::<Vec<_>>(),
            ["math", "paragraph"]
        );
    }

    #[test]
    fn a_preamble_s_macros_are_read_for_katex() {
        let found = macros("\\newcommand{\\R}{\\mathbb{R}}\n\\newcommand\\abs[1]{\\left|#1\\right|}\n\\DeclareMathOperator*{\\argmax}{arg\\,max}\n\\def\\eps{\\varepsilon}\n\\renewcommand{\\vec}[1]{\\mathbf{#1}}\n");
        assert_eq!(
            found,
            [
                ("\\R".to_string(), "\\mathbb{R}".to_string()),
                ("\\abs".to_string(), "\\left|#1\\right|".to_string()),
                ("\\vec".to_string(), "\\mathbf{#1}".to_string()),
                (
                    "\\argmax".to_string(),
                    "\\operatorname*{arg\\,max}".to_string()
                ),
                ("\\eps".to_string(), "\\varepsilon".to_string()),
            ]
        );
    }
}
