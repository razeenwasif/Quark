//! The real sources, asked without keys. Run by hand — they are slow on
//! purpose, and spend the hundred calls a day OpenAlex allows without a key:
//!
//!   cargo test -p quark-literature --test live -- --ignored --test-threads 1
//!
//! With `PROTON_LITERATURE_RECORD=1` they also write what the sources
//! answered to `tests/fixtures`, trimmed, for `tests/fixtures.rs` to read
//! offline.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use quark_literature::{arxiv, crossref, http, openalex, s2, Failure, Http};
use quark_literature::model::Ident;
use serde_json::Value;

/// What is asked about, here and in the fixtures.
const QUERY: &str = "rayleigh scattering sky colour";
const DOI: &str = "10.1038/nature14539";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn recording() -> bool {
    std::env::var_os("PROTON_LITERATURE_RECORD").is_some()
}

/// Keep an answer, with nothing in it that names whoever asked.
fn keep(name: &str, body: &str) {
    if recording() {
        let body = http::scrub(body);
        assert!(!body.contains("mailto:"), "{name} names a contact");
        std::fs::write(fixture(name), body).unwrap();
    }
}

/// Keep a JSON answer, less the parts of it named in `drop` (at the top of
/// `message` for Crossref, otherwise at the top), pretty.
fn keep_json(name: &str, body: &str, drop: &[&str]) {
    let mut v: Value = serde_json::from_str(body).unwrap();
    let inner = if v.get("message").is_some() { &mut v["message"] } else { &mut v };
    if let Some(map) = inner.as_object_mut() {
        for key in drop {
            map.remove(*key);
        }
    }
    keep(name, &format!("{}\n", serde_json::to_string_pretty(&v).unwrap()));
}

fn text(http: &Http, url: &str) -> Result<String, Failure> {
    http.text(url, &[], false, &AtomicBool::new(false))
}

#[test]
#[ignore]
fn arxiv_answers_a_search_and_its_ids() {
    let http = Http::live(None);
    let xml = text(&http, &arxiv::search_url(QUERY, 3).unwrap()).unwrap();
    let found = arxiv::parse(&xml).unwrap();
    assert!(!found.is_empty() && found.iter().all(|r| !r.title.is_empty() && r.arxiv.is_some() && r.pdf_url.is_some()), "{found:#?}");
    keep("arxiv_search.xml", &xml);

    let xml = text(&http, &arxiv::ids_url(&["1706.03762".into(), "hep-th/9901001".into()])).unwrap();
    let found = arxiv::parse(&xml).unwrap();
    let ids: Vec<_> = found.iter().filter_map(|r| r.arxiv.clone()).collect();
    assert_eq!(ids, ["1706.03762", "hep-th/9901001"], "{found:#?}");
    keep("arxiv_ids.xml", &xml);

    // An id in the right form that names no paper.
    let xml = text(&http, &arxiv::ids_url(&["2101.99999".into()])).unwrap();
    assert_eq!(arxiv::parse(&xml).unwrap(), Vec::new(), "{xml}");
    keep("arxiv_unknown.xml", &xml);
}

#[test]
#[ignore]
fn semantic_scholar_answers_a_search_and_a_paper() {
    let http = Http::live(None);
    let body = text(&http, &s2::search_url(QUERY, 3)).unwrap();
    let found = s2::parse_search(&serde_json::from_str(&body).unwrap());
    assert!(!found.is_empty() && found.iter().all(|r| r.s2.is_some()), "{found:#?}");
    keep_json("s2_search.json", &body, &[]);

    let body = text(&http, &s2::lookup_url(&Ident::Arxiv("1706.03762".into())).unwrap()).unwrap();
    let paper = s2::parse_paper(&serde_json::from_str(&body).unwrap()).unwrap();
    assert_eq!(paper.arxiv.as_deref(), Some("1706.03762"));
    assert!(!paper.tldr.is_empty(), "a paper asked about by name has its TL;DR: {paper:#?}");
    keep_json("s2_paper.json", &body, &[]);
}

#[test]
#[ignore]
fn openalex_answers_a_search_and_a_doi() {
    let http = Http::live(None);
    let body = text(&http, &openalex::search_url(QUERY, 3, None)).unwrap();
    let found = openalex::parse_works(&serde_json::from_str(&body).unwrap());
    assert!(!found.is_empty() && found.iter().all(|r| r.openalex.is_some()), "{found:#?}");
    keep_json("openalex_search.json", &body, &[]);

    let body = text(&http, &openalex::doi_url(DOI, None)).unwrap();
    let work = openalex::parse_work(&serde_json::from_str(&body).unwrap()).unwrap();
    assert_eq!(work.doi.as_deref().map(str::to_lowercase).as_deref(), Some(DOI));
    keep_json("openalex_work.json", &body, &[]);
}

#[test]
#[ignore]
fn crossref_and_the_doi_system_say_what_is_real() {
    let http = Http::live(None);
    let body = text(&http, &crossref::work_url(DOI)).unwrap();
    let work = crossref::parse_work(&serde_json::from_str(&body).unwrap()).unwrap();
    assert_eq!(work.title, "Deep learning");
    keep_json("crossref_work.json", &body, &["reference", "relation", "assertion", "link", "license", "funder"]);
    assert_eq!(text(&http, &crossref::work_url("10.1038/proton-no-such-doi")), Err(Failure::NotFound));

    let cancel = AtomicBool::new(false);
    let body = text(&http, &crossref::handle_url("10.48550/arXiv.1706.03762")).unwrap();
    keep_json("doi_handle.json", &body, &[]);
    assert_eq!(crossref::registered(&http, "10.48550/arXiv.1706.03762", &cancel), Ok(true), "arXiv's own DOI, which Crossref does not hold");
    assert_eq!(crossref::registered(&http, "10.1038/proton-no-such-doi", &cancel), Ok(false));
}
