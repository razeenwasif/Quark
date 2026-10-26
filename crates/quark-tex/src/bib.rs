//! BibTeX: a project's `.bib` files read — every entry, where it is, and
//! what its fields say — and new entries written the way a person would
//! write them, with keys of the form `knuth1984tex`.
//!
//! Reading is tolerant, because `.bib` files are written by hand and by a
//! dozen tools, and one bad entry must not lose the rest: an entry that does
//! not close is read as far as it goes, `@comment` and `@preamble` are
//! passed over, text between entries is a comment, `@string` macros are
//! expanded and `#` joins the parts of a value.

use serde::{Deserialize, Serialize};

/// One entry, as the file has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `article`, `inproceedings` — lower case.
    pub kind: String,
    pub key: String,
    /// Each field's name, lower case, and its value with `@string`s
    /// expanded, parts joined and its outer braces or quotes taken off.
    pub fields: Vec<(String, String)>,
    /// Where it is in the file, in bytes: from its `@` to past its closing
    /// brace.
    pub start: usize,
    pub end: usize,
}

impl Entry {
    /// A field's value, by its name in any case.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The authors' names as written, `and` between them.
    pub fn authors(&self) -> Vec<String> {
        self.get("author")
            .or_else(|| self.get("editor"))
            .map(split_names)
            .unwrap_or_default()
    }
}

/// A `.bib` file read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bib {
    pub entries: Vec<Entry>,
}

impl Bib {
    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }
}

/// The month macros every style defines.
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

struct Reader<'a> {
    text: &'a str,
    at: usize,
    strings: Vec<(String, String)>,
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    /// A name: an entry's kind, a field's, a macro's, or a key.
    fn name(&mut self) -> String {
        let from = self.at;
        while self
            .peek()
            .is_some_and(|c| !c.is_whitespace() && !"{}(),=#\"@".contains(c))
        {
            self.bump();
        }
        self.text[from..self.at].to_string()
    }

    /// A braced group's inside, braces within it counted, from just past
    /// its `{`.
    fn braced(&mut self) -> String {
        let from = self.at;
        let mut depth = 1;
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump();
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return self.text[from..self.at - 1].to_string();
                    }
                }
                _ => {}
            }
        }
        self.text[from..].to_string()
    }

    /// A quoted value's inside, from just past its `"`: a quote within
    /// braces does not end it.
    fn quoted(&mut self) -> String {
        let from = self.at;
        let mut depth = 0;
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump();
                }
                '{' => depth += 1,
                '}' => depth -= 1,
                '"' if depth <= 0 => return self.text[from..self.at - 1].to_string(),
                _ => {}
            }
        }
        self.text[from..].to_string()
    }

    /// A value: its parts, `#` between them, joined.
    fn value(&mut self) -> String {
        let mut out = String::new();
        loop {
            self.skip_space();
            match self.peek() {
                Some('{') => {
                    self.bump();
                    out.push_str(&self.braced());
                }
                Some('"') => {
                    self.bump();
                    out.push_str(&self.quoted());
                }
                Some(c) if c.is_ascii_digit() => {
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.bump();
                    }
                    // Taken back off below: the digits are the value.
                    let digits: String = self.text[..self.at]
                        .chars()
                        .rev()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    out.push_str(&digits);
                }
                Some(c) if !",})".contains(c) => {
                    let name = self.name();
                    if name.is_empty() {
                        self.bump();
                        continue;
                    }
                    let lower = name.to_ascii_lowercase();
                    match self.strings.iter().find(|(n, _)| *n == lower) {
                        Some((_, v)) => out.push_str(v),
                        None => match MONTHS.iter().position(|m| *m == lower) {
                            Some(i) => out.push_str(&(i + 1).to_string()),
                            None => out.push_str(&name),
                        },
                    }
                }
                _ => break,
            }
            self.skip_space();
            if self.peek() == Some('#') {
                self.bump();
            } else {
                break;
            }
        }
        out
    }

    /// The fields of an entry or a `@string`, to its close.
    fn fields(&mut self, close: char) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                None => break,
                Some(c) if c == close => {
                    self.bump();
                    break;
                }
                Some(',') => {
                    self.bump();
                }
                Some('@') => break,
                _ => {
                    let name = self.name().to_ascii_lowercase();
                    self.skip_space();
                    if name.is_empty() || self.peek() != Some('=') {
                        // Not a field: passed over to what might be one.
                        if self.bump().is_none() {
                            break;
                        }
                        continue;
                    }
                    self.bump();
                    let value = self.value();
                    fields.push((name, value));
                }
            }
        }
        fields
    }
}

