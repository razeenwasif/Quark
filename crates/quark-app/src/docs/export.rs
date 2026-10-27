//! A document out of Quark: as Markdown, as a web page, as a Word file, as
//! plain text, or printed.
//!
//! The web page is the one rendering: Word's file is made from it by macOS's
//! own `textutil`, and printing opens it in the browser, whose print dialog
//! also saves a PDF. Comments and suggestions stay behind; they are notes
//! about the document, not part of it.

use std::path::Path;

use quark_doc::{Block, BlockKind, Document, Marks, markdown};

use crate::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Markdown,
    Html,
    Word,
    Text,
    Print,
    CopyMarkdown,
    Reveal,
}

impl Format {
    pub(crate) const ALL: [Format; 7] = [Format::Print, Format::Word, Format::Html, Format::Markdown, Format::Text, Format::CopyMarkdown, Format::Reveal];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Format::Markdown => "Markdown (.md)…",
            Format::Html => "Web page (.html)…",
            Format::Word => "Word document (.docx)…",
            Format::Text => "Plain text (.txt)…",
            Format::Print => "Print or save as PDF…",
            Format::CopyMarkdown => "Copy as Markdown",
            Format::Reveal => if cfg!(target_os = "macos") { "Show in Finder" } else { "Show in folder" },
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html | Format::Print => "html",
            Format::Word => "docx",
            _ => "txt",
        }
    }
}

impl App {
    pub(crate) fn export_document(&mut self, index: usize, format: Format) {
        let Some(tab) = self.docs.tabs.get(index) else { return };
        let title = tab.title();
        let doc = tab.editor.doc.clone();
        let path = tab.path.clone();
        match format {
            // Done where the clipboard is, in the menu itself.
            Format::CopyMarkdown => {}
            Format::Reveal => {
                if !path.exists() {
                    self.toast("Not saved yet — it has no file to show", false);
                } else if let Err(e) = reveal(&path) {
                    self.toast(e, true);
                }
            }
            Format::Print => {
                let file = std::env::temp_dir().join(format!("{}.html", safe_stem(&title)));
                let result = std::fs::write(&file, to_html(&doc, &title, true)).map_err(|e| e.to_string()).and_then(|_| open(&file));
                match result {
                    Ok(()) => self.toast("Opened in your browser — print it, or save it as a PDF", false),
                    Err(e) => self.toast(format!("Could not open it to print: {e}"), true),
                }
            }
            _ => {
                let Some(to) = rfd::FileDialog::new()
                    .set_directory(path.parent().unwrap_or(Path::new(".")))
                    .set_file_name(format!("{title}.{}", format.extension()))
                    .add_filter(format.label().trim_end_matches('…'), &[format.extension()])
                    .save_file()
                else {
                    return;
                };
                match write(&doc, &title, format, &to) {
                    Ok(()) => self.toast(format!("Exported {}", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), false),
                    Err(e) => self.toast(format!("Could not export: {e}"), true),
                }
            }
        }
    }
}

fn write(doc: &Document, title: &str, format: Format, to: &Path) -> Result<(), String> {
    match format {
        Format::Markdown => std::fs::write(to, markdown::write_blocks(&doc.blocks)).map_err(|e| e.to_string()),
        Format::Html => std::fs::write(to, to_html(doc, title, false)).map_err(|e| e.to_string()),
        Format::Text => std::fs::write(to, to_text(doc)).map_err(|e| e.to_string()),
        Format::Word => word(doc, title, to),
        _ => Ok(()),
    }
}

/// Word, by way of the web page and macOS's `textutil`.
fn word(doc: &Document, title: &str, to: &Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Word export uses macOS's textutil; export a web page and open it in Word instead".into());
    }
    let html = std::env::temp_dir().join(format!("{}-{}.html", safe_stem(title), std::process::id()));
    std::fs::write(&html, to_html(doc, title, false)).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("/usr/bin/textutil")
        .args(["-convert", "docx", "-output"])
        .arg(to)
        .arg(&html)
        .output()
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&html);
    let out = out?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

fn safe_stem(title: &str) -> String {
    let s: String = title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    if s.trim().is_empty() { "Document".into() } else { s }
}

