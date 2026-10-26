//! A LaTeX project as a build needs it: which file is typeset — its main
//! file — and with which engine.
//!
//! A project of several files is typeset from one. That one is, first
//! found of:
//!
//! 1. the main file the person pinned (`.quark/tex.json`);
//! 2. the file a `% !TEX root = …` line at the top of the file being edited
//!    names;
//! 3. the file being edited, if it begins a document (`\documentclass`);
//! 4. a top-level file called `main`, `paper`, `thesis`, `ms` or `report`;
//! 5. the top-level file whose `\input`s and `\include`s reach the one being
//!    edited;
//! 6. the only top-level file that begins a document.
//!
//! The engine is the `% !TEX program = …` line's, else the person's
//! setting, else what the preamble needs — fontspec and its kin want
//! XeLaTeX, `\directlua` LuaLaTeX — else pdfLaTeX.

use std::path::{Path, PathBuf};

/// The names a main file is conventionally given.
const CONVENTIONAL: &[&str] = &[
    "main", "paper", "thesis", "ms", "report", "article", "document",
];

/// How many files of a project are read looking for its main one.
const LOOKED: usize = 400;

/// What a file's `% !TEX` lines say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Magic {
    /// `% !TEX root = ../main.tex`: the file to typeset instead.
    pub root: Option<String>,
    /// `% !TEX program = xelatex`.
    pub program: Option<String>,
}

/// The `% !TEX` lines at the top of a file — before anything that is not a
/// comment or blank.
pub fn magic(text: &str) -> Magic {
    let mut found = Magic::default();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix('%') else {
            break;
        };
        let comment = comment.trim_start();
        let Some(rest) = comment.strip_prefix('!').map(str::trim_start) else {
            continue;
        };
        let lower = rest.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("tex").map(|_| rest[3..].trim_start()) else {
            continue;
        };
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_string());
        match key.as_str() {
            "root" if !value.is_empty() => found.root = Some(value),
            "program" | "ts-program" if !value.is_empty() => {
                found.program = Some(value.to_ascii_lowercase())
            }
            _ => {}
        }
    }
    found
}

/// The lines of a file that are not comments: a `%` not escaped ends one.
fn uncommented(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().map(|line| {
        let mut out = String::new();
        let mut escaped = false;
        for c in line.chars() {
            if c == '%' && !escaped {
                break;
            }
            escaped = c == '\\' && !escaped;
            out.push(c);
        }
        out
    })
}

/// Whether a file begins a document of its own.
pub fn is_main(text: &str) -> bool {
    uncommented(text).any(|l| l.contains("\\documentclass"))
}

/// The files a file brings in, as written: `\input{a}`, `\include{b}`,
/// `\subfile{c}`, `\import{dir/}{d}` — each with `.tex` when it has no
/// extension.
pub fn includes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in uncommented(text) {
        for command in [
            "\\input",
            "\\include",
            "\\subfile",
            "\\import",
            "\\subimport",
            "\\includeonly",
        ] {
            let mut rest = line.as_str();
            while let Some(at) = rest.find(command) {
                let after = &rest[at + command.len()..];
                rest = after;
                // `\inputencoding` and `\includegraphics` are other commands.
                if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    continue;
                }
                let args = braced(after, if command.ends_with("import") { 2 } else { 1 });
                let name = match args.as_slice() {
                    [dir, file] => format!("{}/{}", dir.trim_end_matches('/'), file),
                    [file] if command != "\\includeonly" => file.clone(),
                    _ => continue,
                };
                let name = name.trim().to_string();
                if name.is_empty() {
                    continue;
                }
                let with_ext = if Path::new(&name).extension().is_some() {
                    name
                } else {
                    format!("{name}.tex")
                };
                if !out.contains(&with_ext) {
                    out.push(with_ext);
                }
            }
        }
    }
    out
}

/// The first `count` brace groups at the start of `text`, if they are there.
fn braced(text: &str, count: usize) -> Vec<String> {
    let mut groups = Vec::new();
    let mut rest = text.trim_start();
    while groups.len() < count {
        let Some(inner) = rest.strip_prefix('{') else {
            break;
        };
        let Some(end) = inner.find('}') else { break };
        groups.push(inner[..end].to_string());
        rest = inner[end + 1..].trim_start();
    }
    groups
}

/// The classes and packages a file asks TeX for, as the files TeX looks
/// for: `\documentclass{article}` wants `article.cls`, `\usepackage{a,b}`
/// wants `a.sty` and `b.sty`, and Beamer's `\usetheme{x}` wants
/// `beamerthemex.sty`.
pub fn wants(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in uncommented(text) {
        for (command, made) in [
            ("\\documentclass", "{}.cls"),
            ("\\usepackage", "{}.sty"),
            ("\\RequirePackage", "{}.sty"),
            ("\\usetheme", "beamertheme{}.sty"),
        ] {
            let mut rest = line.as_str();
            while let Some(at) = rest.find(command) {
                let after = &rest[at + command.len()..];
                rest = after;
                if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    continue;
                }
                // Past its options, `[margin=1in]`.
                let after = after.trim_start();
                let after = match after.strip_prefix('[') {
                    Some(options) => options.split_once(']').map_or("", |(_, r)| r),
                    None => after,
                };
                for name in braced(after, 1)
                    .first()
                    .map(|names| names.split(',').map(str::trim).collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    let file = made.replace("{}", name);
                    if !name.is_empty() && !out.contains(&file) {
                        out.push(file);
                    }
                }
            }
        }
    }
    out
}