/// Read a `.bib` file.
pub fn parse(text: &str) -> Bib {
    let mut reader = Reader {
        text,
        at: 0,
        strings: Vec::new(),
    };
    let mut entries = Vec::new();
    while let Some(found) = reader.text[reader.at..].find('@') {
        let start = reader.at + found;
        reader.at = start + 1;
        let kind = reader.name().to_ascii_lowercase();
        reader.skip_space();
        let close = match reader.peek() {
            Some('{') => '}',
            Some('(') => ')',
            _ => continue,
        };
        reader.bump();
        match kind.as_str() {
            "comment" | "preamble" => {
                if close == '}' {
                    reader.braced();
                } else {
                    reader.fields(close);
                }
            }
            "string" => {
                for (name, value) in reader.fields(close) {
                    reader.strings.retain(|(n, _)| *n != name);
                    reader.strings.push((name, value));
                }
            }
            "" => {}
            _ => {
                reader.skip_space();
                let key = reader.name();
                let fields = reader.fields(close);
                entries.push(Entry {
                    kind,
                    key: key.trim().to_string(),
                    fields,
                    start,
                    end: reader.at,
                });
            }
        }
    }
    Bib { entries }
}

/// Names as BibTeX writes them, `and` between: `Knuth, Donald E. and
/// Lamport, Leslie` — and not the `and` inside a brace group.
pub fn split_names(value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    let words: Vec<&str> = value.split_inclusive(char::is_whitespace).collect();
    for word in words {
        if depth == 0 && word.trim() == "and" {
            names.push(current.trim().to_string());
            current.clear();
            continue;
        }
        for c in word.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        current.push_str(word);
    }
    if !current.trim().is_empty() {
        names.push(current.trim().to_string());
    }
    names
        .into_iter()
        .filter(|n| !n.is_empty())
        .map(|n| n.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// A name's family part: `Knuth` of `Knuth, Donald E.` or `Donald E.
/// Knuth`, `van der Berg` of `Jan van der Berg`, or a braced name whole.
pub fn family(name: &str) -> String {
    let name = name.trim();
    if name.starts_with('{') && name.ends_with('}') {
        return name[1..name.len() - 1].to_string();
    }
    if let Some((last, _)) = name.split_once(',') {
        return last.trim().to_string();
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    // The family part starts at the first lower-case particle, else it is
    // the last word.
    let from = words
        .iter()
        .position(|w| w.chars().next().is_some_and(char::is_lowercase))
        .filter(|&i| i > 0)
        .unwrap_or(words.len().saturating_sub(1));
    words[from..].join(" ")
}

/// Text as a value may hold it: TeX's special characters escaped, unless
/// they already are, and mathematics between `$`s left as it is.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut math = false;
    let mut previous = ' ';
    for c in text.chars() {
        match c {
            '$' if previous != '\\' => math = !math,
            '&' | '%' | '#' | '_' if !math && previous != '\\' => out.push('\\'),
            _ => {}
        }
        out.push(c);
        previous = c;
    }
    // A brace that does not close would swallow the rest of the file.
    let opened = out.chars().filter(|c| *c == '{').count();
    let closed = out.chars().filter(|c| *c == '}').count();
    if opened != closed {
        out = out.replace(['{', '}'], "");
    }
    out
}

/// ASCII letters of a word, lower case: `Gödel` → `godel`.
fn plain_word(word: &str) -> String {
    let mut out = String::new();
    for c in word.chars() {
        let folded = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => {
                "a"
            }
            'ç' | 'Ç' | 'č' | 'Č' | 'ć' => "c",
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' | 'ě' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => "i",
            'ñ' | 'Ñ' | 'ń' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => {
                "o"
            }
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => "u",
            'ý' | 'ÿ' | 'Ý' => "y",
            'š' | 'Š' | 'ś' => "s",
            'ž' | 'Ž' | 'ź' | 'ż' => "z",
            'ß' => "ss",
            'ł' | 'Ł' => "l",
            c if c.is_ascii_alphanumeric() => {
                out.push(c.to_ascii_lowercase());
                continue;
            }
            _ => continue,
        };
        out.push_str(folded);
    }
    out
}

/// Words a key passes over in a title.
const SMALL: &[&str] = &[
    "a", "an", "the", "on", "of", "for", "in", "and", "to", "with", "from", "by", "at", "is",
    "are", "towards", "toward", "via", "using",
];

/// A key for an entry: the first author's family name, the year, and the
/// title's first word that says something — `knuth1984texbook` — with `b`,
/// `c` after it when `taken` has it already.
pub fn key_for(authors: &[String], year: Option<&str>, title: &str, taken: &[String]) -> String {
    let family = authors.first().map(|a| family(a)).unwrap_or_default();
    let name = family
        .split_whitespace()
        .last()
        .map(plain_word)
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "anon".into());
    let year: String = year
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_digit)
        .take(4)
        .collect();
    let word = title
        .split(|c: char| c.is_whitespace() || c == '-' || c == ':')
        .map(|w| plain_word(w.trim_start_matches('\\')))
        .find(|w| w.len() > 1 && !SMALL.contains(&w.as_str()))
        .unwrap_or_default();
    let base = format!("{name}{year}{word}");
    if !taken.iter().any(|k| k == &base) {
        return base;
    }
    ('b'..='z')
        .map(|s| format!("{base}{s}"))
        .find(|k| !taken.contains(k))
        .unwrap_or_else(|| format!("{base}{}", taken.len()))
}

