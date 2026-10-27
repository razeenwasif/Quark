//! Crossref: what a DOI names, asked to check one the team cites. Its
//! public pool needs no key; a person who gives an address is asked in its
//! polite one, through the name Quark asks under (`Http::new`).

use std::sync::atomic::AtomicBool;

use crate::model::Record;
use serde_json::Value;

use crate::http::{encode_path, Failure, Http};

pub fn work_url(doi: &str) -> String {
    format!("https://api.crossref.org/works/{}", encode_path(doi))
}

fn first(v: &Value) -> Option<String> {
    let text = v.as_array().and_then(|a| a.first()).or(Some(v)).and_then(Value::as_str)?;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// What Crossref says a DOI is.
pub fn parse_work(v: &Value) -> Option<Record> {
    let m = &v["message"];
    let title = first(&m["title"])?;
    let year = ["issued", "published-print", "published-online", "created"]
        .iter()
        .find_map(|k| m[*k]["date-parts"].as_array()?.first()?.as_array()?.first()?.as_u64())
        .map(|y| y as u32);
    let authors = m["author"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let name = [x["given"].as_str(), x["family"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                    let name = if name.is_empty() { x["name"].as_str().unwrap_or("").to_string() } else { name };
                    (!name.is_empty()).then_some(name)
                })
                .collect()
        })
        .unwrap_or_default();
    Some(
        Record {
            title,
            authors,
            year,
            venue: first(&m["container-title"]),
            doi: m["DOI"].as_str().map(str::to_string),
            url: m["URL"].as_str().map(str::to_string),
            citations: m["is-referenced-by-count"].as_u64().map(|c| c as u32),
            ..Record::default()
        }
        .capped(),
    )
}

/// What a DOI names, or none if Crossref has no such DOI.
pub fn lookup(http: &Http, doi: &str, cancel: &AtomicBool) -> Result<Option<Record>, Failure> {
    match http.json(&work_url(doi), &[], false, cancel) {
        Ok(v) => Ok(parse_work(&v)),
        Err(Failure::NotFound) => Ok(None),
        Err(other) => Err(other),
    }
}

/// Where the DOI system says whether a DOI was ever registered.
pub fn handle_url(doi: &str) -> String {
    format!("https://doi.org/api/handles/{}", encode_path(doi))
}

/// Whether `doi` was ever registered, by anyone. Crossref knows only its
/// own: a dataset's DOI from DataCite, say, is real though Crossref has no
/// record of it, and must not be called made up.
pub fn registered(http: &Http, doi: &str, cancel: &AtomicBool) -> Result<bool, Failure> {
    match http.json(&handle_url(doi), &[], false, cancel) {
        Ok(_) => Ok(true),
        Err(Failure::NotFound) => Ok(false),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_doi_is_read_as_what_it_names() {
        let answer = json!({ "status": "ok", "message": {
            "DOI": "10.1364/JOSAA.18.000412", "URL": "https://doi.org/10.1364/josaa.18.000412",
            "title": ["Colorimetric and spectroradiometric characteristics of narrow-field-of-view clear skylight\n in Granada, Spain"],
            "author": [ { "given": "Javier", "family": "Hernández-Andrés" }, { "name": "The Granada Group" } ],
            "container-title": ["Journal of the Optical Society of America A"],
            "issued": { "date-parts": [[2001, 2, 1]] }, "is-referenced-by-count": 140
        }});
        let r = parse_work(&answer).unwrap();
        assert_eq!(r.title, "Colorimetric and spectroradiometric characteristics of narrow-field-of-view clear skylight in Granada, Spain");
        assert_eq!((r.year, r.venue.as_deref(), r.citations), (Some(2001), Some("Journal of the Optical Society of America A"), Some(140)));
        assert_eq!(r.authors, ["Javier Hernández-Andrés", "The Granada Group"]);
        assert!(parse_work(&json!({ "message": {} })).is_none());
        assert_eq!(work_url("10.1/a b"), "https://api.crossref.org/works/10.1/a%20b");
    }
}
