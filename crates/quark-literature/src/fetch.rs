//! The network, kept off the core's thread: a search, a lookup or a set of
//! downloads runs on a worker of its own and says what it finds into a
//! mailbox the core empties when it polls, as a trace does (`quark-trace`'s
//! `Run`). Dropped, it is cancelled; a worker that dies says so rather than
//! leave the core waiting.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use crate::model::Ident;
use crate::model::Candidate;
use crate::model::{LiteratureSource, Record};

use crate::http::{Failure, Http};
use crate::keys::Keys;
use crate::merge::{combine, merge, Hit};
use crate::{arxiv, crossref, openalex, pdf, s2};

/// What a worker is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// Ask each of `sources` each of `queries`, for `per_query` papers apiece.
    Search { queries: Vec<String>, sources: Vec<LiteratureSource>, per_query: u32 },
    /// Find papers by name — DOIs and arXiv ids — in `sources`, and in arXiv
    /// and Crossref, whose word on their own ids is final. With no sources it
    /// only checks that each name is real.
    Lookup { idents: Vec<Ident>, sources: Vec<LiteratureSource> },
    /// Keep open PDFs, each no bigger than `cap` bytes.
    Download { items: Vec<Download>, cap: u64 },
}

/// A PDF to keep: where it may be had, best first, and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    /// What the asker knows it by: a paper's id.
    pub key: String,
    pub urls: Vec<String>,
    pub dest: PathBuf,
}

/// A source that could not be asked.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFailure {
    pub source: LiteratureSource,
    pub why: String,
    /// It asked not to be asked again for now — busy, or out of the calls it
    /// allows in a day — and was not, for the rest of the job.
    pub exhausted: bool,
}

/// What a worker says.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// A source answered one query with `found` papers.
    Progress { source: LiteratureSource, query: String, found: usize },
    /// The search is done: the papers it found, merged, best first.
    Searched { candidates: Vec<Candidate>, failures: Vec<SourceFailure> },
    /// The lookup is done: what each name was found to be; the names nobody
    /// has; and the ones that could not be asked about, with why.
    Resolved { found: Vec<(Ident, Record)>, missing: Vec<Ident>, failures: Vec<(Ident, String)> },
    /// A PDF kept, from `url`.
    Saved { key: String, bytes: u64, sha256: String, url: String },
    /// A PDF not kept, and why.
    Unsaved { key: String, why: String },
    /// The downloads are done.
    Finished,
    /// The worker could not go on.
    Failed { why: String },
}

impl Reply {
    /// Whether it is the last a worker says.
    pub fn is_last(&self) -> bool {
        matches!(self, Reply::Searched { .. } | Reply::Resolved { .. } | Reply::Finished | Reply::Failed { .. })
    }
}

/// What a worker asks with: the client, and the keys — found only when a
/// worker first needs them, since finding them may mean asking the login
/// shell, which the core must not wait on.
#[derive(Clone)]
pub struct Context {
    pub http: Http,
    pub keys: Arc<dyn Fn() -> Keys + Send + Sync>,
}

impl Context {
    pub fn new(http: Http, keys: impl Fn() -> Keys + Send + Sync + 'static) -> Context {
        Context { http, keys: Arc::new(keys) }
    }
}

/// A job in flight.
pub struct Fetch {
    replies: Receiver<Reply>,
    stop: Arc<AtomicBool>,
    done: bool,
    /// Keeps a mailbox open that nobody else writes to.
    _held: Option<Sender<Reply>>,
}

impl std::fmt::Debug for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetch").field("done", &self.done).finish()
    }
}

