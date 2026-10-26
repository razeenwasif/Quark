//! Markdown as LaTeX: what Research writes — briefs, hypotheses, verdicts —
//! turned into a paper's source.
//!
//! The Markdown Research writes is plain: headings, paragraphs, `**bold**`
//! and `*emphasis*`, `` `code` ``, fenced code, lists, links, and
//! mathematics between `$`s, which is TeX already and passes through as it
//! is. Everything else is text, escaped. A paper's id — `P003`, `[P003]`,
//! `[[literature/P003.pdf p. 4]]` — becomes `\cite{…}` when `cite` knows it.

/// Text escaped for TeX, outside mathematics.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '\\' => out.push_str("\\textbackslash{}"),
            '<' => out.push_str("\\textless{}"),
            '>' => out.push_str("\\textgreater{}"),
            // What pdfLaTeX has no glyph for in text, as mathematics.
            c => match symbol(c) {
                Some(command) => out.push_str(&format!("$\\{command}$")),
                None => out.push(c),
            },
        }
    }
    out
}

/// Greek letters and the commonest mathematical signs, by the command
/// that sets each: what Research writes in prose that pdfLaTeX cannot set
/// as text.
fn symbol(c: char) -> Option<&'static str> {
    const GREEK: [(char, &str); 30] = [
        ('α', "alpha"),
        ('β', "beta"),
        ('γ', "gamma"),
        ('δ', "delta"),
        ('ε', "epsilon"),
        ('ζ', "zeta"),
        ('η', "eta"),
        ('θ', "theta"),
        ('ι', "iota"),
        ('κ', "kappa"),
        ('λ', "lambda"),
        ('μ', "mu"),
        ('µ', "mu"),
        ('ν', "nu"),
        ('ξ', "xi"),
        ('π', "pi"),
        ('ρ', "rho"),
        ('σ', "sigma"),
        ('τ', "tau"),
        ('φ', "phi"),
        ('χ', "chi"),
        ('ψ', "psi"),
        ('ω', "omega"),
        ('Γ', "Gamma"),
        ('Δ', "Delta"),
        ('Θ', "Theta"),
        ('Λ', "Lambda"),
        ('Σ', "Sigma"),
        ('Φ', "Phi"),
        ('Ω', "Omega"),
    ];
    const SIGNS: [(char, &str); 14] = [
        ('≈', "approx"),
        ('≤', "leq"),
        ('≥', "geq"),
        ('≠', "neq"),
        ('×', "times"),
        ('±', "pm"),
        ('→', "to"),
        ('←', "leftarrow"),
        ('⇒', "Rightarrow"),
        ('∞', "infty"),
        ('∑', "sum"),
        ('∫', "int"),
        ('√', "surd"),
        ('∝', "propto"),
    ];
    GREEK
        .iter()
        .chain(SIGNS.iter())
        .find(|(g, _)| *g == c)
        .map(|(_, command)| *command)
}

/// Code as `\texttt` sets it.
fn escape_code(text: &str) -> String {
    escape_text(text).replace(' ', "~")
}

/// A paper's id at the start of `text` — `P003`, `[P003]`,
/// `[[literature/P003.pdf p. 4]]` — and how long the citation is, and its
/// page if it gives one.
fn paper_at(text: &str) -> Option<(String, usize, Option<String>)> {
    // `[[literature/P003.pdf p. 4]]`
    if let Some(inner) = text.strip_prefix("[[") {
        let end = inner.find("]]")?;
        let body = &inner[..end];
        let name = body.trim_start_matches("literature/");
        let id: String = name
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !is_id(&id) {
            return None;
        }
        let page = body
            .split_once(" p. ")
            .or_else(|| body.split_once(" p."))
            .map(|(_, p)| p.trim().to_string())
            .filter(|p| !p.is_empty());
        return Some((id, end + 4, page));
    }
    // `[P003]`, `[P003, P007]` is read as its first only when alone.
    if let Some(inner) = text.strip_prefix('[') {
        let id: String = inner
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if is_id(&id) && inner[id.len()..].starts_with(']') {
            return Some((id.clone(), id.len() + 2, None));
        }
        return None;
    }
    let id: String = text
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    is_id(&id).then(|| (id.clone(), id.len(), None))
}

/// `P` and three or more digits.
fn is_id(word: &str) -> bool {
    word.len() >= 4 && word.starts_with('P') && word[1..].chars().all(|c| c.is_ascii_digit())
}

