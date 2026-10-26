//! What a build said: TeX's log, and BibTeX's or biber's, read into errors,
//! warnings and bad boxes, each where it is in the project.
//!
//! The build runs TeX with `-file-line-error` and an unwrapped log
//! (`max_print_line=10000`), so an error reads
//!
//! ```text
//! ./chapters/intro.tex:12: Undefined control sequence.
//! l.12 \foo
//! ```
//!
//! and most warnings name their line (`on input line 23`). What names no
//! file — an overfull box, a package's warning — is placed by the file
//! stack: TeX prints `(./chapters/intro.tex` when it opens a file and `)`
//! when it closes one, among a good many other parentheses. A file is
//! anything opened that looks like one; everything else opened is closed
//! again without being taken for a file.

use serde::{Deserialize, Serialize};

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Error,
    Warning,
    /// An overfull or underfull box: typeset, but not well.
    Box,
}

/// One thing a build said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub kind: Kind,
    /// The file, as the project names it: `chapters/intro.tex`. None when
    /// nothing says which.
    pub file: Option<String>,
    /// One-based, as an editor's gutter counts.
    pub line: Option<u32>,
    pub message: String,
    /// The source TeX was reading when it stopped: `\foo`.
    pub context: Option<String>,
}

/// Everything a build said, read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub entries: Vec<Entry>,
    /// Files TeX looked for and its installation lacks — `xcolor.sty`,
    /// `IEEEtran.cls`, `plainnat.bst` — which a package would bring.
    pub missing: Vec<String>,
    /// Pages in the PDF, when TeX said.
    pub pages: Option<u32>,
}

impl Log {
    pub fn errors(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.kind == Kind::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.kind == Kind::Warning)
            .count()
    }

    pub fn boxes(&self) -> usize {
        self.entries.iter().filter(|e| e.kind == Kind::Box).count()
    }
}

/// How TeX names the file a message is about, made the project's own: a
/// path relative to `main_dir` (where TeX ran) put under `root`, and taken
/// out of it. A file outside the project — a package's own `.sty` — keeps
/// its whole path.
pub fn project_path(name: &str, main_dir: &str, root: &str) -> String {
    let name = name.trim();
    let joined = if name.starts_with('/') {
        name.to_string()
    } else {
        format!(
            "{}/{}",
            main_dir.trim_end_matches('/'),
            name.trim_start_matches("./")
        )
    };
    let normal = normalise(&joined);
    let root = normalise(root.trim_end_matches('/'));
    match normal.strip_prefix(&format!("{root}/")) {
        Some(rel) => rel.to_string(),
        None => normal,
    }
}

/// A path with its `.` and `..` worked out.
fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!(
        "{}{}",
        if path.starts_with('/') { "/" } else { "" },
        parts.join("/")
    )
}

/// Whether what follows an opening parenthesis is a file TeX opened.
fn file_like(token: &str) -> bool {
    let t = token.trim_matches(|c: char| c == '"' || c == '\'');
    if t.is_empty() || t.contains(char::is_whitespace) {
        return false;
    }
    let looks = t.starts_with("./") || t.starts_with('/') || t.starts_with("../");
    let ext = t
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.starts_with(|c: char| c.is_ascii_alphabetic())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
    looks || ext
}

/// The file stack, as TeX's parentheses say.
#[derive(Default)]
struct Stack {
    /// Each `(` opened: the file it opened, or nothing.
    open: Vec<Option<String>>,
}

impl Stack {
    fn current(&self) -> Option<&str> {
        self.open.iter().rev().find_map(|f| f.as_deref())
    }