impl Fetch {
    /// Start `job` on a worker. Returns at once.
    pub fn start(context: &Context, job: Job) -> Fetch {
        let (tx, replies) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let context = context.clone();
        let worker_tx = tx.clone();
        let spawned = std::thread::Builder::new().name("quark-literature".into()).spawn(move || {
            let tx = worker_tx;
            if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| work(&context, job, &tx, &worker_stop))) {
                let what = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
                let _ = tx.send(Reply::Failed { why: format!("the literature worker stopped: {what}") });
            }
        });
        if let Err(e) = spawned {
            let _ = tx.send(Reply::Failed { why: format!("the literature worker could not start: {e}") });
        }
        Fetch { replies, stop, done: false, _held: None }
    }

    /// A job whose replies are `replies`, for tests. Without a last one among
    /// them it is a worker that died.
    pub fn scripted(replies: Vec<Reply>) -> Fetch {
        let (fetch, tx) = Fetch::manual();
        for reply in replies {
            let _ = tx.send(reply);
        }
        fetch
    }

    /// A job whose replies a test sends when it likes; dropping the sender
    /// is the worker dying.
    pub fn manual() -> (Fetch, Sender<Reply>) {
        let (tx, replies) = mpsc::channel();
        (Fetch { replies, stop: Arc::new(AtomicBool::new(false)), done: false, _held: None }, tx)
    }

    /// A job that never answers.
    pub fn pending() -> Fetch {
        let (mut fetch, tx) = Fetch::manual();
        fetch._held = Some(tx);
        fetch
    }

    /// Whatever has arrived. Never blocks.
    pub fn poll(&mut self) -> Vec<Reply> {
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        loop {
            match self.replies.try_recv() {
                Ok(reply) => {
                    let last = reply.is_last();
                    out.push(reply);
                    if last {
                        self.done = true;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.done = true;
                    out.push(Reply::Failed { why: "the literature worker stopped unexpectedly".into() });
                    break;
                }
            }
        }
        out
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Stop asking. A request already sent is let finish, and its answer
    /// thrown away.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn work(context: &Context, job: Job, tx: &Sender<Reply>, stop: &AtomicBool) {
    let keys = (context.keys)();
    let http = context.http.with_contact(keys.mailto.as_deref());
    match job {
        Job::Search { queries, sources, per_query } => {
            let (candidates, failures) = search(&http, &keys, &queries, &sources, per_query, tx, stop);
            if !stop.load(Ordering::Relaxed) {
                let _ = tx.send(Reply::Searched { candidates, failures });
            }
        }
        Job::Lookup { idents, sources } => {
            if let Some((found, missing, failures)) = lookup(&http, &keys, &idents, &sources, stop) {
                let _ = tx.send(Reply::Resolved { found, missing, failures });
            }
        }
        Job::Download { items, cap } => {
            for item in items {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let reply = keep(&http, &item, cap, stop);
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(reply);
            }
            let _ = tx.send(Reply::Finished);
        }
    }
}

/// Each source asked on a thread of its own — each waits only on its own
/// host — and their answers merged in the order the sources were given, so
/// the same answers make the same papers.
fn search(
    http: &Http,
    keys: &Keys,
    queries: &[String],
    sources: &[LiteratureSource],
    per_query: u32,
    tx: &Sender<Reply>,
    stop: &AtomicBool,
) -> (Vec<Candidate>, Vec<SourceFailure>) {
    let mut asked: Vec<String> = Vec::new();
    for query in queries.iter().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        if !asked.iter().any(|a| a == query) {
            asked.push(query.to_string());
        }
    }
    let mut which: Vec<LiteratureSource> = Vec::new();
    for source in sources {
        if !which.contains(source) {
            which.push(*source);
        }
    }
    let asked = &asked;
    let answers: Vec<(Vec<Hit>, Vec<SourceFailure>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = which
            .iter()
            .map(|&source| {
                let tx = tx.clone();
                (source, scope.spawn(move || ask(http, keys, source, asked, per_query, &tx, stop)))
            })
            .collect();
        workers
            .into_iter()
            .map(|(source, worker)| {
                worker.join().unwrap_or_else(|_| (Vec::new(), vec![SourceFailure { source, why: "its search stopped unexpectedly".into(), exhausted: false }]))
            })
            .collect()
    });
    let mut hits = Vec::new();
    let mut failures = Vec::new();
    for (found, failed) in answers {
        hits.extend(found);
        failures.extend(failed);
    }
    (merge(hits), failures)
}

