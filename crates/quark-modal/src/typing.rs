//! What typing adds beyond the key pressed: a level of indentation as wide
//! as the file's own, the partner of a bracket or a quote, and a new line
//! indented to where the code it starts belongs.
//!
//! Pure rules over the characters around the cursor, so each can be tested
//! without an editor. The workspace and the editor decide when they apply.

use std::path::{Path, PathBuf};

/// One level of indentation, and how wide a tab is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indent {
    /// Whether a level is a tab rather than spaces.
    pub tabs: bool,
    /// How many columns a level is.
    pub width: usize,
    /// How many columns a tab is drawn as.
    pub tab: usize,
}

impl Indent {
    /// What one level is, typed.
    pub fn unit(&self) -> String {
        match self.tabs {
            true => "\t".into(),
            false => " ".repeat(self.width),
        }
    }
}

/// How a file is indented: what its `.editorconfig` says (`config`), then
/// what its own lines show, then `fallback` columns of spaces — the
/// `editor.tabSize` setting.
pub fn indent_of(config: Config, name: &str, lines: &[String], fallback: usize) -> Indent {
    let (seen_tabs, seen_width) = seen(name, lines);
    let tabs = config.tabs.unwrap_or(seen_tabs);
    let tab = config.tab.or(config.width).unwrap_or(fallback).clamp(1, 16);
    let width = match tabs {
        true => tab,
        false => config.width.or(seen_width).unwrap_or(fallback).clamp(1, 16),
    };
    Indent { tabs, width, tab }
}

/// Whether a file's lines are indented with tabs, and how many spaces a level
/// is when they are not.
///
/// Tabs where more indented lines begin with one than with a space, and in a
/// Makefile, which must use them, or a Go file with no indentation to go on,
/// which gofmt will give them. The width is the step by which one line's
/// indentation most often exceeds the line before it — two to eight columns,
/// so the single space before a block comment's `*` says nothing — and
/// nothing when no line steps in.
pub fn seen(name: &str, lines: &[String]) -> (bool, Option<usize>) {
    let make = matches!(name, "Makefile" | "makefile" | "GNUmakefile") || name.ends_with(".mk");
    let (mut tabs, mut spaces) = (0, 0);
    let mut steps = [0usize; 9];
    let mut last: Option<usize> = None;
    for line in lines.iter().take(1000) {
        if line.trim().is_empty() {
            continue;
        }
        match line.chars().next() {
            Some('\t') => {
                tabs += 1;
                last = None;
                continue;
            }
            Some(' ') => spaces += 1,
            _ => {}
        }
        let lead = line.chars().take_while(|c| *c == ' ').count();
        if let Some(before) = last {
            if (2..=8).contains(&lead.saturating_sub(before)) && lead > before {
                steps[lead - before] += 1;
            }
        }
        last = Some(lead);
    }
    let tabbed = make || tabs > spaces || (tabs == 0 && spaces == 0 && name.ends_with(".go"));
    // The commonest step, and between equals the more usual width.
    let width = [4, 2, 8, 3, 6, 5, 7]
        .into_iter()
        .filter(|w| steps[*w] > 0)
        .max_by_key(|w| (steps[*w], std::cmp::Reverse([4, 2, 8, 3, 6, 5, 7].iter().position(|x| x == w))));
    (tabbed, width)
}

/// What `.editorconfig` says about a file's indentation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    pub tabs: Option<bool>,
    pub width: Option<usize>,
    pub tab: Option<usize>,
}

/// What the `.editorconfig` files in the folders above `file` say about it:
/// the nearest first, and none above one that says `root = true`.
pub fn editorconfig(file: &Path) -> Config {
    let mut config = Config::default();
    let mut dir = file.parent();
    while let Some(here) = dir {
        if let Ok(text) = std::fs::read_to_string(here.join(".editorconfig")) {
            let relative = file.strip_prefix(here).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from(file));
            let (found, root) = read_editorconfig(&text, &relative);
            // Nearer files win: what one closer already said stands.
            config.tabs = config.tabs.or(found.tabs);
            config.width = config.width.or(found.width);
            config.tab = config.tab.or(found.tab);
            if root {
                break;
            }
        }
        dir = here.parent();
    }
    config
}

