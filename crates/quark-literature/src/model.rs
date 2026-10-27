//! What the literature crate shares with the rest of Quark: a paper's
//! record as the sources describe it, the sources, how it is named, and a
//! search's candidates. From Quark's Research crate (`model`, `cite`,
//! `literature`, `view`), only what searching and citing need.

use serde::{Deserialize, Serialize};

/// A scholarly source the literature is searched in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LiteratureSource {
    /// Physics, mathematics and computer science preprints, with their PDFs.
    Arxiv,
    /// Every field, with citation counts and who cites whom.
    SemanticScholar,
    /// Everything, with where an open copy of each paper is.
    OpenAlex,
}

impl LiteratureSource {
    pub const ALL: [LiteratureSource; 3] = [LiteratureSource::Arxiv, LiteratureSource::SemanticScholar, LiteratureSource::OpenAlex];

    pub fn name(self) -> &'static str {
        match self {
            LiteratureSource::Arxiv => "arXiv",
            LiteratureSource::SemanticScholar => "Semantic Scholar",
            LiteratureSource::OpenAlex => "OpenAlex",
        }
    }
}

/// Whether a paper's full text can be had freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// An open copy is to be had.
    Open,
    /// Behind a paywall, as far as the sources know.
    Closed,
    #[default]
    Unknown,
}

/// The most authors a paper keeps by name; the rest are counted.
pub const AUTHORS: usize = 50;

/// The longest abstract kept, in characters.
pub const ABSTRACT: usize = 8_000;

/// What the sources say about a paper.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Record {
    pub title: String,
    pub authors: Vec<String>,
    /// The authors past [`AUTHORS`], counted rather than kept.
    #[serde(default)]
    pub authors_more: u32,
    pub year: Option<u32>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    /// Without its version: `1706.03762`.
    pub arxiv: Option<String>,
    /// Semantic Scholar's id for it.
    pub s2: Option<String>,
    /// OpenAlex's: `W2741809807`.
    pub openalex: Option<String>,
    #[serde(rename = "abstract")]
    pub abstract_: String,
    /// Semantic Scholar's one-line summary, when it has one.
    pub tldr: String,
    /// How many papers cite it, as the most generous source counts.
    pub citations: Option<u32>,
    pub access: Access,
    /// Where an open copy of its PDF is.
    pub pdf_url: Option<String>,
    /// A page about it: arXiv's abstract page, a publisher's.
    pub url: Option<String>,
    /// The sources that found it.
    pub sources: Vec<LiteratureSource>,
}

impl Record {
    /// Within what a paper keeps: [`AUTHORS`] by name, [`ABSTRACT`] characters.
    pub fn capped(mut self) -> Record {
        if self.authors.len() > AUTHORS {
            self.authors_more += (self.authors.len() - AUTHORS) as u32;
            self.authors.truncate(AUTHORS);
        }
        if self.abstract_.chars().count() > ABSTRACT {
            self.abstract_ = self.abstract_.chars().take(ABSTRACT).collect::<String>() + "…";
        }
        self
    }
}

/// A paper, as a citation names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ident {
    /// A DOI, without a resolver in front of it, as it was written.
    Doi(String),
    /// An arXiv id without its version: `1706.03762`, `hep-th/9901001`.
    Arxiv(String),
    /// One of the investigation's papers: `P003`.
    Paper(String),
}

impl Ident {
    /// One identifier as somebody typed it: a DOI (`10.…`, `doi:…`, a
    /// doi.org link), an arXiv id (`arXiv:…`, a bare new-style or old-style
    /// id, an arxiv.org link) or a paper of the investigation's.
    pub fn parse(text: &str) -> Option<Ident> {
        let text = text.trim().trim_matches(|c| matches!(c, '<' | '>'));
        if is_paper(text) {
            return Some(Ident::Paper(text.to_string()));
        }
        let lower = text.to_ascii_lowercase();
        if let Some(rest) = strip_url(&lower).and_then(|rest| rest.strip_prefix("doi.org/").or_else(|| rest.strip_prefix("dx.doi.org/"))) {
            let start = text.len() - rest.len();
            return doi(&decode(&text[start..]));
        }
        if let Some(rest) = strip_url(&lower).and_then(arxiv_path) {
            let start = text.len() - rest.len();
            return arxiv(&text[start..]).map(Ident::Arxiv);
        }
        if let Some(rest) = lower.strip_prefix("doi:") {
            return doi(text[text.len() - rest.len()..].trim());
        }
        if let Some(rest) = lower.strip_prefix("arxiv:") {
            return arxiv(text[text.len() - rest.len()..].trim()).map(Ident::Arxiv);
        }
        if text.starts_with("10.") {
            return doi(text);
        }
        arxiv(text).map(Ident::Arxiv)
    }

    /// What two spellings of one paper share: a DOI in lower case, an arXiv
    /// id without its version, a paper's own id.
    pub fn key(&self) -> String {
        match self {
            Ident::Doi(doi) => format!("doi:{}", doi.to_ascii_lowercase()),
            Ident::Arxiv(id) => format!("arxiv:{}", id.to_ascii_lowercase()),
            Ident::Paper(id) => format!("paper:{id}"),
        }
    }

