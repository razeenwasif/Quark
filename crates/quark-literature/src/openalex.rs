//! OpenAlex: everything, with where an open copy of each paper is. Since
//! February 2026 it wants a free key for more than a hundred calls a day.

use std::sync::atomic::AtomicBool;

use crate::model::{Access, LiteratureSource, Record};
use serde_json::Value;

use crate::http::{encode, encode_path, Failure, Http};

const API: &str = "https://api.openalex.org";

/// What is asked about each work.
pub const SELECT: &str =
    "id,doi,title,display_name,publication_year,authorships,primary_location,open_access,best_oa_location,locations,abstract_inverted_index,cited_by_count";

fn key_param(key: Option<&str>) -> String {
    key.map(|k| format!("&api_key={}", encode(k))).unwrap_or_default()
}

pub fn search_url(query: &str, per_page: u32, key: Option<&str>) -> String {
    format!("{API}/works?search={}&per-page={}&select={SELECT}{}", encode(query), per_page.clamp(1, 200), key_param(key))
}

pub fn doi_url(doi: &str, key: Option<&str>) -> String {
    format!("{API}/works/doi:{}?select={SELECT}{}", encode_path(doi), key_param(key))
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// An abstract, put back together: OpenAlex keeps one as each word and the
/// places it stands.
pub fn abstract_from_inverted(v: &Value) -> String {
    let Some(words) = v.as_object() else { return String::new() };
    let mut placed: Vec<(u64, &str)> = Vec::new();
    for (word, places) in words {
        for place in places.as_array().into_iter().flatten().filter_map(Value::as_u64) {
            placed.push((place, word));
        }
    }
    placed.sort();
    placed.into_iter().map(|(_, word)| word).collect::<Vec<_>>().join(" ")
}

/// One work as OpenAlex describes it.
pub fn parse_work(v: &Value) -> Option<Record> {
    let title = text(&v["title"]).or_else(|| text(&v["display_name"]))?;
    let landing: Vec<String> = v["locations"].as_array().into_iter().flatten().filter_map(|l| text(&l["landing_page_url"])).collect();
    let arxiv = landing.iter().find_map(|url| crate::model::Ident::parse(url)).and_then(|id| match id {
        crate::model::Ident::Arxiv(id) => Some(id),
        _ => None,
    });
    let pdf_url = text(&v["best_oa_location"]["pdf_url"]).or_else(|| text(&v["open_access"]["oa_url"]).filter(|u| u.to_lowercase().ends_with(".pdf")));
    let access = match v["open_access"]["is_oa"].as_bool() {
        Some(true) => Access::Open,
        Some(false) if pdf_url.is_none() => Access::Closed,
        _ if pdf_url.is_some() => Access::Open,
        _ => Access::Unknown,
    };
    let doi = text(&v["doi"]).map(|d| d.trim_start_matches("https://doi.org/").to_string());
    Some(
        Record {
            title,
            authors: v["authorships"].as_array().map(|a| a.iter().filter_map(|x| text(&x["author"]["display_name"])).collect()).unwrap_or_default(),
            year: v["publication_year"].as_u64().map(|y| y as u32),
            venue: text(&v["primary_location"]["source"]["display_name"]),
            url: text(&v["primary_location"]["landing_page_url"]).or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}"))),
            doi,
            arxiv,
            openalex: text(&v["id"]).map(|id| id.rsplit('/').next().unwrap_or(&id).to_string()),
            abstract_: abstract_from_inverted(&v["abstract_inverted_index"]),
            citations: v["cited_by_count"].as_u64().map(|c| c as u32),
            access,
            pdf_url,
            sources: vec![LiteratureSource::OpenAlex],
            ..Record::default()
        }
        .capped(),
    )
}

pub fn parse_works(v: &Value) -> Vec<Record> {
    v["results"].as_array().map(|works| works.iter().filter_map(parse_work).collect()).unwrap_or_default()
}

pub fn search(http: &Http, query: &str, per_page: u32, key: Option<&str>, cancel: &AtomicBool) -> Result<Vec<Record>, Failure> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(parse_works(&http.json(&search_url(query, per_page, key), &[], key.is_some(), cancel)?))
}

/// One work by its DOI; none if OpenAlex has no such work.
pub fn lookup_doi(http: &Http, doi: &str, key: Option<&str>, cancel: &AtomicBool) -> Result<Option<Record>, Failure> {
    match http.json(&doi_url(doi, key), &[], key.is_some(), cancel) {
        Ok(v) => Ok(parse_work(&v)),
        Err(Failure::NotFound) => Ok(None),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_work_is_read_its_abstract_put_back_in_order() {
        let answer = json!({ "results": [{
            "id": "https://openalex.org/W2963403868", "doi": "https://doi.org/10.48550/arxiv.1706.03762",
            "title": "Attention Is All You Need", "publication_year": 2017,
            "authorships": [ { "author": { "display_name": "Ashish Vaswani" } }, { "author": { "display_name": "Noam Shazeer" } } ],
            "primary_location": { "landing_page_url": "https://arxiv.org/abs/1706.03762", "source": { "display_name": "arXiv (Cornell University)" } },
            "locations": [ { "landing_page_url": "https://arxiv.org/abs/1706.03762" } ],
            "open_access": { "is_oa": true, "oa_url": "https://arxiv.org/abs/1706.03762" },
            "best_oa_location": { "pdf_url": "https://arxiv.org/pdf/1706.03762" },
            "abstract_inverted_index": { "models": [2], "The": [0], "dominant": [1], "rely": [3] },
            "cited_by_count": 120000
        }]});
        let r = &parse_works(&answer)[0];
        assert_eq!(r.abstract_, "The dominant models rely");
        assert_eq!((r.openalex.as_deref(), r.doi.as_deref(), r.arxiv.as_deref()), (Some("W2963403868"), Some("10.48550/arxiv.1706.03762"), Some("1706.03762")));
        assert_eq!((r.access, r.pdf_url.as_deref(), r.citations), (Access::Open, Some("https://arxiv.org/pdf/1706.03762"), Some(120000)));
        assert_eq!(r.authors, ["Ashish Vaswani", "Noam Shazeer"]);
        let closed = parse_work(&json!({ "id": "https://openalex.org/W1", "display_name": "Closed", "open_access": { "is_oa": false } })).unwrap();
        assert_eq!((closed.title.as_str(), closed.access, closed.pdf_url.clone()), ("Closed", Access::Closed, None));
        assert!(parse_work(&json!({ "id": "https://openalex.org/W2" })).is_none(), "no title, no paper");
    }

    #[test]
    fn a_key_goes_in_the_url_and_only_when_there_is_one() {
        assert!(search_url("sky", 8, Some("k&1")).ends_with("&api_key=k%261"));
        assert!(!search_url("sky", 8, None).contains("api_key"));
        assert!(doi_url("10.1364/JOSAA.18.000412", None).starts_with("https://api.openalex.org/works/doi:10.1364/JOSAA.18.000412?select="));
    }
}