/// What one `.editorconfig` says about the file at `relative` to its folder,
/// and whether it is the root, above which none is read. Later sections win
/// over earlier ones, as the format says.
pub fn read_editorconfig(text: &str, relative: &Path) -> (Config, bool) {
    let path: String = relative.to_string_lossy().replace('\\', "/");
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let mut config = Config::default();
    let mut root = false;
    // `None` before the first section, where only `root` may be said.
    let mut applies: Option<bool> = None;
    let mut size_is_tab = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(glob) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            applies = Some(match glob.contains('/') {
                true => globbed(glob.trim_start_matches('/'), &path),
                false => globbed(glob, &name),
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_ascii_lowercase());
        match applies {
            None if key == "root" => root = value == "true",
            Some(true) => match key.as_str() {
                "indent_style" => {
                    config.tabs = match value.as_str() {
                        "tab" => Some(true),
                        "space" => Some(false),
                        _ => None,
                    }
                }
                "indent_size" => {
                    size_is_tab = value == "tab";
                    config.width = value.parse().ok().filter(|n| (1..=16).contains(n));
                }
                "tab_width" => config.tab = value.parse().ok().filter(|n| (1..=16).contains(n)),
                _ => {}
            },
            _ => {}
        }
    }
    if size_is_tab {
        config.width = config.tab;
    }
    (config, root)
}

/// Whether an EditorConfig glob matches `text`: `*` within a folder, `**`
/// across them, `?`, `[abc]` and `[!abc]`, and `{one,two}`.
pub fn globbed(glob: &str, text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    expanded(glob).iter().any(|one| {
        let pattern: Vec<char> = one.chars().collect();
        matches(&pattern, &text)
    })
}

/// A glob with its `{one,two}` choices made, every way.
fn expanded(glob: &str) -> Vec<String> {
    let chars: Vec<char> = glob.chars().collect();
    let Some(open) = chars.iter().position(|c| *c == '{') else { return vec![glob.to_string()] };
    let mut depth = 0;
    let mut close = None;
    let mut commas = Vec::new();
    for (i, c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            ',' if depth == 1 => commas.push(i),
            _ => {}
        }
    }
    let Some(close) = close.filter(|_| !commas.is_empty()) else { return vec![glob.to_string()] };
    let head: String = chars[..open].iter().collect();
    let tail: String = chars[close + 1..].iter().collect();
    let mut bounds = vec![open];
    bounds.extend(&commas);
    bounds.push(close);
    bounds
        .windows(2)
        .flat_map(|w| {
            let choice: String = chars[w[0] + 1..w[1]].iter().collect();
            expanded(&format!("{head}{choice}{tail}"))
        })
        .collect()
}

fn matches(pattern: &[char], text: &[char]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some('*') if pattern.get(1) == Some(&'*') => (0..=text.len()).any(|i| matches(&pattern[2..], &text[i..])),
        Some('*') => {
            for i in 0..=text.len() {
                if matches(&pattern[1..], &text[i..]) {
                    return true;
                }
                if text.get(i) == Some(&'/') {
                    return false;
                }
            }
            false
        }
        Some('?') => text.first().is_some_and(|c| *c != '/') && matches(&pattern[1..], &text[1..]),
        Some('[') => {
            let Some(end) = pattern.iter().skip(1).position(|c| *c == ']').map(|i| i + 1) else {
                return text.first() == Some(&'[') && matches(&pattern[1..], &text[1..]);
            };
            let Some(c) = text.first() else { return false };
            let (negated, set) = match pattern.get(1) {
                Some('!') => (true, &pattern[2..end]),
                _ => (false, &pattern[1..end]),
            };
            set.contains(c) != negated && matches(&pattern[end + 1..], &text[1..])
        }
        Some('\\') if pattern.len() > 1 => text.first() == Some(&pattern[1]) && matches(&pattern[2..], &text[1..]),
        Some(c) => text.first() == Some(c) && matches(&pattern[1..], &text[1..]),
    }
}

/// What a typed character should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// Put this in, and the cursor after its first character.
    Insert(String),
    /// Step over the character after the cursor, which is the one typed.
    Over,
}

/// The bracket that closes `open`.
pub fn partner(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' | '\'' | '`' => Some(open),
        _ => None,
    }
}