/// The `.bib` files a document draws its references from, as written:
/// `\\bibliography{refs,more}` (BibTeX) or `\\addbibresource{refs.bib}`
/// (biblatex), each with `.bib` when it has no extension.
pub fn bibliographies(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in uncommented(text) {
        for command in ["\\bibliography", "\\addbibresource"] {
            let mut rest = line.as_str();
            while let Some(at) = rest.find(command) {
                let after = &rest[at + command.len()..];
                rest = after;
                if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                    continue;
                }
                let after = after.trim_start();
                let after = match after.strip_prefix('[') {
                    Some(options) => options.split_once(']').map_or("", |(_, r)| r),
                    None => after,
                };
                for name in braced(after, 1)
                    .first()
                    .map(|names| {
                        names
                            .split(',')
                            .map(str::trim)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                {
                    if name.is_empty() {
                        continue;
                    }
                    let file = if name.ends_with(".bib") {
                        name
                    } else {
                        format!("{name}.bib")
                    };
                    if !out.contains(&file) {
                        out.push(file);
                    }
                }
            }
        }
    }
    out
}

/// What the preamble needs: XeLaTeX for system fonts and polyglossia,
/// LuaLaTeX for Lua; else nothing.
pub fn engine_needed(text: &str) -> Option<&'static str> {
    let preamble: String = uncommented(text)
        .take_while(|l| !l.contains("\\begin{document}"))
        .collect::<Vec<_>>()
        .join("\n");
    if ["\\directlua", "luacode", "luatexja", "\\luaexec"]
        .iter()
        .any(|w| preamble.contains(w))
    {
        return Some("lualatex");
    }
    if ["fontspec", "unicode-math", "polyglossia", "xeCJK"]
        .iter()
        .any(|w| preamble.contains(w))
    {
        return Some("xelatex");
    }
    None
}

/// The engine a main file is typeset with, by name: its `% !TEX program`,
/// else `setting`, else what it needs, else pdfLaTeX.
pub fn engine(main_text: &str, setting: Option<&str>) -> String {
    magic(main_text)
        .program
        .or_else(|| setting.map(str::to_string))
        .or_else(|| engine_needed(main_text).map(str::to_string))
        .unwrap_or_else(|| "pdflatex".into())
}

/// A project's `.tex` files, its top-level ones first, as many as are
/// looked at: hidden folders and build folders are passed over.
fn tex_files(root: &Path) -> Vec<PathBuf> {
    let mut top = Vec::new();
    let mut deeper = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if top.len() + deeper.len() >= LOOKED {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.')
                || ["node_modules", "target", "build", "out"].contains(&name.as_str())
            {
                continue;
            }
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 6 {
                    stack.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| e == "tex") {
                if depth == 0 {
                    top.push(path);
                } else {
                    deeper.push(path);
                }
            }
        }
    }
    top.extend(deeper);
    top
}