/// One source asked each query in turn. One that says it is busy, or out of
/// calls, or cannot be reached, or will not take the key, is asked no more.
fn ask(http: &Http, keys: &Keys, source: LiteratureSource, queries: &[String], per_query: u32, tx: &Sender<Reply>, stop: &AtomicBool) -> (Vec<Hit>, Vec<SourceFailure>) {
    let mut hits = Vec::new();
    let mut failures = Vec::new();
    for query in queries {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let answer = match source {
            LiteratureSource::Arxiv => arxiv::search(http, query, per_query, stop),
            LiteratureSource::SemanticScholar => s2::search(http, query, per_query, keys.semantic_scholar.as_deref(), stop),
            LiteratureSource::OpenAlex => openalex::search(http, query, per_query, keys.openalex.as_deref(), stop),
        };
        match answer {
            Ok(records) => {
                let _ = tx.send(Reply::Progress { source, query: query.clone(), found: records.len() });
                hits.extend(records.into_iter().enumerate().map(|(rank, record)| Hit { record, query: query.clone(), rank: rank as u32 }));
            }
            Err(Failure::Cancelled) => break,
            Err(Failure::NotFound) => {
                let _ = tx.send(Reply::Progress { source, query: query.clone(), found: 0 });
            }
            Err(failure) => {
                let exhausted = matches!(failure, Failure::Busy { .. } | Failure::Status { status: 429, .. });
                let done = exhausted || matches!(failure, Failure::Network { .. } | Failure::Status { status: 401 | 403, .. });
                failures.push(SourceFailure { source, why: explain(source, &failure, keys), exhausted });
                if done {
                    break;
                }
            }
        }
    }
    (hits, failures)
}

/// Why a source failed, said so the person knows what would help.
pub fn explain(source: LiteratureSource, failure: &Failure, keys: &Keys) -> String {
    let busy = matches!(failure, Failure::Busy { .. } | Failure::Status { status: 429, .. });
    let refused = matches!(failure, Failure::Status { status: 401 | 403, .. });
    match source {
        LiteratureSource::OpenAlex if busy && keys.openalex.is_none() => {
            "OpenAlex has answered all the calls it allows in a day without a key: a free one, in OPENALEX_API_KEY, gives more".into()
        }
        LiteratureSource::OpenAlex if refused && keys.openalex.is_some() => "OpenAlex did not take the key in OPENALEX_API_KEY".into(),
        LiteratureSource::SemanticScholar if busy && keys.semantic_scholar.is_none() => {
            "Semantic Scholar's pool for callers without a key is busy: a free key, in SEMANTIC_SCHOLAR_API_KEY, gives Quark its own".into()
        }
        LiteratureSource::SemanticScholar if refused && keys.semantic_scholar.is_some() => "Semantic Scholar did not take the key in SEMANTIC_SCHOLAR_API_KEY".into(),
        _ => failure.to_string(),
    }
}

/// What a lookup found, what nobody has, and what could not be asked about.
type Resolved = (Vec<(Ident, Record)>, Vec<Ident>, Vec<(Ident, String)>);