    /// Where it can be read about: doi.org's resolver, arXiv's abstract page.
    pub fn url(&self) -> Option<String> {
        match self {
            Ident::Doi(doi) => Some(format!("https://doi.org/{doi}")),
            Ident::Arxiv(id) => Some(format!("https://arxiv.org/abs/{id}")),
            Ident::Paper(_) => None,
        }
    }

    /// As it is written back: `doi:10.…`, `arXiv:1706.03762`, `P003`.
    pub fn text(&self) -> String {
        match self {
            Ident::Doi(doi) => format!("doi:{doi}"),
            Ident::Arxiv(id) => format!("arXiv:{id}"),
            Ident::Paper(id) => id.clone(),
        }
    }
}

/// A paper a search found: what the sources say about it, the queries that
/// found it, and its best place in any source's answer to one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Candidate {
    pub record: Record,
    pub queries: Vec<String>,
    /// Its best rank in a source's answer, from 0.
    pub rank: u32,
}

/// A title as it is compared: its words, lower case, one space apart.
pub fn normalised(title: &str) -> String {
    title.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

/// Every name a paper goes by: its DOI, its arXiv id, its Semantic Scholar
/// and OpenAlex ids, and its title with its year — the last for a source that
/// gives none of the others.
pub fn keys(record: &Record) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |key: String| {
        if !out.contains(&key) {
            out.push(key);
        }
    };
    // arXiv's own DOIs name the arXiv paper, so a record with both has one key for them.
    if let Some(doi) = record.doi.as_deref().and_then(Ident::parse) {
        add(doi.key());
    }
    if let Some(arxiv) = record.arxiv.as_deref().and_then(|id| Ident::parse(&format!("arXiv:{id}"))) {
        add(arxiv.key());
    }
    if let Some(s2) = record.s2.as_deref().filter(|s| !s.is_empty()) {
        add(format!("s2:{s2}"));
    }
    if let Some(work) = record.openalex.as_deref().filter(|s| !s.is_empty()) {
        add(format!("openalex:{}", work.to_ascii_lowercase()));
    }
    let title = normalised(&record.title);
    if !title.is_empty() {
        add(format!("title:{title}|{}", record.year.unwrap_or(0)));
    }
    out
}

/// Whether a source's key was found: all that is ever said of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyState {
    Found,
    Missing,
    /// Not looked for yet: nothing has been asked.
    NotLooked,
    /// It takes none.
    NotNeeded,
}


/* ── reading an identifier: from Proton's cite.rs ── */

fn is_paper(text: &str) -> bool {
    text.len() >= 4 && text.starts_with('P') && text[1..].bytes().all(|b| b.is_ascii_digit())
}

/// What follows `http(s)://` and `www.`, when there is one.
fn strip_url(lower: &str) -> Option<&str> {
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"))?;
    Some(rest.strip_prefix("www.").unwrap_or(rest))
}

/// What follows an arxiv.org link's `/abs/` or `/pdf/`, anywhere in `lower`'s start.
fn arxiv_path(lower: &str) -> Option<&str> {
    let rest = strip_url(lower).unwrap_or(lower);
    let rest = rest.strip_prefix("export.").unwrap_or(rest);
    rest.strip_prefix("arxiv.org/abs/").or_else(|| rest.strip_prefix("arxiv.org/pdf/"))
}

/// A DOI's text checked: `10.`, a registrant of digits (and dots), a slash,
/// a suffix. arXiv's own DOIs name an arXiv paper.
fn doi(text: &str) -> Option<Ident> {
    let text = text.trim();
    let (registrant, suffix) = text.strip_prefix("10.")?.split_once('/')?;
    let registrant_ok = registrant.len() >= 4 && registrant.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    if !registrant_ok || suffix.is_empty() || suffix.contains(char::is_whitespace) {
        return None;
    }
    if registrant == "48550" {
        if let Some(id) = suffix.strip_prefix("arXiv.").or_else(|| suffix.strip_prefix("ARXIV.")).and_then(arxiv) {
            return Some(Ident::Arxiv(id));
        }
    }
    Some(Ident::Doi(text.to_string()))
}


/// An arXiv id with its version removed: new style `YYMM.NNNNN`, or old
/// style `archive(.SUBJ)/YYMMNNN`. A `.pdf` on the end, from a link, goes too.
fn arxiv(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches(".pdf");
    let id = strip_version(text);
    let new_style = id.split_once('.').is_some_and(|(yymm, n)| {
        yymm.len() == 4 && yymm.bytes().all(|b| b.is_ascii_digit()) && (4..=5).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit())
    });
    let old_style = id.split_once('/').is_some_and(|(archive, n)| {
        let name = archive.split('.').next().unwrap_or("");
        !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
            && archive.split('.').nth(1).is_none_or(|subj| subj.len() == 2 && subj.bytes().all(|b| b.is_ascii_uppercase()))
            && n.len() == 7
            && n.bytes().all(|b| b.is_ascii_digit())
    });
    (new_style || old_style).then(|| id.to_string())
}

fn strip_version(id: &str) -> &str {
    match id.rfind('v') {
        Some(at) if at > 0 && at + 1 < id.len() && id[at + 1..].bytes().all(|b| b.is_ascii_digit()) => &id[..at],
        _ => id,
    }
}


/// `%2F` and its kind decoded, as doi.org links write a DOI's slashes and brackets.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(text.get(i + 1..i + 3).unwrap_or(""), 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