/// One line's inline Markdown as LaTeX.
fn inline(text: &str, cite: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    let mut previous: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        let boundary = previous.is_none_or(|p| !p.is_ascii_alphanumeric());
        // A paper cited.
        if boundary && (c == 'P' || c == '[') {
            if let Some((id, len, page)) = paper_at(rest) {
                if let Some(key) = cite(&id) {
                    match page {
                        Some(page) => {
                            out.push_str(&format!("\\cite[p.~{}]{{{key}}}", escape_text(&page)))
                        }
                        None => out.push_str(&format!("\\cite{{{key}}}")),
                    }
                    rest = &rest[len..];
                    previous = Some(']');
                    continue;
                }
            }
        }
        // Mathematics, as it is: `$…$` and `$$…$$`.
        if c == '$' {
            let fence = if rest.starts_with("$$") { "$$" } else { "$" };
            if let Some(end) = rest[fence.len()..].find(fence) {
                let body = &rest[fence.len()..fence.len() + end];
                // As Pandoc reads it: no space just inside a single `$`, and
                // no digit just after the closing one — `$5 and $10` is money.
                let after = rest[fence.len() * 2 + end..].chars().next();
                let tight = fence == "$$"
                    || (!body.starts_with(char::is_whitespace)
                        && !body.ends_with(char::is_whitespace)
                        && !after.is_some_and(|c| c.is_ascii_digit()));
                if !body.trim().is_empty() && tight {
                    if fence == "$$" {
                        out.push_str(&format!("\\[{body}\\]"));
                    } else {
                        out.push_str(&format!("${body}$"));
                    }
                    rest = &rest[fence.len() * 2 + end..];
                    previous = Some('$');
                    continue;
                }
            }
        }
        // Code.
        if c == '`' {
            if let Some(end) = rest[1..].find('`') {
                out.push_str(&format!("\\texttt{{{}}}", escape_code(&rest[1..1 + end])));
                rest = &rest[end + 2..];
                previous = Some('`');
                continue;
            }
        }
        // Bold, then emphasis.
        for (mark, command) in [
            ("**", "textbf"),
            ("__", "textbf"),
            ("*", "emph"),
            ("_", "emph"),
        ] {
            if rest.starts_with(mark) && (mark.starts_with('*') || boundary) {
                if let Some(end) = rest[mark.len()..].find(mark) {
                    let body = &rest[mark.len()..mark.len() + end];
                    if !body.is_empty() && !body.starts_with(' ') && !body.ends_with(' ') {
                        out.push_str(&format!("\\{command}{{{}}}", inline(body, cite)));
                        rest = &rest[mark.len() * 2 + end..];
                        previous = Some('}');
                        break;
                    }
                }
            }
        }
        if previous == Some('}') && !rest.starts_with(c) {
            previous = None;
            continue;
        }
        // A link: `[text](url)`.
        if c == '[' {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let label = &rest[1..close];
                    let url = &rest[close + 2..close + 2 + end];
                    if !label.contains('[') && !url.contains(' ') {
                        out.push_str(&format!(
                            "\\href{{{}}}{{{}}}",
                            url.replace('%', "\\%").replace('#', "\\#"),
                            inline(label, cite)
                        ));
                        rest = &rest[close + 3 + end..];
                        previous = Some(')');
                        continue;
                    }
                }
            }
        }
        out.push_str(&escape_text(&c.to_string()));
        rest = &rest[c.len_utf8()..];
        previous = Some(c);
    }
    out
}

/// A list item's marker and its text, if `line` is one: `- x`, `* x`,
/// `1. x`.
fn item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return Some((false, rest));
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, rest));
        }
    }
    None
}