/// Every name looked up: arXiv's ids in arXiv, DOIs in the chosen sources
/// and then Crossref, and a DOI none of them has in the DOI system itself —
/// a dataset's DOI is real though Crossref never heard of it. `None` when
/// cancelled.
fn lookup(http: &Http, keys: &Keys, idents: &[Ident], sources: &[LiteratureSource], stop: &AtomicBool) -> Option<Resolved> {
    let mut unique: Vec<Ident> = Vec::new();
    for ident in idents {
        if !matches!(ident, Ident::Paper(_)) && !unique.iter().any(|u| u.key() == ident.key()) {
            unique.push(ident.clone());
        }
    }
    let wants = |s: LiteratureSource| sources.contains(&s);
    let mut found = Vec::new();
    let mut missing = Vec::new();
    let mut failures = Vec::new();

    let ids: Vec<String> = unique.iter().filter_map(|i| if let Ident::Arxiv(id) = i { Some(id.clone()) } else { None }).collect();
    let from_arxiv = if ids.is_empty() { Ok(Vec::new()) } else { arxiv::lookup(http, &ids, stop) };
    if matches!(from_arxiv, Err(Failure::Cancelled)) {
        return None;
    }

    for ident in unique {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        match &ident {
            Ident::Arxiv(id) => match &from_arxiv {
                Err(failure) => failures.push((ident.clone(), failure.to_string())),
                Ok(records) => {
                    let Some(record) = records.iter().find(|r| r.arxiv.as_deref() == Some(id.as_str())) else {
                        missing.push(ident);
                        continue;
                    };
                    let mut said = vec![record.clone()];
                    if wants(LiteratureSource::SemanticScholar) {
                        if let Ok(Some(more)) = s2::lookup(http, &ident, keys.semantic_scholar.as_deref(), stop) {
                            said.push(more);
                        }
                    }
                    found.push((ident, combine(&said.iter().collect::<Vec<_>>())));
                }
            },
            Ident::Doi(doi) => {
                let mut said: Vec<Record> = Vec::new();
                let mut failed: Option<String> = None;
                let note = |answer: Result<Option<Record>, Failure>, said: &mut Vec<Record>, failed: &mut Option<String>| match answer {
                    Ok(Some(record)) => said.push(record),
                    Ok(None) => {}
                    Err(failure) => {
                        failed.get_or_insert(failure.to_string());
                    }
                };
                if wants(LiteratureSource::SemanticScholar) {
                    note(s2::lookup(http, &ident, keys.semantic_scholar.as_deref(), stop), &mut said, &mut failed);
                }
                if wants(LiteratureSource::OpenAlex) {
                    note(openalex::lookup_doi(http, doi, keys.openalex.as_deref(), stop), &mut said, &mut failed);
                }
                if said.is_empty() {
                    note(crossref::lookup(http, doi, stop), &mut said, &mut failed);
                }
                if stop.load(Ordering::Relaxed) {
                    return None;
                }
                if !said.is_empty() {
                    found.push((ident, combine(&said.iter().collect::<Vec<_>>())));
                    continue;
                }
                match crossref::registered(http, doi, stop) {
                    Ok(true) => {
                        let bare = Record { doi: Some(doi.clone()), url: ident.url(), ..Record::default() };
                        found.push((ident, bare));
                    }
                    Ok(false) => missing.push(ident),
                    Err(Failure::Cancelled) => return None,
                    Err(failure) => failures.push((ident, failed.unwrap_or_else(|| failure.to_string()))),
                }
            }
            Ident::Paper(_) => {}
        }
    }
    Some((found, missing, failures))
}

