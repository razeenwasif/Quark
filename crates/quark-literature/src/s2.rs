//! Semantic Scholar: every field, with how often each paper is cited and,
//! for one asked about by name, a one-line summary. Without a key it is a
//! pool shared by everybody and throttled when busy; with one, one request
//! a second (`http::spacing`).

use std::sync::atomic::AtomicBool;

use crate::model::Ident;
use crate::model::{Access, LiteratureSource, Record};
use serde_json::Value;

use crate::http::{encode, encode_path, Failure, Http};

const API: &str = "https://api.semanticscholar.org/graph/v1";

/// What is asked about each paper. Its search does not give `tldr`; asked
/// about by name, a paper does.
pub const FIELDS: &str = "title,url,abstract,authors,year,venue,externalIds,citationCount,isOpenAccess,openAccessPdf";

pub fn search_url(query: &str, limit: u32) -> String {
    format!("{API}/paper/search?query={}&limit={}&fields={FIELDS}", encode(query), limit.clamp(1, 100))
}

/// Where to ask about one paper, by its DOI or its arXiv id.
pub fn lookup_url(ident: &Ident) -> Option<String> {
    let id = match ident {
        Ident::Doi(doi) => format!("DOI:{doi}"),
        Ident::Arxiv(id) => format!("ARXIV:{id}"),
        Ident::Paper(_) => return None,
    };
    Some(format!("{API}/paper/{}?fields={FIELDS},tldr", encode_path(&id)))
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// One paper as Semantic Scholar describes it.
pub fn parse_paper(v: &Value) -> Option<Record> {
    let title = text(&v["title"])?;
    let pdf_url = text(&v["openAccessPdf"]["url"]);
    let access = match (pdf_url.is_some(), v["isOpenAccess"].as_bool()) {
        (true, _) | (false, Some(true)) => Access::Open,
        (false, Some(false)) => Access::Closed,
        (false, None) => Access::Unknown,
    };
    Some(
        Record {
            title,
            authors: v["authors"].as_array().map(|a| a.iter().filter_map(|x| text(&x["name"])).collect()).unwrap_or_default(),
            year: v["year"].as_u64().map(|y| y as u32),
            venue: text(&v["venue"]),
            doi: text(&v["externalIds"]["DOI"]),
            arxiv: text(&v["externalIds"]["ArXiv"]),
            s2: text(&v["paperId"]),
            abstract_: text(&v["abstract"]).unwrap_or_default(),
            tldr: text(&v["tldr"]["text"]).unwrap_or_default(),
            citations: v["citationCount"].as_u64().map(|c| c as u32),
            access,
            pdf_url,
            url: text(&v["url"]),
            sources: vec![LiteratureSource::SemanticScholar],
            ..Record::default()
        }
        .capped(),
    )
}

pub fn parse_search(v: &Value) -> Vec<Record> {
    v["data"].as_array().map(|papers| papers.iter().filter_map(parse_paper).collect()).unwrap_or_default()
}

fn headers(key: Option<&str>) -> Vec<(&str, &str)> {
    key.map(|k| vec![("x-api-key", k)]).unwrap_or_default()
}

/// The `limit` papers Semantic Scholar finds most relevant to `query`.
pub fn search(http: &Http, query: &str, limit: u32, key: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Record>, Failure> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(parse_search(&http.json(&search_url(query, limit), &headers(key), key.is_some(), cancel)?))
}

/// One paper by name; none if it has no such paper.
pub fn lookup(http: &Http, ident: &Ident, key: Option<&str>, cancel: &AtomicBool) -> Result<Option<Record>, Failure> {
    let Some(url) = lookup_url(ident) else { return Ok(None) };
    match http.json(&url, &headers(key), key.is_some(), cancel) {
        Ok(v) => Ok(parse_paper(&v)),
        Err(Failure::NotFound) => Ok(None),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_paper_is_read_and_an_empty_pdf_link_is_none() {
        let answer = json!({ "total": 2, "offset": 0, "data": [
            { "paperId": "204e3073", "title": "Attention is All you Need", "year": 2017, "venue": "Neural Information Processing Systems",
              "externalIds": { "ArXiv": "1706.03762", "DOI": "10.48550/arXiv.1706.03762", "CorpusId": 13756489 },
              "abstract": "The dominant sequence transduction models…", "citationCount": 150000, "isOpenAccess": false,
              "openAccessPdf": { "url": "", "status": null, "license": null, "disclaimer": "Notice: …" },
              "authors": [ { "authorId": "40348417", "name": "Ashish Vaswani" } ], "url": "https://www.semanticscholar.org/paper/204e3073" },
            { "paperId": "x", "title": null }
        ]});
        let records = parse_search(&answer);
        assert_eq!(records.len(), 1, "a paper without a title is left out");
        let r = &records[0];
        assert_eq!((r.s2.as_deref(), r.arxiv.as_deref(), r.year, r.citations), (Some("204e3073"), Some("1706.03762"), Some(2017), Some(150000)));
        assert_eq!((r.pdf_url.clone(), r.access), (None, Access::Closed), "an empty link is no link");
        assert_eq!(r.authors, ["Ashish Vaswani"]);
        let open = parse_paper(&json!({ "paperId": "y", "title": "Open", "openAccessPdf": { "url": "https://example.org/p.pdf" }, "tldr": { "text": "In one line." } })).unwrap();
        assert_eq!((open.access, open.pdf_url.as_deref(), open.tldr.as_str()), (Access::Open, Some("https://example.org/p.pdf"), "In one line."));
    }

    #[test]
    fn papers_are_asked_for_by_doi_or_arxiv_id() {
        assert_eq!(
            lookup_url(&Ident::Doi("10.1364/JOSAA.18.000412".into())).unwrap(),
            format!("{API}/paper/DOI:10.1364/JOSAA.18.000412?fields={FIELDS},tldr")
        );
        assert_eq!(lookup_url(&Ident::Arxiv("1706.03762".into())).unwrap(), format!("{API}/paper/ARXIV:1706.03762?fields={FIELDS},tldr"));
        assert_eq!(lookup_url(&Ident::Paper("P001".into())), None);
        assert!(search_url("sky colour", 500).contains("&limit=100&"), "at most a hundred");
    }
}