/// Whether a quote is closed as it is typed in a file with this extension.
/// Not a `'` where it begins a lifetime, a quoted form or a name, nor a
/// backtick where one quotes a name or opens a TeX quotation.
fn closes_quote(quote: char, extension: &str) -> bool {
    match quote {
        '\'' => !matches!(extension, "rs" | "ml" | "mli" | "hs" | "lean" | "el" | "lisp" | "clj" | "cljs" | "scm" | "rkt"),
        '`' => !matches!(extension, "lean" | "tex" | "el" | "lisp" | "clj" | "cljs" | "scm" | "rkt" | "ml" | "mli"),
        _ => true,
    }
}

/// What typing `c` between `before` and `after` should do.
///
/// A closing bracket or quote steps over the same one after the cursor. An
/// opening bracket is closed when nothing follows it but space, the end of
/// the line, or punctuation a bracket can come before — not in front of a
/// word, where the partner would have to go somewhere else. A quote is closed
/// the same way, and never straight after a letter or a digit, where it is
/// an apostrophe, a prime or the end of a string.
pub fn typed(c: char, before: Option<char>, after: Option<char>, extension: &str) -> Typed {
    let quote = matches!(c, '"' | '\'' | '`');
    if (quote || matches!(c, ')' | ']' | '}')) && after == Some(c) {
        return Typed::Over;
    }
    let open_after = after.map_or(true, |a| a.is_whitespace() || matches!(a, ')' | ']' | '}' | ',' | ';' | ':'));
    let Some(close) = partner(c) else { return Typed::Insert(c.to_string()) };
    let closes = match quote {
        false => open_after,
        true => {
            open_after
                && closes_quote(c, extension)
                && !before.is_some_and(|b| b.is_alphanumeric() || b == '_' || b == c || b == '\\')
        }
    };
    match closes {
        true => Typed::Insert(format!("{c}{close}")),
        false => Typed::Insert(c.to_string()),
    }
}

/// Whether Backspace between these two takes both: an empty pair.
pub fn empty_pair(before: Option<char>, after: Option<char>) -> bool {
    matches!((before, after), (Some(b), Some(a)) if partner(b) == Some(a))
}

/// The indentation a line begins with.
pub fn lead(line: &str) -> &str {
    let end = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..end]
}

/// Whether a line opens a block that the next one is inside: it ends with an
/// opening bracket, or in Python with a colon.
pub fn opens(line: &str, extension: &str) -> bool {
    let code = line.trim_end();
    matches!(code.chars().last(), Some('{' | '[' | '('))
        || (matches!(extension, "py" | "pyi") && code.ends_with(':') && !code.trim_start().starts_with('#'))
}

/// What Enter does on a line cut at the cursor into `before` and `after`:
/// how many characters before the cursor and after it to take away, what to
/// put in their place, and how far into that the cursor goes.
///
/// The new line is indented as this one is, a level deeper when this one
/// opens a block; between a bracket and its partner, the partner moves to a
/// line of its own at this one's indentation. A line that was only
/// indentation is left empty rather than holding spaces nobody typed.
pub fn newline(before: &str, after: &str, indent: &Indent, extension: &str, eol: &str) -> (usize, usize, String, usize) {
    let lead = lead(before);
    let blank = before.trim().is_empty();
    let unit = indent.unit();
    let opened = !blank && opens(before, extension);
    let next = after.trim_start().chars().next();
    let between = opened && before.trim_end().chars().last().and_then(partner).is_some_and(|p| Some(p) == next);
    let take_before = if blank { before.chars().count() } else { 0 };
    match (opened, between) {
        (true, true) => {
            let take_after = after.chars().count() - after.trim_start().chars().count();
            let text = format!("{eol}{lead}{unit}{eol}{lead}");
            let cursor = eol.chars().count() + lead.chars().count() + unit.chars().count();
            (take_before, take_after, text, cursor)
        }
        (true, false) => {
            let text = format!("{eol}{lead}{unit}");
            let cursor = text.chars().count();
            (take_before, 0, text, cursor)
        }
        _ => {
            let text = format!("{eol}{lead}");
            let cursor = text.chars().count();
            (take_before, 0, text, cursor)
        }
    }
}

