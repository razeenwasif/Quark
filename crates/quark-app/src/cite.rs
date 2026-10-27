//! Citing: a paper found in the project's own `.bib`, or online, and
//! `\cite{key}` put where the cursor is — the entry added to the document's
//! `.bib` when it is not there yet. From Proton's core (`tex/cite.rs`),
//! without its Research papers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quark_literature::fetch::{Fetch, Job, Reply};
use quark_literature::model::{LiteratureSource, Record};
use quark_literature::{Context, Http, Keys};
use quark_tex::bib::{self, Bib, Citation};

/// How many of each kind a search shows.
const SHOWN: usize = 12;

/// The cite picker's search: what was asked, what was found, and an online
/// search under way.
#[derive(Default)]
pub(crate) struct Cite {
    pub query: String,
    pub local: Vec<Citation>,
    /// What the last online search found, by its place, and as citations.
    online: Vec<Record>,
    pub online_citations: Vec<Citation>,
    fetch: Option<Fetch>,
    pub note: Option<String>,
}

/// The `.bib` files the document at `main` names, beside it — and, if it
/// names none, the `references.bib` a new entry would go in, which it
/// would have to be told about.
pub(crate) fn bib_files(main: &Path) -> (Vec<PathBuf>, bool) {
    let dir = main.parent().unwrap_or(Path::new("."));
    let named = quark_tex::project::bibliographies(&std::fs::read_to_string(main).unwrap_or_default());
    if named.is_empty() {
        return (vec![dir.join("references.bib")], false);
    }
    (named.iter().map(|n| dir.join(n)).collect(), true)
}

/// Each of the document's `.bib` files that is there, read.
fn read_bibs(main: &Path) -> Vec<(PathBuf, Bib)> {
    bib_files(main)
        .0
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, bib::parse(&text))))
        .collect()
}

/// Every key the document's `.bib` files have.
fn taken(bibs: &[(PathBuf, Bib)]) -> Vec<String> {
    bibs.iter().flat_map(|(_, b)| b.entries.iter().map(|e| e.key.clone())).collect()
}

/// A paper the sources found, as BibTeX would record it.
pub(crate) fn record_of(r: &Record) -> bib::Record {
    let mut authors = r.authors.clone();
    if r.authors_more > 0 {
        authors.push("others".into());
    }
    bib::Record {
        title: r.title.clone(),
        authors,
        year: r.year.map(|y| y.to_string()),
        venue: r.venue.clone(),
        doi: r.doi.clone(),
        arxiv: r.arxiv.clone(),
        category: None,
        url: r.url.clone(),
    }
}

/// A record as a citation to choose: cited by the key it has in the
/// document's `.bib`, or the one it would be given.
fn citation(record: &Record, id: String, bibs: &[(PathBuf, Bib)]) -> Citation {
    let as_bib = record_of(record);
    let key = bibs
        .iter()
        .find_map(|(_, b)| bib::same(b, &as_bib).map(|e| e.key.clone()))
        .unwrap_or_else(|| bib::key_for(&as_bib.authors, as_bib.year.as_deref(), &as_bib.title, &taken(bibs)));
    let mut authors = bib::short_authors(&record.authors);
    if record.authors_more > 0 && record.authors.len() == 1 {
        authors.push_str(" et al.");
    }
    Citation {
        source: "online".into(),
        id,
        key,
        title: record.title.clone(),
        authors,
        year: record.year.map(|y| y.to_string()),
        venue: record.venue.clone(),
        from: Some(record.sources.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()),
    }
}

/// The client online searches ask with: keys from Quark's own environment,
/// read when a search first needs them, never kept.
pub(crate) fn context() -> Context {
    let vars: BTreeMap<String, String> =
        quark_literature::keys::NAMES.iter().filter_map(|n| std::env::var(n).ok().map(|v| (n.to_string(), v))).collect();
    let keys = Keys::from_vars(&vars);
    Context::new(Http::live(keys.mailto.as_deref()), move || keys.clone())
}

impl Cite {
    /// Search the document's `.bib` for `query`. An online search under way
    /// for another query is let go.
    pub(crate) fn search(&mut self, main: Option<&Path>, query: &str) {
        let query = query.trim().to_string();
        if query != self.query {
            self.fetch = None;
            self.online.clear();
            self.online_citations.clear();
            self.note = None;
        }
        self.query = query.clone();
        let bibs = main.map(read_bibs).unwrap_or_default();
        let mut local = Vec::new();
        for (path, b) in &bibs {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            local.extend(bib::search(b, &query, SHOWN).into_iter().map(|c| Citation { from: Some(name.clone()), ..c }));
        }
        self.local = local;
    }

