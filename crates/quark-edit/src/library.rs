//! What a book on the library's shelves is: its title, author and pages, read
//! from its file without loading it whole — the title cleaned of what
//! downloads and word processors leave in one, and taken from the file's name
//! when it says none.

use std::path::Path;

/// A book, as its shelf shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    pub title: String,
    pub author: String,
    pub pages: u32,
}

/// What a PDF says it is; its name's words where it says nothing usable, and
/// no pages when it cannot be read at all.
pub fn about(path: &Path) -> About {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (named, by) = from_name(&stem);
    let meta = crate::guarded("a book", || {
        lopdf::Document::load_metadata(path).map_err(|e| e.to_string())
    })
    .ok();
    let title = meta
        .as_ref()
        .and_then(|m| m.title.as_deref())
        .and_then(clean_title)
        .unwrap_or(named);
    let author = meta
        .as_ref()
        .and_then(|m| m.author.as_deref())
        .and_then(clean_author)
        .or(by)
        .unwrap_or_default();
    About {
        title,
        author,
        pages: meta.map_or(0, |m| m.page_count),
    }
}

/// What a download site or a print queue adds to a title, at its end.
fn junk_tail(text: &str) -> &str {
    let mut text = text.trim();
    loop {
        let Some(open) = text.rfind('(').filter(|_| text.ends_with(')')) else {
            return text;
        };
        let inner = text[open + 1..text.len() - 1].to_ascii_lowercase();
        let junk = inner.contains("z-lib")
            || inner.contains("libgen")
            || inner.contains("download")
            || inner.contains("ebook")
            || inner == "pdf";
        if !junk {
            return text;
        }
        text = text[..open].trim_end();
    }
}

/// A title worth showing, from what a PDF's Info says.
fn clean_title(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_matches('\0');
    let raw = raw
        .strip_prefix("Microsoft Word - ")
        .or_else(|| raw.strip_prefix("Microsoft PowerPoint - "))
        .unwrap_or(raw);
    let title = junk_tail(raw);
    let lower = title.to_ascii_lowercase();
    let file = [
        ".pdf", ".doc", ".docx", ".tex", ".dvi", ".ps", ".indd", ".qxd",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext));
    let empty = matches!(
        lower.as_str(),
        "" | "untitled" | "title" | "document" | "no title" | "(anonymous)"
    );
    (!file
        && !empty
        && title.chars().filter(|c| c.is_alphabetic()).count() >= 2
        && title.len() <= 300)
        .then(|| title.to_string())
}

/// An author worth showing, from what a PDF's Info says: not a tool's or a
/// publisher's account.
fn clean_author(raw: &str) -> Option<String> {
    let author = raw.trim().trim_matches('\0').replace("(Author)", "");
    let author = author.trim().trim_end_matches([';', ',']).trim();
    let lower = author.to_ascii_lowercase();
    let account = matches!(
        lower.as_str(),
        "" | "nobody"
            | "unknown"
            | "admin"
            | "administrator"
            | "user"
            | "owner"
            | "author"
            | "oreilly"
            | "o'reilly"
            | "pearson"
            | "springer"
            | "wiley"
    );
    (!account && author.chars().filter(|c| c.is_alphabetic()).count() >= 3)
        .then(|| author.to_string())
}

