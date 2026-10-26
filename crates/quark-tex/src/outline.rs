//! A document's outline — its parts, chapters, sections and the rest, each
//! where it is, across every file its main file brings in — and how many
//! words it has.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::includes;

/// The commands a heading is made with, by depth.
const LEVELS: &[(&str, u8)] = &[
    ("part", 0),
    ("chapter", 1),
    ("section", 2),
    ("subsection", 3),
    ("subsubsection", 4),
    ("paragraph", 5),
    ("subparagraph", 6),
];

/// One heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heading {
    /// 0 for a part, 1 a chapter, 2 a section, down to 6 a subparagraph.
    pub level: u8,
    pub title: String,
    /// The file, as the project names it, and the line, one-based.
    pub file: String,
    pub line: u32,
    /// The label that follows it, when one does: `sec:intro`.
    pub label: Option<String>,
}

/// A document as a reader sees its shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outline {
    pub headings: Vec<Heading>,
    /// Words of prose in the body: not commands, comments or mathematics.
    pub words: u32,
}

/// A line with its comment taken off: a `%` not escaped ends it.
fn uncommented(line: &str) -> &str {
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if c == '%' && !escaped {
            return &line[..i];
        }
        escaped = c == '\\' && !escaped;
    }
    line
}

/// The text of the brace group that starts `text`, if it does, braces
/// within it counted.
fn group(text: &str) -> Option<&str> {
    let inner = text.trim_start().strip_prefix('{')?;
    let mut depth = 1;
    for (i, c) in inner.char_indices() {
        match c {
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

/// A heading's title as a reader would write it: markup taken out.
fn plain(title: &str) -> String {
    let mut out = String::new();
    let mut chars = title.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // `\emph`, `\textbf` and their kind go; their argument stays.
                while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    chars.next();
                }
            }
            '{' | '}' => {}
            '~' => out.push(' '),
            other => out.push(other),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The headings of one file, and those of the files it brings in, in order.
fn walk(
    file: &Path,
    root: &Path,
    main_dir: &Path,
    seen: &mut Vec<PathBuf>,
    out: &mut Vec<Heading>,
) {
    if seen.len() > 200 || seen.contains(&file.to_path_buf()) {
        return;
    }
    seen.push(file.to_path_buf());
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let name = file
        .strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string();
    for (n, raw) in text.lines().enumerate() {
        let line = uncommented(raw);
        for (command, level) in LEVELS {
            let needle = format!("\\{command}");
            let mut rest = line;
            while let Some(at) = rest.find(&needle) {
                let after = &rest[at + needle.len()..];
                rest = after;
                // `\sectionmark` is another command; `\section*` and an
                // optional short title are the same one.
                let after = after.strip_prefix('*').unwrap_or(after);
                if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    continue;
                }
                let after = match after.trim_start().strip_prefix('[') {
                    Some(short) => short.split_once(']').map_or(after, |(_, r)| r),
                    None => after,
                };
                if let Some(title) = group(after) {
                    out.push(Heading {
                        level: *level,
                        title: plain(title),
                        file: name.clone(),
                        line: n as u32 + 1,
                        label: None,
                    });
                }
            }
        }
        // A label just after a heading names it.
        if let Some(at) = line.find("\\label") {
            if let (Some(last), Some(label)) =
                (out.last_mut(), group(&line[at + "\\label".len()..]))
            {
                if last.file == name
                    && last.label.is_none()
                    && (n as u32 + 1).saturating_sub(last.line) <= 2
                {
                    last.label = Some(label.trim().to_string());
                }
            }
        }
        for included in includes(line) {
            for candidate in [
                main_dir.join(&included),
                file.parent().unwrap_or(main_dir).join(&included),
            ] {
                if candidate.is_file() {
                    walk(&candidate, root, main_dir, seen, out);
                    break;
                }
            }
        }
    }
}

/// The words of prose in text: what is left when comments, commands with
/// their options, environments' names and mathematics are taken out.
pub fn count_words(text: &str) -> u32 {
    let body = match text.find("\\begin{document}") {
        Some(at) => {
            &text
                [at + "\\begin{document}".len()..text.find("\\end{document}").unwrap_or(text.len())]
        }
        None if text.contains("\\documentclass") => "",
        None => text,
    };
    let mut prose = String::with_capacity(body.len());
    for line in body.lines() {
        prose.push_str(uncommented(line));
        prose.push('\n');
    }
    let chars: Vec<char> = prose.chars().collect();
    let mut words = 0;
    let mut in_word = false;
    let mut i = 0;
    let mut math: Option<&str> = None;
    while i < chars.len() {
        let c = chars[i];
        if let Some(end) = math {
            // Inside mathematics: nothing counts until it ends.
            let ends = match end {
                "$" => c == '$',
                "$$" => c == '$' && chars.get(i + 1) == Some(&'$'),
                _ => chars[i..].iter().collect::<String>().starts_with(end),
            };
            if ends {
                i += end.chars().count();
                math = None;
            } else {
                i += 1;
            }
            continue;
        }
        match c {
            '$' => {
                in_word = false;
                if chars.get(i + 1) == Some(&'$') {
                    math = Some("$$");
                    i += 2;
                } else {
                    math = Some("$");
                    i += 1;
                }
            }
            '\\' => {
                in_word = false;
                let name: String = chars[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                i += 1 + name.chars().count().max(1);
                match name.as_str() {
                    "[" | "" if chars.get(i - 1) == Some(&'[') => math = Some("\\]"),
                    "begin" => {
                        let env: String = chars[i..]
                            .iter()
                            .skip_while(|c| **c == '{')
                            .take_while(|c| **c != '}')
                            .collect();
                        let env = env.trim_end_matches('*');
                        if [
                            "equation",
                            "align",
                            "gather",
                            "multline",
                            "displaymath",
                            "math",
                            "eqnarray",
                        ]
                        .contains(&env)
                        {
                            math = Some(match env {
                                "equation" => "\\end{equation",
                                "align" => "\\end{align",
                                "gather" => "\\end{gather",
                                "multline" => "\\end{multline",
                                "displaymath" => "\\end{displaymath",
                                "math" => "\\end{math",
                                _ => "\\end{eqnarray",
                            });
                        }
                        // The environment's own name is not prose.
                        while i < chars.len() && chars[i] != '}' {
                            i += 1;
                        }
                    }
                    "end" | "label" | "ref" | "eqref" | "cite" | "citep" | "citet"
                    | "usepackage" | "input" | "include" | "includegraphics" | "bibliography"
                    | "bibliographystyle" | "documentclass" | "url" => {
                        // What follows is a name, not prose.
                        if chars.get(i) == Some(&'[') {
                            while i < chars.len() && chars[i] != ']' {
                                i += 1;
                            }
                        }
                        while i < chars.len() && chars[i] != '}' && chars[i] != '\n' {
                            i += 1;
                        }
                    }
                    _ => {}
                }
            }
            c if c.is_alphanumeric() || (in_word && (c == '\'' || c == '-')) => {
                if !in_word {
                    words += 1;
                    in_word = true;
                }
                i += 1;
            }
            _ => {
                in_word = false;
                i += 1;
            }
        }
    }
    words
}

/// A document's outline and word count, from its main file at `main`, in
/// the project at `root`.
pub fn outline(root: &Path, main: &Path) -> Outline {
    let main_dir = main.parent().unwrap_or(root);
    let mut headings = Vec::new();
    let mut seen = Vec::new();
    walk(main, root, main_dir, &mut seen, &mut headings);
    let words = seen
        .iter()
        .map(|file| {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            // Only the main file has a preamble to skip.
            if file == main {
                count_words(&text)
            } else {
                count_words(&format!("\\begin{{document}}{text}\\end{{document}}"))
            }
        })
        .sum();
    Outline { headings, words }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_counted_in_the_prose_and_not_in_commands_mathematics_or_comments() {
        let text = "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n\\section{Two words}\nIt's a well-known fact, as \\cite{knuth84} shows: $x^2 + y^2$ is \\emph{positive}. % not this\n\\begin{equation}\na = b\n\\end{equation}\nDone \\[ z \\] here.\n\\end{document}\nafter";
        // Two words; It's a well-known fact as shows is positive; Done here.
        assert_eq!(
            count_words(text),
            2 + 8 + 2,
            "counted: {}",
            count_words(text)
        );
        assert_eq!(
            count_words("no preamble here"),
            3,
            "a chapter file is all body"
        );
    }

    #[test]
    fn the_outline_follows_the_includes_in_order_with_labels() {
        let root = std::env::temp_dir().join(format!("quark-tex-outline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("chapters")).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{book}\n\\begin{document}\n\\chapter[Short]{The \\emph{first} chapter}\n\\label{ch:one}\n\\input{chapters/two}\n\\section*{Last}\n\\end{document}\n").unwrap();
        std::fs::write(root.join("chapters/two.tex"), "% \\section{Commented}\n\\section{Second~part}\\label{sec:two}\nText of ten words or so in the included chapter file here.\n").unwrap();
        let outline = outline(&root, &root.join("main.tex"));
        let titles: Vec<(u8, &str, &str, u32, Option<&str>)> = outline
            .headings
            .iter()
            .map(|h| {
                (
                    h.level,
                    h.title.as_str(),
                    h.file.as_str(),
                    h.line,
                    h.label.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            titles,
            [
                (1, "The first chapter", "main.tex", 3, Some("ch:one")),
                (2, "Second part", "chapters/two.tex", 2, Some("sec:two")),
                (2, "Last", "main.tex", 6, None)
            ]
        );
        assert!(outline.words >= 15, "{}", outline.words);
        let _ = std::fs::remove_dir_all(root);
    }
}