fn open(path: &Path) -> Result<(), String> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn reveal(path: &Path) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
    } else {
        open(path.parent().unwrap_or(Path::new(".")))
    }
}

/// The text alone, with list markers and blank lines between paragraphs.
pub(crate) fn to_text(doc: &Document) -> String {
    let mut out = String::new();
    let mut numbers: Vec<u64> = Vec::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        let prev_list = i > 0 && doc.blocks[i - 1].kind.is_list();
        if i > 0 {
            out.push_str(if b.kind.is_list() && prev_list { "\n" } else { "\n\n" });
        }
        let pad = "  ".repeat(b.kind.depth().unwrap_or(0) as usize);
        match &b.kind {
            BlockKind::Bullet { .. } => out.push_str(&format!("{pad}• ")),
            BlockKind::Numbered { depth } => {
                let d = *depth as usize;
                numbers.truncate(d + 1);
                numbers.resize(d + 1, 0);
                numbers[d] += 1;
                out.push_str(&format!("{pad}{}. ", numbers[d]));
            }
            BlockKind::Task { checked, .. } => out.push_str(&format!("{pad}{} ", if *checked { "☑" } else { "☐" })),
            BlockKind::Rule => out.push_str("———"),
            _ => {}
        }
        if !b.kind.is_list() {
            numbers.clear();
        }
        out.push_str(b.text());
    }
    out.push('\n');
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
    }
    out
}

/// A block's text as HTML, its formatting as tags.
fn inline(block: &Block) -> String {
    let mut out = String::new();
    for (_, text, style) in block.runs() {
        let mut open = String::new();
        let mut close = String::new();
        if let Some(url) = &style.link {
            open.push_str(&format!("<a href=\"{}\">", escape(url)));
            close.insert_str(0, "</a>");
        }
        for (mark, tag) in [(Marks::BOLD, "strong"), (Marks::ITALIC, "em"), (Marks::STRIKE, "s"), (Marks::UNDERLINE, "u"), (Marks::CODE, "code")] {
            if style.marks.contains(mark) {
                open.push_str(&format!("<{tag}>"));
                close.insert_str(0, &format!("</{tag}>"));
            }
        }
        out.push_str(&open);
        out.push_str(&escape(text));
        out.push_str(&close);
    }
    out
}

const STYLE: &str = "body{margin:0;background:#f6f5f9;color:#1f1c25;font:16px/1.65 'Manrope',-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif}\
main{max-width:640px;margin:48px auto;padding:72px 88px;background:#fff;border-radius:8px;box-shadow:0 20px 50px rgba(30,20,60,.12)}\
h1{font-size:30px;line-height:1.25;letter-spacing:-.02em;margin:1.4em 0 .4em}h2{font-size:22px;margin:1.3em 0 .3em}h3{font-size:18px;margin:1.2em 0 .3em}\
h1:first-child,h2:first-child,h3:first-child{margin-top:0}p{margin:0 0 .8em}ul,ol{margin:0 0 .8em;padding-left:1.5em}li{margin:.15em 0}\
li.task{list-style:none;margin-left:-1.4em}li.task input{margin-right:.5em}li.done{color:#8a8592;text-decoration:line-through}\
blockquote{margin:0 0 .8em;padding:.6em 1em;background:#f4f2f8;border-radius:10px;color:#4a4652}\
pre{margin:0 0 .8em;padding:.8em 1em;background:#f1eff5;border-radius:10px;overflow:auto;font:13.5px/1.55 'SF Mono',Menlo,Consolas,monospace}\
code{font-family:'SF Mono',Menlo,Consolas,monospace;font-size:.88em;background:#f1eff5;color:#4b3a99;padding:.1em .35em;border-radius:5px}pre code{background:none;padding:0;color:inherit;font-size:1em}\
a{color:#5b3fc4}hr{border:0;border-top:1px solid #ddd9e4;margin:1.6em 0}\
@media print{body{background:#fff}main{margin:0;padding:0;box-shadow:none;max-width:none}@page{margin:2.2cm}}";