/// Markdown as LaTeX. Headings start at `top` — `\section` for 1,
/// `\subsection` for 2 — and go deeper as Markdown's do; `cite` gives a
/// paper's key by its id, or nothing for one it does not know, which stays
/// as it was written.
pub fn to_latex(markdown: &str, top: usize, cite: &dyn Fn(&str) -> Option<String>) -> String {
    const LEVELS: [&str; 5] = [
        "section",
        "subsection",
        "subsubsection",
        "paragraph",
        "subparagraph",
    ];
    let mut out = String::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut list: Option<bool> = None;
    let mut lines = markdown.lines().peekable();
    let flush = |paragraph: &mut Vec<String>, out: &mut String| {
        if !paragraph.is_empty() {
            out.push_str(&paragraph.join("\n"));
            out.push_str("\n\n");
            paragraph.clear();
        }
    };
    let close = |list: &mut Option<bool>, out: &mut String| {
        if let Some(numbered) = list.take() {
            out.push_str(if numbered {
                "\\end{enumerate}\n\n"
            } else {
                "\\end{itemize}\n\n"
            });
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        // Fenced code, as it is.
        if trimmed.starts_with("```") {
            flush(&mut paragraph, &mut out);
            close(&mut list, &mut out);
            out.push_str("\\begin{verbatim}\n");
            for code in lines.by_ref() {
                if code.trim_start().starts_with("```") {
                    break;
                }
                out.push_str(code);
                out.push('\n');
            }
            out.push_str("\\end{verbatim}\n\n");
            continue;
        }
        // Display mathematics on lines of its own.
        if trimmed == "$$" {
            flush(&mut paragraph, &mut out);
            close(&mut list, &mut out);
            out.push_str("\\[\n");
            for math in lines.by_ref() {
                if math.trim() == "$$" {
                    break;
                }
                out.push_str(math);
                out.push('\n');
            }
            out.push_str("\\]\n\n");
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut out);
            if lines.peek().is_none_or(|next| item(next).is_none()) {
                close(&mut list, &mut out);
            }
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && trimmed[hashes..].starts_with(' ') {
            flush(&mut paragraph, &mut out);
            close(&mut list, &mut out);
            let level = LEVELS[(top + hashes - 2).min(LEVELS.len() - 1)];
            out.push_str(&format!(
                "\\{level}{{{}}}\n\n",
                inline(trimmed[hashes..].trim(), cite)
            ));
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            flush(&mut paragraph, &mut out);
            close(&mut list, &mut out);
            continue;
        }
        if let Some((numbered, text)) = item(line) {
            flush(&mut paragraph, &mut out);
            if list != Some(numbered) {
                close(&mut list, &mut out);
                out.push_str(if numbered {
                    "\\begin{enumerate}\n"
                } else {
                    "\\begin{itemize}\n"
                });
                list = Some(numbered);
            }
            out.push_str(&format!("  \\item {}\n", inline(text.trim(), cite)));
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>') {
            paragraph.push(format!("\\emph{{{}}}", inline(quote.trim(), cite)));
            continue;
        }
        if list.is_some() && line.starts_with(char::is_whitespace) {
            // A list item's text, carried on to the next line.
            out.push_str(&format!("    {}\n", inline(trimmed, cite)));
            continue;
        }
        close(&mut list, &mut out);
        paragraph.push(inline(trimmed, cite));
    }
    flush(&mut paragraph, &mut out);
    close(&mut list, &mut out);
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cite(id: &str) -> Option<String> {
        (id == "P003").then(|| "rayleigh1899light".to_string())
    }

    #[test]
    fn markdown_becomes_latex_with_its_text_escaped_and_its_mathematics_kept() {
        let md = "## Method\n\nWe fit **all** of it, as *P003* and [P003] did — 100% & more, `a_b` with $x_1^2$ — see [[literature/P003.pdf p. 4]] but not P999.\n\n- one\n- two with [a link](https://example.org/a#b)\n\n1. first\n2. second\n\n```\nlet x = {1};\n```\n\n$$\nE = mc^2\n$$\n";
        let tex = to_latex(md, 1, &cite);
        assert_eq!(tex, "\\subsection{Method}\n\n\
We fit \\textbf{all} of it, as \\emph{\\cite{rayleigh1899light}} and \\cite{rayleigh1899light} did — 100\\% \\& more, \\texttt{a\\_b} with $x_1^2$ — see \\cite[p.~4]{rayleigh1899light} but not P999.\n\n\
\\begin{itemize}\n  \\item one\n  \\item two with \\href{https://example.org/a\\#b}{a link}\n\\end{itemize}\n\n\
\\begin{enumerate}\n  \\item first\n  \\item second\n\\end{enumerate}\n\n\
\\begin{verbatim}\nlet x = {1};\n\\end{verbatim}\n\n\
\\[\nE = mc^2\n\\]\n");
    }

    #[test]
    fn headings_start_where_they_are_told_and_words_are_not_ids() {
        assert_eq!(
            to_latex("# Results\n### Detail", 2, &cite),
            "\\subsection{Results}\n\n\\paragraph{Detail}\n"
        );
        assert_eq!(
            to_latex("PROP003 and P0031x and snake_case_name", 1, &cite),
            "PROP003 and P0031x and snake\\_case\\_name\n"
        );
        assert_eq!(
            to_latex("cost: $5 and $10", 1, &|_| None),
            "cost: $5 and $10\n".replace('$', "\\$"),
            "dollars that are not mathematics"
        );
    }

    #[test]
    fn greek_and_signs_in_prose_are_set_as_mathematics() {
        assert_eq!(
            to_latex("λ ≈ 3 × 10 → ∞, 20°C", 1, &|_| None),
            "$\\lambda$ $\\approx$ 3 $\\times$ 10 $\\to$ $\\infty$, 20°C\n"
        );
    }
}
