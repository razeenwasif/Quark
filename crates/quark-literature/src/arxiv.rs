//! arXiv: physics, mathematics and computer science preprints, every one
//! with its PDF. Asked through its export API, which answers in Atom, no
//! oftener than once every three seconds (`http::spacing`).

use std::sync::atomic::AtomicBool;

use crate::model::{Access, LiteratureSource, Record};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::http::{encode, Failure, Http};

const API: &str = "https://export.arxiv.org/api/query";

/// Words a search does better without; arXiv joins what is left with AND.
const LITTLE: [&str; 16] = ["the", "a", "an", "of", "in", "on", "to", "for", "and", "or", "by", "with", "is", "are", "why", "how"];

/// A query's terms: its quoted phrases whole, its other words one by one,
/// the little ones and stray punctuation left out — at most six.
fn terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, part) in query.split('"').enumerate() {
        let clean = |w: &str| w.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>().to_lowercase();
        if i % 2 == 1 {
            let phrase = part.split_whitespace().map(clean).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
            if !phrase.is_empty() {
                out.push(phrase);
            }
        } else {
            out.extend(part.split_whitespace().map(clean).filter(|w| w.chars().count() > 1 && !LITTLE.contains(&w.as_str())));
        }
    }
    out.truncate(6);
    out
}

/// Where to ask arXiv for the `max` papers most relevant to `query`.
pub fn search_url(query: &str, max: u32) -> Option<String> {
    let terms = terms(query);
    if terms.is_empty() {
        return None;
    }
    let asked = terms.iter().map(|t| if t.contains(' ') { format!("all:\"{t}\"") } else { format!("all:{t}") }).collect::<Vec<_>>().join(" AND ");
    Some(format!("{API}?search_query={}&start=0&max_results={max}&sortBy=relevance&sortOrder=descending", encode(&asked)))
}

/// Where to ask arXiv for papers by their ids.
pub fn ids_url(ids: &[String]) -> String {
    let list = ids.iter().map(|id| encode(id)).collect::<Vec<_>>().join(",");
    format!("{API}?id_list={list}&max_results={}", ids.len().max(1))
}

/// One `<entry>` as it is read.
#[derive(Default)]
struct Entry {
    id: String,
    title: String,
    summary: String,
    published: String,
    authors: Vec<String>,
    doi: String,
    journal: String,
}

/// Runs of whitespace — arXiv's titles and abstracts break their lines — made single spaces.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An arXiv id out of an entry's `<id>`, without its version.
fn id_of(link: &str) -> Option<String> {
    let rest = link.split("/abs/").nth(1)?;
    let id = match rest.rfind('v') {
        Some(at) if at > 0 && rest[at + 1..].bytes().all(|b| b.is_ascii_digit()) && at + 1 < rest.len() => &rest[..at],
        _ => rest,
    };
    (!id.is_empty()).then(|| id.to_string())
}

impl Entry {
    fn record(self) -> Option<Record> {
        let id = id_of(&self.id)?;
        Some(
            Record {
                title: squeeze(&self.title),
                authors: self.authors.iter().map(|a| squeeze(a)).filter(|a| !a.is_empty()).collect(),
                year: self.published.get(..4).and_then(|y| y.parse().ok()),
                venue: Some(squeeze(&self.journal)).filter(|j| !j.is_empty()),
                doi: Some(self.doi.trim().to_string()).filter(|d| !d.is_empty()),
                abstract_: squeeze(&self.summary),
                access: Access::Open,
                pdf_url: Some(format!("https://arxiv.org/pdf/{id}")),
                url: Some(format!("https://arxiv.org/abs/{id}")),
                sources: vec![LiteratureSource::Arxiv],
                arxiv: Some(id),
                ..Record::default()
            }
            .capped(),
        )
    }
}