/// A path with its `.` and `..` worked out, without asking the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `main`'s includes, followed, reach `target`.
fn reaches(main: &Path, target: &Path) -> bool {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut queue = vec![main.to_path_buf()];
    while let Some(file) = queue.pop() {
        if seen.len() > 200 || seen.contains(&file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        seen.push(file.clone());
        let dir = file.parent().unwrap_or(Path::new(""));
        for name in includes(&text) {
            // TeX resolves an \input against where it runs: the main file's folder.
            for candidate in [
                lexical(&main.parent().unwrap_or(Path::new("")).join(&name)),
                lexical(&dir.join(&name)),
            ] {
                if candidate == target {
                    return true;
                }
                if candidate.is_file() {
                    queue.push(candidate);
                }
            }
        }
    }
    false
}

/// The main file of the project at `root`, for the file being edited,
/// `active`, and the one pinned, if any.
pub fn find_main(root: &Path, active: Option<&Path>, pinned: Option<&Path>) -> Option<PathBuf> {
    if let Some(pinned) = pinned
        .map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        })
        .filter(|p| p.is_file())
    {
        return Some(pinned);
    }
    if let Some(active) = active.filter(|a| a.is_file() && a.starts_with(root)) {
        let text = std::fs::read_to_string(active).unwrap_or_default();
        if let Some(named) = magic(&text).root {
            let path = lexical(&active.parent().unwrap_or(root).join(named));
            if path.is_file() {
                return Some(path);
            }
        }
        if active.extension().is_some_and(|e| e == "tex") && is_main(&text) {
            return Some(active.to_path_buf());
        }
    }
    let files = tex_files(root);
    let top: Vec<&PathBuf> = files.iter().filter(|f| f.parent() == Some(root)).collect();
    let mains: Vec<&PathBuf> = top
        .iter()
        .copied()
        .filter(|f| std::fs::read_to_string(f).is_ok_and(|t| is_main(&t)))
        .collect();
    for name in CONVENTIONAL {
        if let Some(found) = mains
            .iter()
            .find(|f| f.file_stem().is_some_and(|s| s == *name))
        {
            return Some(found.to_path_buf());
        }
    }
    if let Some(active) = active {
        if let Some(found) = mains.iter().find(|m| reaches(m, active)) {
            return Some(found.to_path_buf());
        }
    }
    match mains.as_slice() {
        [only] => Some(only.to_path_buf()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("quark-tex-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    const DOC: &str =
        "\\documentclass{article}\n\\begin{document}\n\\input{chapters/intro}\n\\end{document}\n";

    #[test]
    fn magic_lines_name_the_root_and_the_program() {
        let text = "% !TEX root = ../main.tex\n%!TeX program = XeLaTeX\n\\section{x}\n% !TEX root = ignored.tex\n";
        assert_eq!(
            magic(text),
            Magic {
                root: Some("../main.tex".into()),
                program: Some("xelatex".into())
            }
        );
        assert_eq!(
            magic("\\documentclass{article}\n% !TEX root = x.tex\n"),
            Magic::default(),
            "only at the top"
        );
    }

    #[test]
    fn includes_are_read_and_commented_ones_and_other_commands_are_not() {
        let text = "\\input{intro}\n% \\input{old}\n\\include{chapters/two.tex}\\includegraphics{fig}\n\\import{parts/}{three}\n\\inputencoding{utf8}\n100\\% done \\subfile{four}";
        assert_eq!(
            includes(text),
            [
                "intro.tex",
                "chapters/two.tex",
                "parts/three.tex",
                "four.tex"
            ]
        );
    }

    #[test]
    fn the_engine_is_the_magic_lines_the_settings_or_what_the_preamble_needs() {
        assert_eq!(
            engine(
                "% !TEX program = lualatex\n\\documentclass{article}",
                Some("xelatex")
            ),
            "lualatex"
        );
        assert_eq!(
            engine("\\documentclass{article}", Some("xelatex")),
            "xelatex"
        );
        assert_eq!(
            engine(
                "\\documentclass{article}\n\\usepackage{fontspec}\n\\begin{document}",
                None
            ),
            "xelatex"
        );
        assert_eq!(
            engine(
                "\\documentclass{article}\n\\begin{document}\nfontspec is a package",
                None
            ),
            "pdflatex",
            "only the preamble counts"
        );
        assert_eq!(
            engine("\\documentclass{article}\n\\usepackage{luacode}", None),
            "lualatex"
        );
    }

    #[test]
    fn the_main_file_is_found_from_the_one_being_edited() {
        let root = project(
            "main",
            &[
                ("thesis.tex", DOC),
                ("chapters/intro.tex", "\\section{Intro}"),
                ("notes.tex", "scratch"),
            ],
        );
        let intro = root.join("chapters/intro.tex");
        assert_eq!(
            find_main(&root, Some(&intro), None),
            Some(root.join("thesis.tex")),
            "by its conventional name"
        );
        // Pinned wins; a magic root wins over the rest.
        assert_eq!(
            find_main(&root, Some(&intro), Some(Path::new("notes.tex"))),
            Some(root.join("notes.tex"))
        );
        std::fs::write(root.join("other.tex"), DOC).unwrap();
        std::fs::write(&intro, "% !TEX root = ../other.tex\n\\section{Intro}").unwrap();
        assert_eq!(
            find_main(&root, Some(&intro), None),
            Some(root.join("other.tex"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn without_a_conventional_name_the_main_file_is_the_one_that_includes_it_or_the_only_one() {
        let root = project(
            "graph",
            &[
                (
                    "a.tex",
                    "\\documentclass{article}\\begin{document}\\input{parts/x}\\end{document}",
                ),
                ("b.tex", "\\documentclass{book}"),
                ("parts/x.tex", "x"),
                ("parts/y.tex", "y"),
            ],
        );
        assert_eq!(
            find_main(&root, Some(&root.join("parts/x.tex")), None),
            Some(root.join("a.tex")),
            "the one whose includes reach it"
        );
        assert_eq!(
            find_main(&root, Some(&root.join("parts/y.tex")), None),
            None,
            "two documents, and neither reaches it"
        );
        let single = project("single", &[("essay.tex", DOC), ("chapters/intro.tex", "x")]);
        assert_eq!(
            find_main(&single, None, None),
            Some(single.join("essay.tex"))
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(single);
    }

    #[test]
    fn the_bibliographies_a_document_uses_are_found_for_bibtex_and_biblatex() {
        let text = "\\bibliographystyle{plain}\n\\bibliography{references, more}\n% \\bibliography{old}\n\\addbibresource[location=local]{extra.bib}\n";
        assert_eq!(
            bibliographies(text),
            ["references.bib", "more.bib", "extra.bib"]
        );
    }
}