/// The indentation a closing bracket typed at the start of a line should
/// have: the line before's, if that one opens the block the bracket closes,
/// and otherwise a level less than it. `None` when the line already has no
/// more than that.
pub fn outdented(here: &str, previous: &str, indent: &Indent, extension: &str) -> Option<String> {
    let have = columns(here, indent.tab);
    let prior = lead(previous);
    let target = match opens(previous, extension) {
        true => prior.to_string(),
        false => less(prior, indent),
    };
    (columns(&target, indent.tab) < have).then_some(target)
}

/// One level less of `lead`.
fn less(lead: &str, indent: &Indent) -> String {
    let mut out = lead.to_string();
    if out.ends_with('\t') {
        out.pop();
        return out;
    }
    let spaces = out.chars().rev().take_while(|c| *c == ' ').count().min(indent.width);
    out.truncate(out.len() - spaces);
    out
}

/// The line ending a buffer uses at `line`: `\r\n` where that line has one,
/// and on a last line, which has none of its own, what the line before has.
pub fn eol(buffer: &quark_text::Buffer, line: usize) -> &'static str {
    let text = buffer.text();
    for at in [line, line.saturating_sub(1)] {
        if at >= text.len_lines() {
            continue;
        }
        let slice = text.line(at);
        let n = slice.len_chars();
        if n > 0 && slice.char(n - 1) == '\n' {
            return match n > 1 && slice.char(n - 2) == '\r' {
                true => "\r\n",
                false => "\n",
            };
        }
    }
    "\n"
}

/// How many columns a run of indentation fills.
pub fn columns(lead: &str, tab: usize) -> usize {
    lead.chars().fold(0, |col, c| match c {
        '\t' => (col / tab + 1) * tab,
        _ => col + 1,
    })
}