/// A title, and an author when it says `… by Someone`, from a file's name:
/// `foundations-of-computer-vision` reads `Foundations of Computer Vision`.
pub fn from_name(stem: &str) -> (String, Option<String>) {
    let stem = junk_tail(stem);
    // Words joined by `-` or `_`, or run together in camel case.
    let spaced = if stem.contains(' ') {
        stem.replace('_', " ")
    } else {
        stem.replace(['-', '_'], " ")
    };
    let mut words = String::new();
    let mut last: Option<char> = None;
    for c in spaced.chars() {
        if c.is_uppercase() && last.is_some_and(|l| l.is_lowercase()) {
            words.push(' ');
        }
        words.push(c);
        last = Some(c);
    }
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let (title, author) = match words.split_once(" by ") {
        Some((title, by)) if !title.is_empty() && by.split_whitespace().count() >= 2 => {
            (title.to_string(), Some(by.trim().to_string()))
        }
        _ => (words, None),
    };
    // All lower case: a title's case, its small words left small.
    let title = match title.chars().any(char::is_uppercase) {
        true => title,
        false => title
            .split(' ')
            .enumerate()
            .map(|(i, w)| {
                let small = matches!(
                    w,
                    "a" | "an"
                        | "and"
                        | "as"
                        | "at"
                        | "for"
                        | "in"
                        | "of"
                        | "on"
                        | "or"
                        | "the"
                        | "to"
                        | "with"
                );
                if i > 0 && small {
                    w.to_string()
                } else {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| f.to_uppercase().chain(c).collect())
                        .unwrap_or_default()
                }
            })
            .collect::<Vec<String>>()
            .join(" "),
    };
    (title, author)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_cleaned_of_what_a_download_or_a_word_processor_left_in_it() {
        assert_eq!(
            clean_title("Computer Networks (2-Downloads)").as_deref(),
            Some("Computer Networks")
        );
        assert_eq!(clean_title("Microsoft Word - Thesis final.docx"), None);
        assert_eq!(
            clean_title("Microsoft Word - Notes on Queues").as_deref(),
            Some("Notes on Queues")
        );
        for junk in ["", "untitled", "Title", "lecture3.pdf", "  ", "12"] {
            assert_eq!(clean_title(junk), None, "{junk:?}");
        }
        assert_eq!(
            clean_author("Chapman, Barbara(Author)").as_deref(),
            Some("Chapman, Barbara")
        );
        assert_eq!(
            clean_author("Bryant, Randal; O’Hallaron, David; ").as_deref(),
            Some("Bryant, Randal; O’Hallaron, David")
        );
        for account in ["nobody", "oreilly", "Admin", ""] {
            assert_eq!(clean_author(account), None, "{account:?}");
        }
    }

    #[test]
    fn a_files_name_reads_as_a_title_and_says_who_wrote_it_when_it_does() {
        assert_eq!(
            from_name("foundations-of-computer-vision"),
            ("Foundations of Computer Vision".into(), None)
        );
        assert_eq!(
            from_name("speech and language processing").0,
            "Speech and Language Processing"
        );
        assert_eq!(from_name("EijkhoutIntroToHPC").0, "Eijkhout Intro To HPC");
        assert_eq!(
            from_name("Convex_Analysis_Short_View").0,
            "Convex Analysis Short View"
        );
        assert_eq!(
            from_name(
                "Deep Learning by Ian Goodfellow, Yoshua Bengio, Aaron Courville (z-lib.org)"
            ),
            (
                "Deep Learning".into(),
                Some("Ian Goodfellow, Yoshua Bengio, Aaron Courville".into())
            )
        );
        assert_eq!(
            from_name("Computer Vision Algorithms and Applications").0,
            "Computer Vision Algorithms and Applications",
            "a title's own case is kept"
        );
    }

    #[test]
    fn a_book_says_what_its_info_says_and_its_name_says_the_rest() {
        let dir = std::env::temp_dir().join(format!("quark-edit-about-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let named = dir.join("graph-algorithms-in-practice.pdf");
        std::fs::write(&named, crate::fixture::fixture(&["One", "Two", "Three"])).unwrap();
        assert_eq!(
            about(&named),
            About {
                title: "Graph Algorithms in Practice".into(),
                author: String::new(),
                pages: 3
            }
        );
        let mut doc = lopdf::Document::load_mem(&crate::fixture::fixture(&["One"])).unwrap();
        crate::clean::set_info(
            &mut doc,
            "The Art of Queues",
            "Ada Lovelace",
            "",
            "",
            "D:20261012",
        )
        .unwrap();
        let titled = dir.join("aoq.pdf");
        doc.save(&titled).unwrap();
        assert_eq!(
            about(&titled),
            About {
                title: "The Art of Queues".into(),
                author: "Ada Lovelace".into(),
                pages: 1
            }
        );
        let broken = dir.join("not-really.pdf");
        std::fs::write(&broken, b"%PDF-1.7 nothing").unwrap();
        assert_eq!(
            about(&broken),
            About {
                title: "Not Really".into(),
                author: String::new(),
                pages: 0
            }
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