    /// Follow a line's parentheses.
    fn follow(&mut self, line: &str) {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '(' => {
                    let rest: String = chars[i + 1..]
                        .iter()
                        .take_while(|c| !c.is_whitespace() && **c != '(' && **c != ')')
                        .collect();
                    self.open
                        .push(file_like(&rest).then(|| rest.trim_matches('"').to_string()));
                    i += 1 + rest.chars().count();
                }
                ')' => {
                    self.open.pop();
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }
}

/// The number after `on input line `, if a message ends with one.
fn input_line(text: &str) -> Option<u32> {
    let at = text.rfind("on input line ")?;
    text[at + "on input line ".len()..]
        .trim_end_matches('.')
        .trim()
        .parse()
        .ok()
}

/// The lines a box warning names: `in paragraph at lines 10--12` → 10.
fn box_line(text: &str) -> Option<u32> {
    let at = text
        .find("at lines ")
        .map(|i| i + "at lines ".len())
        .or_else(|| text.find("at line ").map(|i| i + "at line ".len()))?;
    text[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// A missing file a message names: `File `xcolor.sty' not found`.
fn missing_file(text: &str) -> Option<String> {
    let at = text.find("File `")? + "File `".len();
    let end = text[at..].find('\'')?;
    let name = &text[at..at + end];
    text[at + end..]
        .contains("not found")
        .then(|| name.to_string())
}

/// `file:line: message`, as `-file-line-error` writes an error.
fn file_line_error(line: &str) -> Option<(String, u32, String)> {
    // The file may hold colons; the line is the first `:digits:` after it.
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(off) = line[i..].find(':') {
        let start = i + off;
        let digits: String = line[start + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let after = start + 1 + digits.len();
        if !digits.is_empty() && bytes.get(after) == Some(&b':') && start > 0 {
            let file = &line[..start];
            if file_like(file) {
                return Some((
                    file.to_string(),
                    digits.parse().ok()?,
                    line[after + 1..].trim().to_string(),
                ));
            }
        }
        i = start + 1;
    }
    None
}

/// Read TeX's log. `main_dir` is where TeX ran — the main file's folder —
/// and `root` the project, so files are named as the project names them.
pub fn read(text: &str, main_dir: &str, root: &str) -> Log {
    let mut log = Log::default();
    let mut stack = Stack::default();
    let lines: Vec<&str> = text.lines().collect();
    let place = |stack: &Stack| stack.current().map(|f| project_path(f, main_dir, root));
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((file, number, message)) = file_line_error(line) {
            let context = lines[i + 1..]
                .iter()
                .take(8)
                .find(|l| l.starts_with("l."))
                .map(|l| l.split_once(' ').map_or("", |(_, t)| t).trim().to_string());
            if let Some(name) = missing_file(&message) {
                if !log.missing.contains(&name) {
                    log.missing.push(name);
                }
            }
            log.entries.push(Entry {
                kind: Kind::Error,
                file: Some(project_path(&file, main_dir, root)),
                line: Some(number),
                message,
                context,
            });
            i += 1;
            continue;
        }
        if let Some(message) = line.strip_prefix("! ") {
            // An error with no file: where the stack is.
            let context = lines[i + 1..].iter().take(8).find(|l| l.starts_with("l."));
            let number = context.and_then(|l| l[2..].split(' ').next()?.parse().ok());
            if let Some(name) = missing_file(message) {
                if !log.missing.contains(&name) {
                    log.missing.push(name);
                }
            }
            let context =
                context.map(|l| l.split_once(' ').map_or("", |(_, t)| t).trim().to_string());
            log.entries.push(Entry {
                kind: Kind::Error,
                file: place(&stack),
                line: number,
                message: message.trim().to_string(),
                context,
            });
            i += 1;
            continue;
        }
        let warning = [
            "LaTeX Warning: ",
            "LaTeX Font Warning: ",
            "Package ",
            "Class ",
        ]
        .iter()
        .any(|p| line.starts_with(p))
            && line.contains("Warning: ");
        if warning {
            // A warning goes on over lines that begin with its package in
            // parentheses, padded.
            let mut message = line
                .split_once("Warning: ")
                .map_or(line, |(_, m)| m)
                .trim()
                .to_string();
            let from = line
                .split_once(" Warning: ")
                .map(|(who, _)| who.to_string())
                .unwrap_or_default();
            let mut j = i + 1;
            while j < lines.len()
                && lines[j].trim_start().starts_with('(')
                && lines[j].contains(')')
                && lines[j].trim_start().len() < lines[j].len() + 1
            {
                let more = lines[j].trim_start();
                let Some((_, rest)) = more.split_once(')') else {
                    break;
                };
                if !more.starts_with(&format!(
                    "({}",
                    from.split_whitespace().nth(1).unwrap_or("")
                )) && !from.starts_with("LaTeX")
                {
                    break;
                }
                message.push(' ');
                message.push_str(rest.trim());
                j += 1;
            }
            let prefix = if from.starts_with("Package ") || from.starts_with("Class ") {
                format!("{}: ", from.split_whitespace().nth(1).unwrap_or(""))
            } else {
                String::new()
            };
            log.entries.push(Entry {
                kind: Kind::Warning,
                file: place(&stack),
                line: input_line(&message),
                message: format!("{prefix}{message}"),
                context: None,
            });
            stack.follow(line);
            i = j;
            continue;
        }
        if line.starts_with("Overfull \\") || line.starts_with("Underfull \\") {
            log.entries.push(Entry {
                kind: Kind::Box,
                file: place(&stack),
                line: box_line(line),
                message: line.trim().to_string(),
                context: None,
            });
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Output written on ") {
            log.pages = rest
                .split('(')
                .nth(1)
                .and_then(|p| p.split_whitespace().next()?.parse().ok());
        }
        stack.follow(line);
        i += 1;
    }
    log
}

/// Read BibTeX's `.blg` or biber's: an error or a warning each, about the
/// bibliography, and the style file it could not find.
pub fn read_bib(text: &str, main_dir: &str, root: &str) -> Log {
    let mut log = Log::default();
    for line in text.lines() {
        let line = line.trim_end();
        // biber: `[42] Utils.pm:403> ERROR - …` and `WARN - …`
        if let Some((_, message)) = line.split_once("> ERROR - ") {
            log.entries.push(Entry {
                kind: Kind::Error,
                file: None,
                line: None,
                message: format!("biber: {}", message.trim()),
                context: None,
            });
        } else if let Some((_, message)) = line.split_once("> WARN - ") {
            log.entries.push(Entry {
                kind: Kind::Warning,
                file: None,
                line: None,
                message: format!("biber: {}", message.trim()),
                context: None,
            });
        } else if let Some(message) = line.strip_prefix("Warning--") {
            log.entries.push(Entry {
                kind: Kind::Warning,
                file: None,
                line: None,
                message: format!("BibTeX: {}", message.trim()),
                context: None,
            });
        } else if line.starts_with("I couldn't open ") || line.starts_with("I found no ") {
            if let Some(style) = line.strip_prefix("I couldn't open style file ") {
                let name = style.trim().to_string();
                if !log.missing.contains(&name) {
                    log.missing.push(name);
                }
            }
            log.entries.push(Entry {
                kind: Kind::Error,
                file: None,
                line: None,
                message: format!("BibTeX: {}", line.trim()),
                context: None,
            });
        } else if let Some(rest) = line.strip_prefix("---line ") {
            // `---line 12 of file refs.bib` follows a BibTeX error about an entry.
            if let (Some(number), Some(file)) = (
                rest.split_whitespace().next().and_then(|n| n.parse().ok()),
                rest.split(" of file ").nth(1),
            ) {
                if let Some(last) = log.entries.last_mut() {
                    last.file = Some(project_path(file.trim(), main_dir, root));
                    last.line = Some(number);
                }
            }
        } else if line.contains("---") && line.contains("error") {
            log.entries.push(Entry {
                kind: Kind::Error,
                file: None,
                line: None,
                message: format!("BibTeX: {}", line.trim()),
                context: None,
            });
        }
    }
    log
}

/// How many errors, and warnings, a description shows.
pub const DESCRIBED_ERRORS: usize = 20;
const DESCRIBED_WARNINGS: usize = 10;
/// The lines shown either side of an error's.
const AROUND: u32 = 3;

/// A log as an agent reads it: `header` — which file was typeset, how, and
/// what came of it — then each error where it is, with the source lines
/// round it from `source` (a project file's text, by the name the log
/// gives it), what TeX lacked, and the first warnings.
pub fn describe(log: &Log, header: &str, source: impl Fn(&str) -> Option<String>) -> String {
    let mut out = header.trim_end().to_string();
    let errors: Vec<&Entry> = log
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Error)
        .collect();
    if !errors.is_empty() {
        out.push_str("\n\nErrors:");
        for (n, e) in errors.iter().take(DESCRIBED_ERRORS).enumerate() {
            let place = match (&e.file, e.line) {
                (Some(file), Some(line)) => format!("`{file}` line {line}"),
                (Some(file), None) => format!("`{file}`"),
                _ => "somewhere TeX did not say".into(),
            };
            out.push_str(&format!("\n{}. {place}: {}", n + 1, e.message));
            if let Some(context) = &e.context {
                out.push_str(&format!("\n   TeX stopped at: {context}"));
            }
            if let (Some(file), Some(line)) = (&e.file, e.line) {
                if let Some(text) = source(file) {
                    let lines: Vec<&str> = text.lines().collect();
                    let from = line.saturating_sub(AROUND).max(1);
                    let to = (line + AROUND).min(lines.len() as u32);
                    if from <= to {
                        out.push_str("\n   ```latex");
                        for at in from..=to {
                            let mark = if at == line { ">" } else { " " };
                            out.push_str(&format!(
                                "\n   {mark}{at:>4}  {}",
                                lines[at as usize - 1]
                            ));
                        }
                        out.push_str("\n   ```");
                    }
                }
            }
        }
        if errors.len() > DESCRIBED_ERRORS {
            out.push_str(&format!(
                "\n({} more errors not shown: the first is usually the cause of the rest.)",
                errors.len() - DESCRIBED_ERRORS
            ));
        }
    }
    if !log.missing.is_empty() {
        out.push_str(&format!(
            "\n\nTeX looked for files it does not have: {}. A package would bring them; the person installs it from the Log tab, so suggest that rather than rewriting the document round them.",
            log.missing.join(", ")
        ));
    }
    let warnings: Vec<&Entry> = log
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Warning)
        .collect();
    if !warnings.is_empty() {
        out.push_str("\n\nWarnings:");
        for e in warnings.iter().take(DESCRIBED_WARNINGS) {
            let place = match (&e.file, e.line) {
                (Some(file), Some(line)) => format!("`{file}` line {line}: "),
                (Some(file), None) => format!("`{file}`: "),
                _ => String::new(),
            };
            out.push_str(&format!("\n- {place}{}", e.message));
        }
        if warnings.len() > DESCRIBED_WARNINGS {
            out.push_str(&format!("\n({} more)", warnings.len() - DESCRIBED_WARNINGS));
        }
    }
    if errors.is_empty() && warnings.is_empty() && log.missing.is_empty() {
        out.push_str("\n\nThe log has no errors or warnings.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/Users/you/paper";

    #[test]
    fn errors_with_their_file_line_and_context() {
        let log = read(include_str!("../tests/fixtures/error.log"), ROOT, ROOT);
        assert_eq!(log.errors(), 2, "{:?}", log.entries);
        let first = &log.entries.iter().find(|e| e.kind == Kind::Error).unwrap();
        assert_eq!(
            (first.file.as_deref(), first.line, first.message.as_str()),
            (
                Some("chapters/intro.tex"),
                Some(12),
                "Undefined control sequence."
            )
        );
        assert_eq!(first.context.as_deref(), Some("We use \\foo"));
        let second = log
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Error)
            .nth(1)
            .unwrap();
        assert_eq!(
            (second.file.as_deref(), second.line),
            (Some("main.tex"), Some(20))
        );
        assert!(second.message.starts_with("Missing $ inserted"));
        assert_eq!(log.pages, Some(3));
    }

    #[test]
    fn a_missing_package_or_class_is_named_for_installing() {
        let log = read(include_str!("../tests/fixtures/missing.log"), ROOT, ROOT);
        assert_eq!(log.missing, ["tikz-cd.sty"], "{:?}", log.entries);
        assert!(log.entries[0]
            .message
            .contains("File `tikz-cd.sty' not found"));
        assert_eq!(
            (log.entries[0].file.as_deref(), log.entries[0].line),
            (Some("main.tex"), Some(4))
        );
    }

    #[test]
    fn warnings_and_bad_boxes_are_placed_by_the_file_stack() {
        let log = read(include_str!("../tests/fixtures/warnings.log"), ROOT, ROOT);
        let cite = log
            .entries
            .iter()
            .find(|e| e.message.contains("Citation `knuth84'"))
            .unwrap();
        assert_eq!(
            (cite.kind, cite.file.as_deref(), cite.line),
            (Kind::Warning, Some("chapters/method.tex"), Some(7)),
            "{cite:?}"
        );
        let refer = log
            .entries
            .iter()
            .find(|e| e.message.contains("Reference `fig:loop'"))
            .unwrap();
        assert_eq!(
            (refer.file.as_deref(), refer.line),
            (Some("main.tex"), Some(31)),
            "back in main.tex once method.tex closed"
        );
        let boxed = log.entries.iter().find(|e| e.kind == Kind::Box).unwrap();
        assert_eq!(
            (boxed.file.as_deref(), boxed.line),
            (Some("chapters/method.tex"), Some(14)),
            "{boxed:?}"
        );
        let hyper = log
            .entries
            .iter()
            .find(|e| e.message.starts_with("hyperref:"))
            .unwrap();
        assert!(
            hyper.message.contains("Token not allowed in a PDF string")
                && hyper.message.contains("removing `math shift'"),
            "continued over its lines: {hyper:?}"
        );
        assert_eq!((log.errors(), log.warnings(), log.boxes()), (0, 4, 2));
    }

    #[test]
    fn bibtex_and_biber_say_what_went_wrong_with_the_bibliography() {
        let bib = read_bib(include_str!("../tests/fixtures/bibtex.blg"), ROOT, ROOT);
        assert_eq!(bib.missing, ["acmart.bst"]);
        let warned = bib
            .entries
            .iter()
            .find(|e| e.kind == Kind::Warning)
            .unwrap();
        assert!(warned
            .message
            .contains("didn't find a database entry for \"lamport94\""));
        let biber = read_bib("[12] Utils.pm:418> ERROR - Cannot find 'refs.bib'!\n[13] Biber.pm:1100> WARN - Duplicate entry key 'knuth84'\n", ROOT, ROOT);
        assert_eq!((biber.errors(), biber.warnings()), (1, 1));
        assert!(biber.entries[0].message.contains("Cannot find 'refs.bib'"));
    }

    #[test]
    fn paths_are_the_projects_own_and_parentheses_that_are_not_files_are_not() {
        assert_eq!(
            project_path("./chapters/a.tex", ROOT, ROOT),
            "chapters/a.tex"
        );
        assert_eq!(
            project_path("../shared/defs.tex", "/Users/you/paper/sub", ROOT),
            "shared/defs.tex"
        );
        assert_eq!(
            project_path("/usr/share/texmf/tex/latex/base/article.cls", ROOT, ROOT),
            "/usr/share/texmf/tex/latex/base/article.cls"
        );
        let mut stack = Stack::default();
        stack.follow("(./main.tex (see the note) (./sections/a.tex");
        assert_eq!(stack.current(), Some("./sections/a.tex"));
        stack.follow(")");
        assert_eq!(stack.current(), Some("./main.tex"));
        assert!(
            !file_like("12.0pt")
                && !file_like("see")
                && file_like("article.cls")
                && file_like("./x.tex"),
            "a measure is not a file"
        );
    }

    #[test]
    fn a_log_is_described_with_each_error_s_source_round_it() {
        let log = Log {
            entries: vec![
                Entry {
                    kind: Kind::Error,
                    file: Some("chapters/intro.tex".into()),
                    line: Some(5),
                    message: "Undefined control sequence.".into(),
                    context: Some("l.5 \\foo".into()),
                },
                Entry {
                    kind: Kind::Warning,
                    file: Some("main.tex".into()),
                    line: Some(2),
                    message: "Citation `knuth84' undefined".into(),
                    context: None,
                },
                Entry {
                    kind: Kind::Box,
                    file: None,
                    line: None,
                    message: "Overfull \\hbox".into(),
                    context: None,
                },
            ],
            missing: vec!["tikz-cd.sty".into()],
            pages: None,
        };
        let text = describe(
            &log,
            "`main.tex`, typeset with pdfLaTeX: 1 error.",
            |file| {
                (file == "chapters/intro.tex").then(|| {
                    (1..=9)
                        .map(|n| format!("line {n}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            },
        );
        assert!(text.starts_with("`main.tex`, typeset with pdfLaTeX: 1 error.\n\nErrors:\n1. `chapters/intro.tex` line 5: Undefined control sequence.\n   TeX stopped at: l.5 \\foo\n   ```latex\n       2  line 2\n"), "{text}");
        assert!(
            text.contains("\n   >   5  line 5\n") && text.contains("       8  line 8\n   ```"),
            "{text}"
        );
        assert!(
            !text.contains("line 9") && !text.contains("line 1\n"),
            "three lines either side: {text}"
        );
        assert!(text.contains("does not have: tikz-cd.sty"));
        assert!(text.contains("Warnings:\n- `main.tex` line 2: Citation `knuth84' undefined"));
        assert!(!text.contains("Overfull"), "bad boxes are left out");
        let clean = describe(&Log::default(), "`main.tex`: typeset.", |_| None);
        assert_eq!(
            clean,
            "`main.tex`: typeset.\n\nThe log has no errors or warnings."
        );
    }
}