/// The order fields are written in; others follow, as given.
const ORDER: &[&str] = &[
    "author",
    "editor",
    "title",
    "booktitle",
    "journal",
    "year",
    "month",
    "volume",
    "number",
    "pages",
    "publisher",
    "school",
    "institution",
    "doi",
    "eprint",
    "archiveprefix",
    "primaryclass",
    "url",
    "note",
];

/// A new entry, as a person would write it: one field to a line, the `=`s
/// lined up, in the usual order, empty fields left out.
pub fn write(kind: &str, key: &str, fields: &[(&str, String)]) -> String {
    let mut fields: Vec<(&str, &String)> = fields
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(n, v)| (*n, v))
        .collect();
    fields.sort_by_key(|(name, _)| ORDER.iter().position(|o| o == name).unwrap_or(ORDER.len()));
    let width = fields.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let mut out = format!("@{kind}{{{key},\n");
    for (name, value) in fields {
        out.push_str(&format!("  {name:<width$} = {{{}}},\n", value.trim()));
    }
    out.push_str("}\n");
    out
}

/// A citation to choose: from the project's own `.bib`, Research's papers,
/// or a search online.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// `bib`, `research` or `online`.
    pub source: String,
    /// What choosing it sends back: the key, for the project's own; the
    /// paper's id, for Research's; the result's place, for one online.
    pub id: String,
    /// The key it is cited by — what it has, or what it would be given.
    pub key: String,
    pub title: String,
    /// The authors, shortened for a line: `Knuth`, `Knuth and Lamport`,
    /// `Knuth et al.`
    pub authors: String,
    pub year: Option<String>,
    /// The journal or proceedings, or where it is on arXiv.
    pub venue: Option<String>,
    /// Where it was found: `references.bib`, `P003 · Why is the sky blue?`,
    /// `arXiv`.
    pub from: Option<String>,
}

/// Authors for a line in a list.
pub fn short_authors(authors: &[String]) -> String {
    let families: Vec<String> = authors
        .iter()
        .map(|a| family(a).replace(['{', '}'], ""))
        .collect();
    match families.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [one, ..] => format!("{one} et al."),
    }
}

