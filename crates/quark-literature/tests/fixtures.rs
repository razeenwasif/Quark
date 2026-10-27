//! The sources' answers as they were recorded (`tests/live.rs`), read
//! offline: a source that changes what it sends breaks these, not a round.

use quark_literature::merge::combine;
use quark_literature::{arxiv, crossref, openalex};
use quark_literature::model::Access;
use serde_json::Value;

fn read(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

fn json(name: &str) -> Value {
    serde_json::from_str(&read(name)).unwrap()
}

#[test]
fn arxiv_search_and_ids() {
    let found = arxiv::parse(&read("arxiv_search.xml")).unwrap();
    let ids: Vec<&str> = found.iter().filter_map(|r| r.arxiv.as_deref()).collect();
    assert_eq!(ids, ["2208.11215", "astro-ph/0609195", "2007.11851"], "versionless, old ids and new");
    assert!(found.iter().all(|r| r.access == Access::Open && !r.abstract_.is_empty() && !r.authors.is_empty()));
    assert_eq!(found[1].doi.as_deref(), Some("10.1051/0004-6361:20065032"));
    assert_eq!(found[2].venue.as_deref(), Some("A&A 642, A50 (2020)"), "entities read");
    assert_eq!(found[2].year, Some(2020));
    assert!(!found[0].abstract_.contains('\n') && !found[0].title.contains("  "));

    let found = arxiv::parse(&read("arxiv_ids.xml")).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].arxiv.as_deref(), found[0].title.as_str(), found[0].year), (Some("1706.03762"), "Attention Is All You Need", Some(2017)));
    assert_eq!(found[0].pdf_url.as_deref(), Some("https://arxiv.org/pdf/1706.03762"));
    assert_eq!((found[1].arxiv.as_deref(), found[1].doi.as_deref()), (Some("hep-th/9901001"), Some("10.1143/PTP.101.1155")));

    assert!(arxiv::parse(&read("arxiv_unknown.xml")).unwrap().is_empty(), "an id that names no paper is none, not an error");
}

#[test]
fn openalex_search_and_work() {
    let found = openalex::parse_works(&json("openalex_search.json"));
    assert_eq!(found.len(), 3);
    let first = &found[0];
    assert_eq!(
        (first.title.as_str(), first.doi.as_deref(), first.year, first.openalex.as_deref()),
        ("Laser Rayleigh scattering", Some("10.1088/0957-0233/12/5/201"), Some(2001), Some("W2123918241")),
        "a DOI without its doi.org, a work without its openalex.org"
    );
    assert_eq!((first.authors.len(), first.authors[0].as_str()), (3, "Richard B. Miles"));
    assert_eq!(first.venue.as_deref(), Some("Measurement Science and Technology"));
    assert_eq!((first.access, first.pdf_url.as_deref()), (Access::Open, Some("https://iopscience.iop.org/article/10.1088/0957-0233/12/5/201/pdf")));
    assert_eq!((found[1].access, found[1].pdf_url.as_deref()), (Access::Open, None), "open, but with no PDF to be had");
    assert_eq!(found[2].access, Access::Closed);

    let work = openalex::parse_work(&json("openalex_work.json")).unwrap();
    assert_eq!((work.title.as_str(), work.year, work.venue.as_deref()), ("Deep learning", Some(2015), Some("Nature")));
    assert_eq!(work.abstract_, "", "OpenAlex keeps no abstract of some publishers' papers");
}

#[test]
fn crossref_and_the_doi_system() {
    let work = crossref::parse_work(&json("crossref_work.json")).unwrap();
    assert_eq!((work.title.as_str(), work.year, work.venue.as_deref(), work.doi.as_deref()), ("Deep learning", Some(2015), Some("Nature"), Some("10.1038/nature14539")));
    assert_eq!(work.authors, ["Yann LeCun", "Yoshua Bengio", "Geoffrey Hinton"]);
    assert_eq!(json("doi_handle.json")["responseCode"], 1);

    // What two sources say of one paper, as one.
    let openalex = openalex::parse_work(&json("openalex_work.json")).unwrap();
    let both = combine(&[&openalex, &work]);
    assert_eq!((both.title.as_str(), both.openalex.as_deref(), both.citations), ("Deep learning", Some("W2919115771"), Some(85369)));
}