    /// Search online for the query, with `fetch`.
    pub(crate) fn online(&mut self, fetch: Fetch) {
        self.fetch = Some(fetch);
        self.note = None;
    }

    /// The job an online search for the query is.
    pub(crate) fn job(&self) -> Option<Job> {
        let query = self.query.trim();
        (!query.is_empty()).then(|| Job::Search {
            queries: vec![query.to_string()],
            sources: LiteratureSource::ALL.to_vec(),
            per_query: SHOWN as u32,
        })
    }

    /// What an online search found, once it has; whether it ended.
    pub(crate) fn poll(&mut self, main: Option<&Path>) -> bool {
        let Some(fetch) = &mut self.fetch else { return false };
        let mut ended = None;
        for reply in fetch.poll() {
            match reply {
                Reply::Searched { candidates, failures } => {
                    let failed = failures.iter().map(|f| format!("{}: {}", f.source.name(), f.why)).collect::<Vec<_>>();
                    ended = Some((candidates.into_iter().map(|c| c.record).collect::<Vec<_>>(), failed));
                }
                Reply::Failed { why } => ended = Some((Vec::new(), vec![why])),
                _ => {}
            }
        }
        if ended.is_none() && fetch.is_done() {
            ended = Some((Vec::new(), vec!["the search stopped".into()]));
        }
        let Some((records, failed)) = ended else { return false };
        self.fetch = None;
        let bibs = main.map(read_bibs).unwrap_or_default();
        self.online = records.into_iter().take(SHOWN).collect();
        self.online_citations = self.online.iter().enumerate().map(|(i, r)| citation(r, i.to_string(), &bibs)).collect();
        self.note = match (self.online.is_empty(), failed.is_empty()) {
            (true, true) => Some("Nothing was found online.".into()),
            (_, false) => Some(format!("Not every source answered: {}", failed.join("; "))),
            _ => None,
        };
        true
    }

    /// Whether an online search is under way.
    pub(crate) fn searching(&self) -> bool {
        self.fetch.is_some()
    }

    /// The record an online result is, by its place.
    pub(crate) fn online_record(&self, id: &str) -> Option<Record> {
        id.parse::<usize>().ok().and_then(|i| self.online.get(i)).cloned()
    }
}

/// What citing came to: the key, and the `.bib` an entry was added to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cited {
    pub key: String,
    pub added: Option<PathBuf>,
    /// The `.bib` is one the document does not name yet.
    pub unnamed: bool,
}

/// The key a record is cited by in the document at `main`: the one its
/// `.bib` has for it, or a new entry's, added to the first `.bib` it names
/// (`references.bib` beside it when it names none).
pub(crate) fn cite_record(main: &Path, record: &Record) -> Result<Cited, String> {
    let bibs = read_bibs(main);
    let as_bib = record_of(record);
    if let Some(key) = bibs.iter().find_map(|(_, b)| bib::same(b, &as_bib).map(|e| e.key.clone())) {
        return Ok(Cited { key, added: None, unnamed: false });
    }
    let (files, named) = bib_files(main);
    let target = files.into_iter().next().ok_or("the document names no .bib")?;
    let (key, entry) = bib::entry_for(&as_bib, &taken(&bibs));
    let mut text = std::fs::read_to_string(&target).unwrap_or_default();
    if !text.is_empty() {
        while !text.ends_with("\n\n") {
            text.push('\n');
        }
    }
    text.push_str(&entry);
    std::fs::write(&target, text).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(Cited { key, added: Some(target), unnamed: !named })
}

