//! One paper out of what several sources said about it: joined on any name
//! they share — a DOI, an arXiv id, the title and year — each field taken
//! from the source that says it best.

use crate::model::{keys, Candidate};
use crate::model::{Access, LiteratureSource, Record};

/// What one source answered to one query: a paper, and its place in the answer.
#[derive(Debug, Clone)]
pub struct Hit {
    pub record: Record,
    pub query: String,
    pub rank: u32,
}

use LiteratureSource::{Arxiv, OpenAlex, SemanticScholar};

/// The first of `hits`, in the order of `prefer`'s sources and then any
/// other, that has `field`.
fn pick<T>(hits: &[&Record], prefer: &[LiteratureSource], field: impl Fn(&Record) -> Option<T>) -> Option<T> {
    let rank = |r: &Record| r.sources.first().and_then(|s| prefer.iter().position(|p| p == s)).unwrap_or(prefer.len());
    let mut ordered: Vec<&&Record> = hits.iter().collect();
    ordered.sort_by_key(|r| rank(r));
    ordered.into_iter().find_map(|r| field(r))
}

fn some(text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| text.to_string())
}

/// What several sources said about one paper, as one record: arXiv's
/// abstract and PDF before the others' (OpenAlex's abstract loses its
/// punctuation), a journal's venue and year before arXiv's, a journal's DOI
/// before arXiv's own, the highest count of citations.
pub fn combine(records: &[&Record]) -> Record {
    let doi = records.iter().filter_map(|r| r.doi.clone()).max_by_key(|d| !d.starts_with("10.48550/"));
    let arxiv = records.iter().find_map(|r| r.arxiv.clone());
    let access = if records.iter().any(|r| r.access == Access::Open) {
        Access::Open
    } else if records.iter().any(|r| r.access == Access::Closed) {
        Access::Closed
    } else {
        Access::Unknown
    };
    let mut sources: Vec<LiteratureSource> = records.iter().flat_map(|r| r.sources.iter().copied()).collect();
    sources.sort();
    sources.dedup();
    let url = arxiv
        .as_ref()
        .map(|id| format!("https://arxiv.org/abs/{id}"))
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")))
        .or_else(|| pick(records, &[SemanticScholar, OpenAlex], |r| r.url.clone()));
    Record {
        title: pick(records, &[Arxiv, SemanticScholar, OpenAlex], |r| some(&r.title)).unwrap_or_default(),
        authors: records.iter().map(|r| r.authors.clone()).max_by_key(Vec::len).unwrap_or_default(),
        authors_more: records.iter().map(|r| r.authors_more).max().unwrap_or(0),
        year: pick(records, &[SemanticScholar, OpenAlex, Arxiv], |r| r.year),
        venue: pick(records, &[SemanticScholar, OpenAlex, Arxiv], |r| r.venue.clone()),
        doi,
        arxiv,
        s2: records.iter().find_map(|r| r.s2.clone()),
        openalex: records.iter().find_map(|r| r.openalex.clone()),
        abstract_: pick(records, &[Arxiv, SemanticScholar, OpenAlex], |r| some(&r.abstract_)).unwrap_or_default(),
        tldr: pick(records, &[SemanticScholar], |r| some(&r.tldr)).unwrap_or_default(),
        citations: records.iter().filter_map(|r| r.citations).max(),
        access,
        pdf_url: pick(records, &[Arxiv, SemanticScholar, OpenAlex], |r| r.pdf_url.clone()),
        url,
        sources,
    }
}

/// The papers in several sources' answers, one apiece: hits that share any
/// name joined, each with every query that found it and its best place.
/// Hits that name one paper, with the names they share and where each came.
type Group = (Vec<String>, Vec<(usize, Hit)>);