/// One PDF kept from the first of its links that gives one.
fn keep(http: &Http, item: &Download, cap: u64, stop: &AtomicBool) -> Reply {
    let mut whys: Vec<String> = Vec::new();
    for url in &item.urls {
        match pdf::download(http, url, &item.dest, cap, stop) {
            Ok(saved) => return Reply::Saved { key: item.key.clone(), bytes: saved.bytes, sha256: saved.sha256, url: url.clone() },
            Err(why) => {
                if !whys.contains(&why) {
                    whys.push(why);
                }
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }
    let why = if whys.is_empty() { "there is no open copy to download".to_string() } else { whys.join("; ") };
    Reply::Unsaved { key: item.key.clone(), why }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Answer, FakeClock, Gates, Script};
    use std::time::{Duration, Instant};

    /// A transport that answers by the URL asked, for workers whose sources
    /// are asked in no fixed order.
    struct Routes {
        routes: Vec<(&'static str, Answer)>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl crate::http::Transport for Routes {
        fn get(&self, request: &crate::http::Request) -> Result<crate::http::Response, String> {
            self.asked.lock().unwrap().push(request.url.clone());
            let answer = self.routes.iter().find(|(part, _)| request.url.contains(part)).map(|(_, a)| a.clone()).unwrap_or(Ok((404, Vec::new(), Vec::new())));
            let (status, headers, body) = answer?;
            Ok(crate::http::Response { status, headers, body: Box::new(std::io::Cursor::new(body)) })
        }
    }

    fn context(routes: Vec<(&'static str, Answer)>, keys: Keys) -> (Context, Arc<Routes>) {
        let routes = Arc::new(Routes { routes, asked: Default::default() });
        let http = Http::new(routes.clone(), Arc::new(Gates::default()), Arc::new(FakeClock::default()), None);
        (Context::new(http, move || keys.clone()), routes)
    }

    fn wait(fetch: &mut Fetch) -> Vec<Reply> {
        let until = Instant::now() + Duration::from_secs(20);
        let mut out = Vec::new();
        while !fetch.is_done() && Instant::now() < until {
            out.extend(fetch.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(fetch.is_done(), "the worker never finished: {out:?}");
        out
    }

    const ATOM: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom"><entry>
        <id>http://arxiv.org/abs/2101.00001v2</id><published>2021-01-01T00:00:00Z</published>
        <title>Why the sky is blue</title><summary>Rayleigh, with commas.</summary><author><name>A. Author</name></author></entry></feed>"#;

    fn ok(body: &str) -> Answer {
        Ok((200, Vec::new(), body.as_bytes().to_vec()))
    }

    #[test]
    fn a_search_asks_every_source_merges_what_they_say_and_says_who_failed() {
        let s2 = r#"{"data":[{"paperId":"abc","title":"Why the Sky is Blue","year":2021,"venue":"Optics","externalIds":{"ArXiv":"2101.00001"},"citationCount":12,
            "isOpenAccess":true,"openAccessPdf":{"url":"https://arxiv.org/pdf/2101.00001"}}]}"#;
        let (ctx, routes) = context(
            vec![("export.arxiv.org", ok(ATOM)), ("api.semanticscholar.org", ok(s2)), ("api.openalex.org", Ok((429, Vec::new(), Vec::new())))],
            Keys::default(),
        );
        let job = Job::Search {
            queries: vec!["sky blue".into(), " sky blue ".into(), "".into()],
            sources: vec![LiteratureSource::Arxiv, LiteratureSource::SemanticScholar, LiteratureSource::OpenAlex],
            per_query: 5,
        };
        let replies = wait(&mut Fetch::start(&ctx, job));
        let progress = replies.iter().filter(|r| matches!(r, Reply::Progress { .. })).count();
        assert_eq!(progress, 2, "one query, asked once of each source that answered: {replies:?}");
        let Some(Reply::Searched { candidates, failures }) = replies.last() else { panic!("{replies:?}") };
        assert_eq!(candidates.len(), 1, "arXiv's paper and Semantic Scholar's are one: {candidates:#?}");
        let r = &candidates[0].record;
        assert_eq!((r.abstract_.as_str(), r.venue.as_deref(), r.citations), ("Rayleigh, with commas.", Some("Optics"), Some(12)));
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].source, LiteratureSource::OpenAlex);
        assert!(failures[0].exhausted && failures[0].why.contains("OPENALEX_API_KEY"), "{failures:?}");
        let asked_openalex = routes.asked.lock().unwrap().iter().filter(|u| u.contains("openalex")).count();
        assert_eq!(asked_openalex, 4, "asked, and three times more after it said it was busy, then left alone");
    }

    #[test]
    fn names_are_found_checked_or_said_to_be_nobodys() {
        let crossref = r#"{"status":"ok","message":{"DOI":"10.1364/josaa.18.000412","title":["Skylight in Granada"],"issued":{"date-parts":[[2001]]}}}"#;
        let (ctx, _) = context(
            vec![
                ("export.arxiv.org", ok(ATOM)),
                ("api.crossref.org/works/10.1364", ok(crossref)),
                ("doi.org/api/handles/10.5281", ok(r#"{"responseCode":1}"#)),
                ("doi.org/api/handles/10.9999", Ok((404, Vec::new(), br#"{"responseCode":100}"#.to_vec()))),
            ],
            Keys::default(),
        );
        let idents = ["arXiv:2101.00001", "arXiv:2101.99999", "10.1364/JOSAA.18.000412", "10.5281/zenodo.42", "10.9999/made.up", "P003", "arXiv:2101.00001v2"]
            .iter()
            .map(|t| Ident::parse(t).unwrap())
            .collect();
        let replies = wait(&mut Fetch::start(&ctx, Job::Lookup { idents, sources: Vec::new() }));
        let [Reply::Resolved { found, missing, failures }] = replies.as_slice() else { panic!("{replies:?}") };
        let titles: Vec<(String, String)> = found.iter().map(|(i, r)| (i.key(), r.title.clone())).collect();
        assert_eq!(
            titles,
            [
                ("arxiv:2101.00001".to_string(), "Why the sky is blue".to_string()),
                ("doi:10.1364/josaa.18.000412".to_string(), "Skylight in Granada".to_string()),
                ("doi:10.5281/zenodo.42".to_string(), String::new()),
            ],
            "a DOI Crossref has not heard of is still real when the DOI system has it"
        );
        let missing: Vec<String> = missing.iter().map(Ident::key).collect();
        assert_eq!(missing, ["arxiv:2101.99999", "doi:10.9999/made.up"]);
        assert!(failures.is_empty(), "{failures:?}");

        let (gone, _) = context(vec![("api.crossref.org", Err("refused".into())), ("doi.org", Err("refused".into()))], Keys::default());
        let replies = wait(&mut Fetch::start(&gone, Job::Lookup { idents: vec![Ident::parse("10.1000/x").unwrap()], sources: Vec::new() }));
        let [Reply::Resolved { found, missing, failures }] = replies.as_slice() else { panic!("{replies:?}") };
        assert!(found.is_empty() && missing.is_empty(), "unreachable is not made up");
        assert_eq!(failures.len(), 1);
    }

    #[test]
    fn pdfs_are_kept_from_the_first_link_that_gives_one() {
        let dir = std::env::temp_dir().join(format!("quark-literature-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (ctx, _) = context(
            vec![("publisher.example.org", ok("<!doctype html><title>Sign in</title>")), ("arxiv.org/pdf", Ok((200, Vec::new(), b"%PDF-1.5 paper".to_vec())))],
            Keys::default(),
        );
        let items = vec![
            Download { key: "P001".into(), urls: vec!["https://publisher.example.org/paper.pdf".into(), "https://arxiv.org/pdf/2101.00001".into()], dest: dir.join("P001.pdf") },
            Download { key: "P002".into(), urls: vec!["https://publisher.example.org/other.pdf".into()], dest: dir.join("P002.pdf") },
            Download { key: "P003".into(), urls: Vec::new(), dest: dir.join("P003.pdf") },
        ];
        let replies = wait(&mut Fetch::start(&ctx, Job::Download { items, cap: 1024 }));
        assert!(matches!(&replies[0], Reply::Saved { key, url, bytes: 14, .. } if key == "P001" && url.contains("arxiv.org")), "{replies:?}");
        assert!(matches!(&replies[1], Reply::Unsaved { key, why } if key == "P002" && why.contains("web page")), "{replies:?}");
        assert!(matches!(&replies[2], Reply::Unsaved { key, why } if key == "P003" && why.contains("no open copy")), "{replies:?}");
        assert_eq!(replies[3], Reply::Finished);
        assert!(dir.join("P001.pdf").is_file() && !dir.join("P002.pdf").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cancelled_job_says_nothing_more_and_a_dead_one_says_it_died() {
        // A transport that waits until the job is cancelled, then answers.
        struct Slow(Arc<AtomicBool>);
        impl crate::http::Transport for Slow {
            fn get(&self, _: &crate::http::Request) -> Result<crate::http::Response, String> {
                while !self.0.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(crate::http::Response { status: 200, headers: Vec::new(), body: Box::new(std::io::Cursor::new(ATOM.as_bytes().to_vec())) })
            }
        }
        let release = Arc::new(AtomicBool::new(false));
        let http = Http::new(Arc::new(Slow(release.clone())), Arc::new(Gates::default()), Arc::new(FakeClock::default()), None);
        let ctx = Context::new(http, Keys::default);
        let mut fetch = Fetch::start(&ctx, Job::Search { queries: vec!["sky".into()], sources: vec![LiteratureSource::Arxiv], per_query: 3 });
        fetch.cancel();
        release.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(100));
        let replies = fetch.poll();
        assert!(replies.iter().all(|r| !matches!(r, Reply::Searched { .. })), "{replies:?}");

        let mut died = Fetch::scripted(vec![Reply::Progress { source: LiteratureSource::Arxiv, query: "sky".into(), found: 1 }]);
        let replies = died.poll();
        assert!(matches!(replies.last(), Some(Reply::Failed { why }) if why.contains("stopped")), "{replies:?}");
        assert!(died.is_done() && died.poll().is_empty());

        let panicking = Context::new(Http::new(Arc::new(Script::new(Arc::new(FakeClock::default()), Vec::new())), Arc::new(Gates::default()), Arc::new(FakeClock::default()), None), || {
            panic!("no keys today")
        });
        let replies = wait(&mut Fetch::start(&panicking, Job::Download { items: Vec::new(), cap: 1 }));
        assert!(matches!(replies.as_slice(), [Reply::Failed { why }] if why.contains("no keys today")), "{replies:?}");

        let mut pending = Fetch::pending();
        assert!(pending.poll().is_empty() && !pending.is_done(), "a job that never answers is not over");
    }
}