/// Text from a `.bib` value as it reads: braces and the commonest accents'
/// commands taken out.
pub fn plain(value: &str) -> String {
    let mut out = value.replace(['{', '}'], "");
    for (from, to) in [
        ("\\&", "&"),
        ("\\%", "%"),
        ("\\_", "_"),
        ("\\#", "#"),
        ("~", " "),
        ("--", "–"),
    ] {
        out = out.replace(from, to);
    }
    // `\TeX` reads as TeX.
    let mut read = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_alphabetic()) {
            continue;
        }
        read.push(c);
    }
    read.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The project's entries that match a query — every word of it, in the
/// key, the title, the authors or the year — as citations.
pub fn search(bib: &Bib, query: &str, limit: usize) -> Vec<Citation> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    bib.entries
        .iter()
        .filter(|e| {
            let hay = format!(
                "{} {} {} {}",
                e.key,
                plain(e.get("title").unwrap_or("")),
                plain(e.get("author").unwrap_or("")),
                e.get("year").unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .take(limit)
        .map(|e| Citation {
            source: "bib".into(),
            id: e.key.clone(),
            key: e.key.clone(),
            title: plain(e.get("title").unwrap_or("")),
            authors: short_authors(&e.authors()),
            year: e.get("year").map(str::to_string),
            venue: e.get("journal").or_else(|| e.get("booktitle")).map(plain),
            from: None,
        })
        .collect()
}

/// A paper as a literature search knows it, to be written as an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    pub title: String,
    /// Full names, as given: `Jane Smith`.
    pub authors: Vec<String>,
    pub year: Option<String>,
    /// The journal or proceedings.
    pub venue: Option<String>,
    pub doi: Option<String>,
    /// `2401.01234`, with no `arXiv:` before it.
    pub arxiv: Option<String>,
    /// arXiv's category: `cs.LG`.
    pub category: Option<String>,
    pub url: Option<String>,
}

/// A record's entry and its key — a key not in `taken`. A paper on arXiv
/// and nowhere else is `@misc` with its eprint, as arXiv itself exports
/// it; one in a journal is `@article`, one in proceedings `@inproceedings`.
pub fn entry_for(record: &Record, taken: &[String]) -> (String, String) {
    let key = key_for(
        &record.authors,
        record.year.as_deref(),
        &record.title,
        taken,
    );
    let venue = record
        .venue
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty() && !v.to_ascii_lowercase().starts_with("arxiv"));
    let proceedings = venue.is_some_and(|v| {
        let v = v.to_ascii_lowercase();
        ["proceedings", "conference", "workshop", "symposium"]
            .iter()
            .any(|w| v.contains(w))
    });
    let kind = match venue {
        Some(_) if proceedings => "inproceedings",
        Some(_) => "article",
        None => "misc",
    };
    let mut fields: Vec<(&str, String)> = vec![
        (
            "author",
            record
                .authors
                .iter()
                .map(|a| escape(a.trim()))
                .collect::<Vec<_>>()
                .join(" and "),
        ),
        ("title", escape(record.title.trim())),
        ("year", record.year.clone().unwrap_or_default()),
    ];
    if let Some(venue) = venue {
        fields.push((
            if proceedings { "booktitle" } else { "journal" },
            escape(venue),
        ));
    }
    if let Some(doi) = &record.doi {
        fields.push((
            "doi",
            doi.trim()
                .trim_start_matches("https://doi.org/")
                .to_string(),
        ));
    }
    if let Some(arxiv) = &record.arxiv {
        fields.push((
            "eprint",
            arxiv.trim().trim_start_matches("arXiv:").to_string(),
        ));
        fields.push(("archiveprefix", "arXiv".into()));
        if let Some(category) = &record.category {
            fields.push(("primaryclass", category.clone()));
        }
    }
    if record.doi.is_none() && record.arxiv.is_none() {
        if let Some(url) = &record.url {
            fields.push(("url", url.clone()));
        }
    }
    (key.clone(), write(kind, &key, &fields))
}