pub fn merge(hits: Vec<Hit>) -> Vec<Candidate> {
    let mut groups: Vec<Group> = Vec::new();
    for (at, hit) in hits.into_iter().enumerate() {
        let mut names = keys(&hit.record);
        let mut joined = vec![(at, hit)];
        // Every group this one shares a name with becomes one with it.
        let mut i = groups.len();
        while i > 0 {
            i -= 1;
            if groups[i].0.iter().any(|k| names.contains(k)) {
                let (more, hits) = groups.remove(i);
                names.extend(more);
                joined.extend(hits);
            }
        }
        names.sort();
        names.dedup();
        // In the order they came, whatever order the groups were joined in.
        joined.sort_by_key(|(at, _)| *at);
        groups.push((names, joined));
    }
    groups.sort_by_key(|(_, hits)| hits[0].0);
    let mut out: Vec<Candidate> = groups
        .into_iter()
        .map(|(_, hits)| {
            let hits: Vec<Hit> = hits.into_iter().map(|(_, hit)| hit).collect();
            let records: Vec<&Record> = hits.iter().map(|h| &h.record).collect();
            let mut queries: Vec<String> = Vec::new();
            for hit in &hits {
                if !hit.query.is_empty() && !queries.contains(&hit.query) {
                    queries.push(hit.query.clone());
                }
            }
            Candidate { record: combine(&records), queries, rank: hits.iter().map(|h| h.rank).min().unwrap_or(0) }
        })
        .collect();
    out.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.record.title.cmp(&b.record.title)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(source: LiteratureSource, query: &str, rank: u32, record: Record) -> Hit {
        Hit { record: Record { sources: vec![source], ..record }, query: query.into(), rank }
    }

    #[test]
    fn three_sources_saying_one_paper_are_one_paper() {
        let hits = vec![
            hit(Arxiv, "attention", 3, Record {
                title: "Attention Is All You Need".into(),
                arxiv: Some("1706.03762".into()),
                abstract_: "The dominant sequence transduction models, based on recurrence.".into(),
                pdf_url: Some("https://arxiv.org/pdf/1706.03762".into()),
                access: Access::Open,
                year: Some(2017),
                ..Record::default()
            }),
            hit(SemanticScholar, "transformers", 0, Record {
                title: "Attention is All you Need".into(),
                arxiv: Some("1706.03762".into()),
                doi: Some("10.48550/arXiv.1706.03762".into()),
                s2: Some("204e3073".into()),
                venue: Some("NeurIPS".into()),
                tldr: "A new architecture.".into(),
                citations: Some(150_000),
                access: Access::Closed,
                ..Record::default()
            }),
            hit(OpenAlex, "transformers", 1, Record {
                title: "Attention Is All You Need".into(),
                doi: Some("10.48550/arXiv.1706.03762".into()),
                openalex: Some("W2963403868".into()),
                abstract_: "The dominant sequence transduction models based on recurrence".into(),
                citations: Some(120_000),
                ..Record::default()
            }),
            hit(OpenAlex, "transformers", 2, Record { title: "Something else entirely".into(), year: Some(2020), ..Record::default() }),
        ];
        let merged = merge(hits);
        assert_eq!(merged.len(), 2, "{merged:#?}");
        let paper = &merged[0];
        let r = &paper.record;
        assert_eq!(r.sources, [Arxiv, SemanticScholar, OpenAlex]);
        assert_eq!(r.abstract_, "The dominant sequence transduction models, based on recurrence.", "arXiv's, punctuation and all");
        assert_eq!((r.s2.as_deref(), r.openalex.as_deref(), r.citations, r.access), (Some("204e3073"), Some("W2963403868"), Some(150_000), Access::Open));
        assert_eq!((r.venue.as_deref(), r.tldr.as_str()), (Some("NeurIPS"), "A new architecture."));
        assert_eq!(r.url.as_deref(), Some("https://arxiv.org/abs/1706.03762"));
        assert_eq!((paper.queries.clone(), paper.rank), (vec!["attention".to_string(), "transformers".to_string()], 0));
    }

    #[test]
    fn a_journal_doi_is_kept_over_arxivs_own() {
        let a = Record { doi: Some("10.48550/arXiv.2001.00001".into()), sources: vec![OpenAlex], ..Record::default() };
        let b = Record { doi: Some("10.1103/PhysRevD.1.1".into()), sources: vec![SemanticScholar], ..Record::default() };
        assert_eq!(combine(&[&a, &b]).doi.as_deref(), Some("10.1103/PhysRevD.1.1"));
    }
}