/// The papers in an Atom answer. An answer that is one of arXiv's errors —
/// a malformed query — is that error; an id it has no paper for is left out.
pub fn parse(xml: &str) -> Result<Vec<Record>, String> {
    let mut reader = Reader::from_str(xml);
    let mut records = Vec::new();
    let mut entry: Option<Entry> = None;
    let mut field: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match name.as_str() {
                    "entry" => entry = Some(Entry::default()),
                    "id" | "title" | "summary" | "published" | "name" | "doi" | "journal_ref" if entry.is_some() => {
                        field = Some(name);
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) if field.is_some() => text.push_str(&t.decode().map_err(|e| e.to_string())?),
            Event::CData(t) if field.is_some() => text.push_str(&t.decode().map_err(|e| e.to_string())?),
            Event::GeneralRef(r) if field.is_some() => {
                if r.is_char_ref() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        text.push(c);
                    }
                } else {
                    match r.decode().map_err(|e| e.to_string())?.as_ref() {
                        "amp" => text.push('&'),
                        "lt" => text.push('<'),
                        "gt" => text.push('>'),
                        "quot" => text.push('"'),
                        "apos" => text.push('\''),
                        other => text.push_str(&format!("&{other};")),
                    }
                }
            }
            Event::End(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if field.as_deref() == Some(name.as_str()) {
                    field = None;
                    if let Some(entry) = entry.as_mut() {
                        let value = std::mem::take(&mut text);
                        match name.as_str() {
                            "id" => entry.id = value,
                            "title" => entry.title = value,
                            "summary" => entry.summary = value,
                            "published" => entry.published = value,
                            "name" => entry.authors.push(value),
                            "doi" => entry.doi = value,
                            "journal_ref" => entry.journal = value,
                            _ => {}
                        }
                    }
                } else if name == "entry" {
                    if let Some(done) = entry.take() {
                        if done.id.contains("/api/errors") {
                            // One bad id in a list is that id's; a bad query is the answer's.
                            if done.title.trim() == "Error" && records.is_empty() && !done.summary.contains("id_list") {
                                return Err(squeeze(&done.summary));
                            }
                            continue;
                        }
                        records.extend(done.record());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(records)
}

/// The `max` papers arXiv finds most relevant to `query`.
pub fn search(http: &Http, query: &str, max: u32, cancel: &AtomicBool) -> Result<Vec<Record>, Failure> {
    let Some(url) = search_url(query, max) else { return Ok(Vec::new()) };
    let xml = http.text(&url, &[], false, cancel)?;
    parse(&xml).map_err(|why| Failure::Unreadable { host: "export.arxiv.org".into(), why })
}

/// arXiv's papers by their ids, twenty at a time.
pub fn lookup(http: &Http, ids: &[String], cancel: &AtomicBool) -> Result<Vec<Record>, Failure> {
    let mut out = Vec::new();
    for chunk in ids.chunks(20) {
        let xml = http.text(&ids_url(chunk), &[], false, cancel)?;
        out.extend(parse(&xml).map_err(|why| Failure::Unreadable { host: "export.arxiv.org".into(), why })?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_keeps_its_phrases_and_drops_its_little_words() {
        assert_eq!(terms("Why is the \"Rayleigh scattering\" of sky light blue?"), ["rayleigh scattering", "sky", "light", "blue"]);
        assert_eq!(
            search_url("\"Rayleigh scattering\" ozone", 8).unwrap(),
            "https://export.arxiv.org/api/query?search_query=all%3A%22rayleigh%20scattering%22%20AND%20all%3Aozone&start=0&max_results=8&sortBy=relevance&sortOrder=descending"
        );
        assert_eq!(search_url("? !", 8), None);
        assert_eq!(ids_url(&["1706.03762".into(), "hep-th/9901001".into()]), "https://export.arxiv.org/api/query?id_list=1706.03762,hep-th%2F9901001&max_results=2");
    }

    #[test]
    fn an_answer_is_read_entities_and_all() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title>arXiv Query</title>
  <entry>
    <id>http://arxiv.org/abs/1706.03762v7</id>
    <published>2017-06-12T17:57:34Z</published>
    <title>Attention Is All You
      Need</title>
    <summary>  The dominant sequence transduction models &amp; their &lt;encoders&gt; &#8212; replaced.
    </summary>
    <author><name>Ashish Vaswani</name></author>
    <author><name>Noam Shazeer</name></author>
    <arxiv:doi>10.1000/example.1</arxiv:doi>
    <arxiv:journal_ref>NeurIPS 2017</arxiv:journal_ref>
    <link href="http://arxiv.org/abs/1706.03762v7" rel="alternate" type="text/html"/>
    <link title="pdf" href="http://arxiv.org/pdf/1706.03762v7" rel="related" type="application/pdf"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/hep-th/9901001v1</id>
    <published>1999-01-01T00:00:00Z</published>
    <title><![CDATA[Old <style> id]]></title>
    <summary>An old one.</summary>
    <author><name>A. Physicist</name></author>
  </entry>
</feed>"#;
        let records = parse(xml).unwrap();
        assert_eq!(records.len(), 2);
        let r = &records[0];
        assert_eq!(r.arxiv.as_deref(), Some("1706.03762"), "without its version");
        assert_eq!(r.title, "Attention Is All You Need");
        assert_eq!(r.abstract_, "The dominant sequence transduction models & their <encoders> \u{2014} replaced.");
        assert_eq!((r.year, r.doi.as_deref(), r.venue.as_deref()), (Some(2017), Some("10.1000/example.1"), Some("NeurIPS 2017")));
        assert_eq!(r.authors, ["Ashish Vaswani", "Noam Shazeer"]);
        assert_eq!((r.pdf_url.as_deref(), r.access), (Some("https://arxiv.org/pdf/1706.03762"), Access::Open), "the latest version's PDF");
        assert_eq!((records[1].arxiv.as_deref(), records[1].title.as_str()), (Some("hep-th/9901001"), "Old <style> id"));
    }

    #[test]
    fn an_error_is_said_and_an_unknown_id_left_out() {
        let error = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>http://arxiv.org/api/errors#malformed</id><title>Error</title>
            <summary>malformed query: unbalanced quotes</summary></entry></feed>"#;
        assert_eq!(parse(error).unwrap_err(), "malformed query: unbalanced quotes");
        let unknown = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>http://arxiv.org/api/errors#incorrect_id_format_for_9999.99999</id>
            <title>Error</title><summary>incorrect id format for 9999.99999 in id_list</summary></entry></feed>"#;
        assert_eq!(parse(unknown).unwrap(), Vec::<Record>::new());
        assert_eq!(parse("<feed xmlns=\"http://www.w3.org/2005/Atom\"></feed>").unwrap(), Vec::<Record>::new());
    }
}