/// The entry of `bib` that is this record already — the same DOI, the
/// same eprint, or the same title — so a paper cited twice is one entry.
pub fn same<'a>(bib: &'a Bib, record: &Record) -> Option<&'a Entry> {
    let words = |t: &str| {
        plain(t)
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    };
    let title = words(&record.title);
    bib.entries.iter().find(|e| {
        let doi = record
            .doi
            .as_deref()
            .zip(e.get("doi"))
            .is_some_and(|(a, b)| {
                a.trim()
                    .trim_start_matches("https://doi.org/")
                    .eq_ignore_ascii_case(b.trim())
            });
        let eprint = record
            .arxiv
            .as_deref()
            .zip(e.get("eprint"))
            .is_some_and(|(a, b)| {
                a.trim_start_matches("arXiv:").split('v').next() == b.split('v').next()
            });
        doi || eprint || (!title.is_empty() && e.get("title").is_some_and(|t| words(t) == title))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = r#"% A comment before anything.
@string{ tug = "TeX Users Group" }
@String(acm = {ACM})

@book{knuth1984,
  author    = {Donald E. Knuth},
  title     = {The {\TeX}book},
  publisher = "Addison-" # "Wesley",
  year      = 1984,
  month     = feb,
}

@comment{ @article{fake, title = {Not an entry}} }

@Article{lamport94,
  Author = {Lamport, Leslie and {Barnes and Noble, Inc.}},
  Title = "A {"}quoted{"} title",
  journal = tug # { Journal},
  YEAR = {1994}
}
@misc{broken,
  title = {Never closed
"#;

    #[test]
    fn a_bib_is_read_with_its_strings_its_joins_its_comments_and_where_each_entry_is() {
        let bib = parse(BIB);
        assert_eq!(
            bib.entries
                .iter()
                .map(|e| e.key.as_str())
                .collect::<Vec<_>>(),
            ["knuth1984", "lamport94", "broken"]
        );
        let knuth = bib.get("knuth1984").unwrap();
        assert_eq!(knuth.kind, "book");
        assert_eq!(knuth.get("title"), Some("The {\\TeX}book"));
        assert_eq!(knuth.get("publisher"), Some("Addison-Wesley"));
        assert_eq!(
            (knuth.get("year"), knuth.get("month")),
            (Some("1984"), Some("2"))
        );
        assert!(
            BIB[knuth.start..knuth.end].starts_with("@book{knuth1984,")
                && BIB[knuth.start..knuth.end].ends_with("month     = feb,\n}")
        );
        let lamport = bib.get("lamport94").unwrap();
        assert_eq!(
            lamport.kind, "article",
            "kinds and names are read in any case"
        );
        assert_eq!(lamport.get("journal"), Some("TeX Users Group Journal"));
        assert_eq!(lamport.get("title"), Some("A {\"}quoted{\"} title"));
        assert_eq!(
            lamport.authors(),
            ["Lamport, Leslie", "{Barnes and Noble, Inc.}"],
            "an `and` in braces is part of a name"
        );
        assert_eq!(
            bib.get("broken").unwrap().get("title"),
            Some("Never closed\n"),
            "an entry that never closes is read as far as it goes"
        );
    }

    #[test]
    fn a_family_name_is_found_however_the_name_is_written() {
        assert_eq!(family("Knuth, Donald E."), "Knuth");
        assert_eq!(family("Donald E. Knuth"), "Knuth");
        assert_eq!(family("Jan van der Berg"), "van der Berg");
        assert_eq!(family("{Barnes and Noble, Inc.}"), "Barnes and Noble, Inc.");
        assert_eq!(
            short_authors(&["A. One".into(), "B. Two".into(), "C. Three".into()]),
            "One et al."
        );
        assert_eq!(
            short_authors(&["A. One".into(), "B. Two".into()]),
            "One and Two"
        );
    }

    #[test]
    fn keys_are_family_year_and_word_and_never_one_already_taken() {
        let authors = vec!["Kurt Gödel".to_string()];
        assert_eq!(
            key_for(
                &authors,
                Some("1931"),
                "On Formally Undecidable Propositions",
                &[]
            ),
            "godel1931formally"
        );
        assert_eq!(
            key_for(
                &authors,
                Some("1931"),
                "On Formally Undecidable Propositions",
                &["godel1931formally".into()]
            ),
            "godel1931formallyb"
        );
        assert_eq!(
            key_for(&[], None, "The $\\lambda$-calculus", &[]),
            "anonlambda"
        );
    }

    #[test]
    fn a_new_entry_is_written_in_order_lined_up_with_its_text_escaped() {
        let text = write(
            "article",
            "smith2020deep",
            &[
                ("title", escape("Deep nets & 100% of $x_1$ data")),
                ("author", "Jane Smith and Ravi Rao".into()),
                ("year", "2020".into()),
                ("doi", "10.1000/xyz_1".into()),
                ("note", String::new()),
            ],
        );
        assert_eq!(text, "@article{smith2020deep,\n  author = {Jane Smith and Ravi Rao},\n  title  = {Deep nets \\& 100\\% of $x_1$ data},\n  year   = {2020},\n  doi    = {10.1000/xyz_1},\n}\n");
        assert_eq!(escape("already \\& fine"), "already \\& fine");
        assert_eq!(escape("an { unclosed"), "an  unclosed");
        let read = parse(&text);
        assert_eq!(
            read.entries[0].get("title"),
            Some("Deep nets \\& 100\\% of $x_1$ data"),
            "and reads back as written"
        );
    }

    #[test]
    fn the_project_s_entries_are_searched_by_every_word() {
        let bib = parse(BIB);
        let found = search(&bib, "lamport 1994", 10);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].key.as_str(),
                found[0].authors.as_str(),
                found[0].venue.as_deref()
            ),
            (
                "lamport94",
                "Lamport and Barnes and Noble, Inc.",
                Some("TeX Users Group Journal")
            )
        );
        assert_eq!(search(&bib, "texbook", 10)[0].title, "The TeXbook");
        assert_eq!(search(&bib, "", 10).len(), 3);
    }

    #[test]
    fn a_record_is_written_as_arxiv_or_a_journal_or_proceedings_would_have_it() {
        let arxiv = Record {
            title: "Attention Is All You Need".into(),
            authors: vec!["Ashish Vaswani".into(), "Noam Shazeer".into()],
            year: Some("2017".into()),
            venue: Some("arXiv preprint".into()),
            arxiv: Some("arXiv:1706.03762".into()),
            category: Some("cs.CL".into()),
            ..Record::default()
        };
        let (key, text) = entry_for(&arxiv, &[]);
        assert_eq!(key, "vaswani2017attention");
        assert_eq!(text, "@misc{vaswani2017attention,\n  author        = {Ashish Vaswani and Noam Shazeer},\n  title         = {Attention Is All You Need},\n  year          = {2017},\n  eprint        = {1706.03762},\n  archiveprefix = {arXiv},\n  primaryclass  = {cs.CL},\n}\n");
        let journal = Record {
            title: "R&D at scale".into(),
            authors: vec!["A. Person".into()],
            year: Some("2021".into()),
            venue: Some("Nature".into()),
            doi: Some("https://doi.org/10.1/x".into()),
            ..Record::default()
        };
        let (_, text) = entry_for(&journal, &[]);
        assert!(
            text.starts_with("@article{person2021rd,")
                && text.contains("journal = {Nature}")
                && text.contains("doi     = {10.1/x}")
                && text.contains("R\\&D"),
            "{text}"
        );
        let talk = Record {
            title: "Fast things".into(),
            venue: Some("Proceedings of the 40th ICML".into()),
            ..Record::default()
        };
        assert!(
            entry_for(&talk, &[])
                .1
                .starts_with("@inproceedings{anonfast,")
                && entry_for(&talk, &[])
                    .1
                    .contains("booktitle = {Proceedings of the 40th ICML}")
        );

        let bib = parse(&format!(
            "{}\n@article{{other, title = {{Attention is all you need}}}}",
            entry_for(&journal, &[]).1
        ));
        assert_eq!(
            same(&bib, &journal).map(|e| e.key.as_str()),
            Some("person2021rd"),
            "by DOI"
        );
        assert_eq!(
            same(&bib, &arxiv).map(|e| e.key.as_str()),
            Some("other"),
            "by title, however it is capitalised"
        );
        assert!(same(&bib, &talk).is_none());
    }
}