/// The document as a standalone web page. `print` opens the print dialog
/// once it has loaded.
pub(crate) fn to_html(doc: &Document, title: &str, print: bool) -> String {
    let mut body = String::new();
    // Open lists, innermost last: whether each is numbered, and whether its
    // last item is still open for a list nested in it.
    let mut lists: Vec<(bool, bool)> = Vec::new();
    let close_to = |lists: &mut Vec<(bool, bool)>, depth: usize, body: &mut String| {
        while lists.len() > depth {
            let (numbered, item_open) = lists.pop().unwrap();
            if item_open {
                body.push_str("</li>");
            }
            body.push_str(if numbered { "</ol>" } else { "</ul>" });
        }
    };
    for block in &doc.blocks {
        let Some(depth) = block.kind.depth().map(usize::from) else {
            close_to(&mut lists, 0, &mut body);
            let text = inline(block);
            match &block.kind {
                BlockKind::Heading(n) => body.push_str(&format!("<h{l}>{text}</h{l}>\n", l = (*n).clamp(1, 6))),
                BlockKind::Quote => body.push_str(&format!("<blockquote>{text}</blockquote>\n")),
                BlockKind::Code { lang } => {
                    let class = if lang.is_empty() { String::new() } else { format!(" class=\"language-{}\"", escape(lang)) };
                    body.push_str(&format!("<pre><code{class}>{}</code></pre>\n", escape(block.text()).replace("<br>", "\n")));
                }
                BlockKind::Rule => body.push_str("<hr>\n"),
                _ if block.is_empty() => body.push_str("<p>&nbsp;</p>\n"),
                _ => body.push_str(&format!("<p>{text}</p>\n")),
            }
            continue;
        };
        let numbered = matches!(block.kind, BlockKind::Numbered { .. });
        let depth = depth.min(lists.len());
        close_to(&mut lists, depth + 1, &mut body);
        if let Some(&(kind, item_open)) = lists.last().filter(|_| lists.len() == depth + 1) {
            if kind != numbered {
                close_to(&mut lists, depth, &mut body);
            } else if item_open {
                body.push_str("</li>");
                if let Some(top) = lists.last_mut() {
                    top.1 = false;
                }
            }
        }
        while lists.len() < depth + 1 {
            body.push_str(if numbered { "<ol>" } else { "<ul>" });
            lists.push((numbered, false));
        }
        let item = match block.kind {
            BlockKind::Task { checked, .. } => format!(
                "<li class=\"task{}\"><input type=\"checkbox\" disabled{}>{}",
                if checked { " done" } else { "" },
                if checked { " checked" } else { "" },
                inline(block)
            ),
            _ => format!("<li>{}", inline(block)),
        };
        body.push_str(&item);
        if let Some(top) = lists.last_mut() {
            top.1 = true;
        }
    }
    close_to(&mut lists, 0, &mut body);
    let script = if print { "<script>addEventListener('load',()=>setTimeout(()=>print(),300))</script>" } else { "" };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n{body}</main>\n{script}</body>\n</html>\n",
        escape(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_carries_the_structure() {
        let doc = markdown::read("# Plan\n\nSome **bold** & <stuff>.\n\n- a\n  - b\n- [x] done\n\n1. one\n\n> said\n\n```\nx < y\n```\n");
        let html = to_html(&doc, "Plan", false);
        assert!(html.contains("<h1>Plan</h1>"));
        assert!(html.contains("<p>Some <strong>bold</strong> &amp; &lt;stuff&gt;.</p>"));
        assert!(html.contains("<ul><li>a<ul><li>b</li></ul></li><li class=\"task done\"><input type=\"checkbox\" disabled checked>done</li></ul>"), "{html}");
        assert!(html.contains("<ol><li>one</li></ol>"));
        assert!(html.contains("<blockquote>said</blockquote>"));
        assert!(html.contains("<pre><code>x &lt; y</code></pre>"));
        assert!(!html.contains("print()"));
    }

    #[test]
    fn plain_text_keeps_list_markers() {
        let doc = markdown::read("Intro\n\n1. a\n2. b\n\n- [ ] c\n");
        assert_eq!(to_text(&doc), "Intro\n\n1. a\n2. b\n☐ c\n");
    }
}