/// What goes in at the cursor to cite `key`, given the line before it: the
/// key alone inside a `\cite{…}` already open — after a comma when it is
/// not the first — else `\cite{key}`.
pub(crate) fn cite_text(before: &str, key: &str) -> String {
    let open = before.rfind('\\').and_then(|at| {
        let command = &before[at + 1..];
        let name: String = command.chars().take_while(char::is_ascii_alphabetic).collect();
        if !name.to_ascii_lowercase().contains("cite") {
            return None;
        }
        let rest = &command[name.len()..];
        let brace = rest.rfind('{')?;
        (!rest[brace..].contains('}')).then(|| rest[brace + 1..].trim_end().chars().last())
    });
    match open {
        Some(None) | Some(Some(',')) => key.to_string(),
        Some(Some(_)) => format!(",{key}"),
        None => format!("\\cite{{{key}}}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_literature::model::Candidate;

    fn project(name: &str, with_bib: bool) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("quark-cite-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let main = root.join("main.tex");
        let bib = if with_bib { "\\bibliography{refs}\n" } else { "" };
        std::fs::write(&main, format!("\\documentclass{{article}}\n\\begin{{document}}\nText.\n{bib}\\end{{document}}\n")).unwrap();
        if with_bib {
            std::fs::write(
                root.join("refs.bib"),
                "@book{knuth1984,\n  title = {The {\\TeX}book},\n  author = {Donald E. Knuth},\n  year = {1984},\n  publisher = {Addison-Wesley}\n}\n\n@article{lamport1994,\n  title = {{\\LaTeX}: A Document Preparation System},\n  author = {Leslie Lamport},\n  year = {1994},\n  journal = {Addison-Wesley}\n}\n",
            )
            .unwrap();
        }
        (root, main)
    }

    fn paper() -> Record {
        Record {
            title: "Attention Is All You Need".into(),
            authors: vec!["Ashish Vaswani".into(), "Noam Shazeer".into()],
            year: Some(2017),
            arxiv: Some("1706.03762".into()),
            sources: vec![LiteratureSource::Arxiv],
            ..Default::default()
        }
    }

    #[test]
    fn a_key_goes_into_a_cite_already_open_and_otherwise_makes_one() {
        assert_eq!(cite_text("As shown ", "knuth1984"), "\\cite{knuth1984}");
        assert_eq!(cite_text("As \\cite{", "knuth1984"), "knuth1984");
        assert_eq!(cite_text("As \\citep[p.~4]{lamport94", "knuth1984"), ",knuth1984");
        assert_eq!(cite_text("As \\parencite{lamport94, ", "knuth1984"), "knuth1984");
        assert_eq!(cite_text("As \\cite{lamport94} and ", "knuth1984"), "\\cite{knuth1984}");
        assert_eq!(cite_text("\\section{", "knuth1984"), "\\cite{knuth1984}");
    }

    #[test]
    fn the_documents_own_bib_is_searched_by_any_word() {
        let (root, main) = project("local", true);
        let mut cite = Cite::default();
        cite.search(Some(&main), "lamport");
        let keys: Vec<&str> = cite.local.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["lamport1994"]);
        assert_eq!(cite.local[0].from.as_deref(), Some("refs.bib"));
        cite.search(Some(&main), "1984 knuth");
        assert_eq!(cite.local.len(), 1);
        assert_eq!(cite.local[0].key, "knuth1984");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_online_result_is_listed_by_the_key_it_would_get_and_cited_into_the_bib() {
        let (root, main) = project("online", true);
        let mut cite = Cite::default();
        cite.search(Some(&main), "attention");
        assert!(matches!(cite.job(), Some(Job::Search { .. })));
        cite.online(Fetch::scripted(vec![Reply::Searched {
            candidates: vec![Candidate { record: paper(), queries: vec!["attention".into()], rank: 0 }],
            failures: vec![],
        }]));
        assert!(cite.searching());
        assert!(cite.poll(Some(&main)));
        assert!(!cite.searching());
        let c = &cite.online_citations[0];
        assert_eq!((c.title.as_str(), c.year.as_deref(), c.from.as_deref()), ("Attention Is All You Need", Some("2017"), Some("arXiv")));
        let key = c.key.clone();
        // Cited: added to the .bib the document names, once.
        let record = cite.online_record(&c.id).unwrap();
        let cited = cite_record(&main, &record).unwrap();
        assert_eq!((cited.key.as_str(), cited.added.as_deref(), cited.unnamed), (key.as_str(), Some(root.join("refs.bib").as_path()), false));
        let bib = std::fs::read_to_string(root.join("refs.bib")).unwrap();
        assert!(bib.contains(&format!("{{{key},")) && bib.contains("knuth1984"), "{bib}");
        let again = cite_record(&main, &record).unwrap();
        assert_eq!((again.key, again.added), (key, None), "already there: its key, nothing added");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_document_with_no_bib_gets_references_bib_and_is_told_it_needs_naming() {
        let (root, main) = project("nobib", false);
        let cited = cite_record(&main, &paper()).unwrap();
        assert_eq!(cited.added.as_deref(), Some(root.join("references.bib").as_path()));
        assert!(cited.unnamed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_search_nobody_answers_says_so() {
        let (root, main) = project("failed", true);
        let mut cite = Cite::default();
        cite.search(Some(&main), "x");
        cite.online(Fetch::scripted(vec![Reply::Failed { why: "no network".into() }]));
        assert!(cite.poll(Some(&main)));
        assert_eq!(cite.note.as_deref(), Some("Not every source answered: no network"));
        let _ = std::fs::remove_dir_all(root);
    }
}