/// How many spaces Backspace takes in indentation made of spaces: back to the
/// previous stop, so a level typed with Tab goes with one press.
pub fn back_to_stop(before: &str, indent: &Indent) -> usize {
    if indent.tabs || before.is_empty() || !before.chars().all(|c| c == ' ') {
        return 1;
    }
    let column = before.chars().count();
    match column % indent.width {
        0 => indent.width,
        n => n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    const FOUR: Indent = Indent { tabs: false, width: 4, tab: 4 };

    #[test]
    fn a_files_indentation_is_read_from_its_lines() {
        let two = lines("function a() {\n  if (b) {\n    c();\n  }\n}\n/**\n * a comment\n */\n");
        assert_eq!(seen("a.js", &two), (false, Some(2)));
        let four = lines("fn a() {\n    if b {\n        c();\n    }\n}\n");
        assert_eq!(seen("a.rs", &four), (false, Some(4)));
        let tabs = lines("int a() {\n\tif (b) {\n\t\tc();\n\t}\n}\n");
        assert_eq!(seen("a.c", &tabs).0, true);
        // Nothing to go on: the setting decides, except where gofmt would.
        assert_eq!(seen("a.py", &lines("x = 1\n")), (false, None));
        assert_eq!(seen("main.go", &lines("package main\n")).0, true);
        assert_eq!(seen("Makefile", &lines("all:\n")).0, true);
        let flat = indent_of(Config::default(), "a.py", &lines("x = 1\n"), 4);
        assert_eq!(flat, Indent { tabs: false, width: 4, tab: 4 });
        let js = indent_of(Config::default(), "a.js", &two, 4);
        assert_eq!((js.width, js.unit()), (2, "  ".to_string()));
    }

    #[test]
    fn editorconfig_sections_apply_by_glob_and_the_last_wins() {
        let text = "root = true\n\n[*]\nindent_style = space\nindent_size = 4\n\n[*.{js,ts}]\nindent_size = 2\n\n[Makefile]\nindent_style = tab\n\n[docs/**.md]\nindent_size = 3\n";
        assert_eq!(read_editorconfig(text, Path::new("src/main.rs")), (Config { tabs: Some(false), width: Some(4), tab: None }, true));
        assert_eq!(read_editorconfig(text, Path::new("web/app.ts")).0.width, Some(2));
        assert_eq!(read_editorconfig(text, Path::new("Makefile")).0.tabs, Some(true));
        assert_eq!(read_editorconfig(text, Path::new("docs/a/b.md")).0.width, Some(3));
        assert_eq!(read_editorconfig(text, Path::new("b.md")).0.width, Some(4));
        let tabbed = "[*.go]\nindent_style = tab\nindent_size = tab\ntab_width = 8\n";
        assert_eq!(read_editorconfig(tabbed, Path::new("a.go")).0, Config { tabs: Some(true), width: Some(8), tab: Some(8) });
        assert!(globbed("*.[ch]", "a.h") && !globbed("*.[!ch]", "a.c") && globbed("lib/**/*.rs", "lib/a/b/c.rs"));
        assert!(!globbed("*.rs", "src/a.rs") && globbed("{a,b{c,d}}.txt", "bd.txt"));
    }

    #[test]
    fn the_nearest_editorconfig_wins_and_root_stops_the_search() {
        let dir = std::env::temp_dir().join(format!("quark-editorconfig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("outer/inner")).unwrap();
        std::fs::write(dir.join(".editorconfig"), "[*]\nindent_style = tab\n").unwrap();
        std::fs::write(dir.join("outer/.editorconfig"), "root = true\n[*]\nindent_size = 4\n").unwrap();
        std::fs::write(dir.join("outer/inner/.editorconfig"), "[*.js]\nindent_size = 2\n").unwrap();
        let config = editorconfig(&dir.join("outer/inner/a.js"));
        assert_eq!(config, Config { tabs: None, width: Some(2), tab: None }, "the tab above the root is never read");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn brackets_and_quotes_close_where_nothing_is_in_the_way() {
        let insert = |s: &str| Typed::Insert(s.to_string());
        assert_eq!(typed('(', Some('f'), None, "rs"), insert("()"));
        assert_eq!(typed('{', Some(' '), Some(')'), "rs"), insert("{}"));
        assert_eq!(typed('(', None, Some('x'), "rs"), insert("("), "not in front of a word");
        assert_eq!(typed(')', Some('('), Some(')'), "rs"), Typed::Over);
        assert_eq!(typed(')', Some('a'), Some('b'), "rs"), insert(")"));
        assert_eq!(typed('"', Some(' '), None, "rs"), insert("\"\""));
        assert_eq!(typed('"', Some('a'), Some('"'), "rs"), Typed::Over);
        assert_eq!(typed('\'', Some('n'), Some(' '), "md"), insert("'"), "don't");
        assert_eq!(typed('\'', Some('&'), None, "rs"), insert("'"), "a lifetime");
        assert_eq!(typed('\'', Some(' '), None, "py"), insert("''"));
        assert_eq!(typed('`', Some(' '), None, "lean"), insert("`"));
        assert!(empty_pair(Some('('), Some(')')) && empty_pair(Some('"'), Some('"')) && !empty_pair(Some('('), Some(']')));
    }

    #[test]
    fn enter_indents_the_new_line_and_opens_a_block_between_brackets() {
        assert_eq!(newline("    let a = 1;", "", &FOUR, "rs", "\n"), (0, 0, "\n    ".into(), 5));
        assert_eq!(newline("fn a() {", "}", &FOUR, "rs", "\n"), (0, 0, "\n    \n".into(), 5));
        assert_eq!(newline("  call(", " )", &FOUR, "js", "\n"), (0, 1, "\n      \n  ".into(), 7));
        assert_eq!(newline("if x:", "", &FOUR, "py", "\r\n"), (0, 0, "\r\n    ".into(), 6));
        assert_eq!(newline("    ", "", &FOUR, "rs", "\n"), (4, 0, "\n    ".into(), 5), "a line of only indentation is left empty");
        let tabs = Indent { tabs: true, width: 4, tab: 4 };
        assert_eq!(newline("\tif (a) {", "}", &tabs, "c", "\n"), (0, 0, "\n\t\t\n\t".into(), 3));
    }

    #[test]
    fn a_closing_bracket_on_a_line_of_indentation_goes_back_a_level() {
        assert_eq!(outdented("        ", "        foo();", &FOUR, "rs"), Some("    ".into()));
        assert_eq!(outdented("    ", "fn a() {", &FOUR, "rs"), Some("".into()));
        assert_eq!(outdented("    ", "    foo();", &FOUR, "rs"), Some("".into()));
        assert_eq!(outdented("", "foo();", &FOUR, "rs"), None);
        assert_eq!(back_to_stop("      ", &FOUR), 2);
        assert_eq!(back_to_stop("        ", &FOUR), 4);
        assert_eq!(back_to_stop("  x ", &FOUR), 1);
    }
}
