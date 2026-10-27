//! Asking the sources, politely: each host asked no oftener than it asks to
//! be, one waiting turn at a time across the whole of Quark; asked again
//! when it says it is busy, and only then, for as long as it says; with a
//! name that says who is asking; and never with a key in an error message.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The longest an API answer may take, connecting included.
pub const API_TIMEOUT: Duration = Duration::from_secs(20);

/// The most of an API answer read: generous for a page of results.
pub const API_LIMIT: u64 = 16 * 1024 * 1024;

/// What a cancelled request says.
pub const CANCELLED: &str = "cancelled";

/// A GET as the sources are asked.
#[derive(Debug, Clone)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The whole exchange, connecting included.
    pub timeout: Duration,
    /// The most of the body read.
    pub limit: u64,
}

/// An answer: its status, its headers, and its body to be read.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl Response {
    /// A header's value, by name in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// What carries a request: ureq over the network, or a script in a test.
pub trait Transport: Send + Sync {
    fn get(&self, request: &Request) -> Result<Response, String>;
}

/// The network, over rustls with the platform's own certificates — as the
/// agent's client is, so a proxy that re-signs TLS does not stop it — https
/// only, redirects followed a few times.
pub struct Ureq {
    agent: ureq::Agent,
}

impl Default for Ureq {
    fn default() -> Ureq {
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .https_only(true)
            .tls_config(tls)
            .timeout_connect(Some(Duration::from_secs(10)))
            .max_redirects(5)
            .build()
            .into();
        Ureq { agent }
    }
}

impl Transport for Ureq {
    fn get(&self, request: &Request) -> Result<Response, String> {
        let mut builder = self.agent.get(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = builder.config().timeout_global(Some(request.timeout)).build().call().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let headers = response.headers().iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
        let body = response.into_body().into_with_config().limit(request.limit).reader();
        Ok(Response { status, headers, body: Box::new(body) })
    }
}

/// The time, as the gates keep it: the clock on the wall, or a test's.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, d: Duration);
}

pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// A clock that moves only when slept on, for tests that would otherwise
/// wait out arXiv's three seconds.
pub struct FakeClock {
    now: Mutex<Instant>,
}

impl Default for FakeClock {
    fn default() -> FakeClock {
        FakeClock { now: Mutex::new(Instant::now()) }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn sleep(&self, d: Duration) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) += d;
    }
}

/// How long to leave between two requests to a host, as each asks: arXiv
/// one every three seconds; Semantic Scholar one a second with a key, and
/// gentler on its shared pool without; OpenAlex and Crossref a few a second.
pub fn spacing(host: &str, keyed: bool) -> Duration {
    match host {
        "export.arxiv.org" | "arxiv.org" => Duration::from_secs(3),
        "api.semanticscholar.org" if keyed => Duration::from_secs(1),
        "api.semanticscholar.org" => Duration::from_secs(3),
        "api.openalex.org" => Duration::from_millis(200),
        "api.crossref.org" => Duration::from_millis(250),
        _ => Duration::from_secs(1),
    }
}

/// When each host may next be asked. Process-wide, so two searches at once
/// still ask arXiv once every three seconds between them.
#[derive(Default)]
pub struct Gates {
    next: Mutex<HashMap<String, Instant>>,
}

impl Gates {
    /// The gates Quark's live requests share.
    pub fn shared() -> Arc<Gates> {
        static GATES: OnceLock<Arc<Gates>> = OnceLock::new();
        GATES.get_or_init(Arc::default).clone()
    }

    /// Wait for `host`'s turn — `spacing` after the last one's — or until
    /// cancelled, which answers `false`.
    pub fn wait(&self, host: &str, spacing: Duration, clock: &dyn Clock, cancel: &AtomicBool) -> bool {
        let at = {
            let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
            let now = clock.now();
            let at = next.get(host).copied().filter(|t| *t > now).unwrap_or(now);
            next.insert(host.to_string(), at + spacing);
            at
        };
        sleep_until(at, clock, cancel)
    }
}

/// Sleep until `at` in short steps, so a cancel is heard; `false` if it came.
fn sleep_until(at: Instant, clock: &dyn Clock, cancel: &AtomicBool) -> bool {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let now = clock.now();
        if now >= at {
            return true;
        }
        clock.sleep((at - now).min(Duration::from_millis(100)));
    }
}

/// Why a source could not be asked, or did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// It has nothing by that name.
    NotFound,
    /// It answered, but not with what was asked: its status.
    Status { host: String, status: u16 },
    /// It could not be reached.
    Network { host: String, why: String },
    /// It answered with something that will not read.
    Unreadable { host: String, why: String },
    /// It asked to be left for longer than is worth waiting.
    Busy { host: String, seconds: u64 },
    Cancelled,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::NotFound => write!(f, "not found"),
            Failure::Status { host, status: 429 } => write!(f, "{host} is answering too many requests just now"),
            Failure::Status { host, status } => write!(f, "{host} answered {status}"),
            Failure::Network { host, why } => write!(f, "{host} could not be reached: {why}"),
            Failure::Unreadable { host, why } => write!(f, "{host} answered with something that will not read: {why}"),
            Failure::Busy { host, seconds } => write!(f, "{host} asks to be left for {seconds} s"),
            Failure::Cancelled => write!(f, "{CANCELLED}"),
        }
    }
}

/// The client every source is asked through.
#[derive(Clone)]
pub struct Http {
    transport: Arc<dyn Transport>,
    gates: Arc<Gates>,
    clock: Arc<dyn Clock>,
    agent: String,
}

impl Http {
    /// Over `transport`, waiting at `gates` by `clock`. `mailto`, when the
    /// person has given one, goes in the name it asks under — the polite
    /// pools some sources keep are for callers they can write to.
    pub fn new(transport: Arc<dyn Transport>, gates: Arc<Gates>, clock: Arc<dyn Clock>, mailto: Option<&str>) -> Http {
        Http { transport, gates, clock, agent: String::new() }.with_contact(mailto)
    }

    /// The same client, asking under a name with `mailto` in it — or without one.
    pub fn with_contact(&self, mailto: Option<&str>) -> Http {
        let contact = mailto.filter(|m| !m.trim().is_empty()).map(|m| format!("; mailto:{}", m.trim())).unwrap_or_default();
        Http { agent: format!("Quark/{} (+https://github.com/razeenwasif/Quark{contact})", env!("CARGO_PKG_VERSION")), ..self.clone() }
    }

    /// The network, with the gates every live request shares.
    pub fn live(mailto: Option<&str>) -> Http {
        Http::new(Arc::new(Ureq::default()), Gates::shared(), Arc::new(RealClock), mailto)
    }

    /// The name it asks under.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// GET `url`: at the host's turn, asked again up to three times when it
    /// says it is busy (429, 502, 503, 504) — after its `Retry-After`, or
    /// one, two, four seconds — and once more when it could not be reached.
    /// `keyed` says the request carries a key, for the host's spacing.
    pub fn get(&self, url: &str, headers: &[(&str, &str)], timeout: Duration, limit: u64, keyed: bool, cancel: &AtomicBool) -> Result<Response, Failure> {
        let host = host_of(url);
        let spacing = spacing(&host, keyed);
        // arXiv's papers and its API are one arXiv to wait on.
        let gate = if host.ends_with("arxiv.org") { "arxiv.org" } else { host.as_str() };
        let mut headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        headers.push(("User-Agent".into(), self.agent.clone()));
        let request = Request { url: url.to_string(), headers, timeout, limit };
        let (mut busy, mut unreachable) = (0u32, 0u32);
        loop {
            if !self.gates.wait(gate, spacing, &*self.clock, cancel) {
                return Err(Failure::Cancelled);
            }
            match self.transport.get(&request) {
                Ok(response) if matches!(response.status, 429 | 502 | 503 | 504) && busy < 3 => {
                    let wait = response.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs);
                    let wait = wait.unwrap_or(Duration::from_secs(1 << busy));
                    if wait > Duration::from_secs(60) {
                        return Err(Failure::Busy { host, seconds: wait.as_secs() });
                    }
                    busy += 1;
                    if !sleep_until(self.clock.now() + wait, &*self.clock, cancel) {
                        return Err(Failure::Cancelled);
                    }
                }
                Ok(response) => return Ok(response),
                Err(_) if unreachable < 1 && !cancel.load(Ordering::Relaxed) => unreachable += 1,
                Err(why) => return Err(Failure::Network { why: scrub(&why), host }),
            }
        }
    }

    /// GET an answer as text: anything but 200 is a failure, 404 is "not found".
    pub fn text(&self, url: &str, headers: &[(&str, &str)], keyed: bool, cancel: &AtomicBool) -> Result<String, Failure> {
        let host = host_of(url);
        let mut response = self.get(url, headers, API_TIMEOUT, API_LIMIT, keyed, cancel)?;
        match response.status {
            200 => {}
            404 => return Err(Failure::NotFound),
            status => return Err(Failure::Status { host, status }),
        }
        let mut text = String::new();
        response.body.read_to_string(&mut text).map_err(|e| Failure::Network { host, why: scrub(&e.to_string()) })?;
        Ok(text)
    }

    /// GET an answer as JSON.
    pub fn json(&self, url: &str, headers: &[(&str, &str)], keyed: bool, cancel: &AtomicBool) -> Result<serde_json::Value, Failure> {
        let host = host_of(url);
        let text = self.text(url, headers, keyed, cancel)?;
        serde_json::from_str(&text).map_err(|e| Failure::Unreadable { host, why: e.to_string() })
    }
}

/// The host a URL names.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase()
}

/// Text with any `api_key=` or `mailto=` value cut out of it: an error
/// message that quotes a URL must not carry the person's key or address.
pub fn scrub(text: &str) -> String {
    let mut out = text.to_string();
    for name in ["api_key=", "mailto="] {
        let mut from = 0;
        while let Some(at) = out[from..].find(name).map(|i| from + i + name.len()) {
            let end = out[at..].find(|c: char| c == '&' || c == ')' || c.is_whitespace()).map_or(out.len(), |i| at + i);
            out.replace_range(at..end, "…");
            from = at + '…'.len_utf8();
        }
    }
    out
}

/// A query parameter's value, percent-encoded.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A DOI or an id as part of a URL's path: what would end the path or break
/// the URL encoded, its slashes and colons kept — the sources read
/// `/paper/DOI:10.1364/JOSAA.18.000412` as written.
pub fn encode_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            ' ' | '#' | '?' | '%' | '<' | '>' | '"' | '{' | '}' | '|' | '\\' | '^' | '`' | '[' | ']' => out.push_str(&format!("%{:02X}", c as u32)),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                let mut buffer = [0u8; 4];
                for byte in c.encode_utf8(&mut buffer).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) use tests::{Answer, Script};

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted answer: a status, headers and a body — or a network error.
    pub(crate) type Answer = Result<(u16, Vec<(String, String)>, Vec<u8>), String>;

    /// Answers each request with the next of `script`, and remembers what was asked and when.
    pub(crate) struct Script {
        answers: Mutex<Vec<Answer>>,
        pub asked: Mutex<Vec<(Request, Instant)>>,
        clock: Arc<FakeClock>,
    }

    impl Script {
        pub(crate) fn new(clock: Arc<FakeClock>, answers: Vec<Answer>) -> Script {
            Script { answers: Mutex::new(answers.into_iter().rev().collect()), asked: Mutex::new(Vec::new()), clock }
        }
    }

    impl Transport for Script {
        fn get(&self, request: &Request) -> Result<Response, String> {
            self.asked.lock().unwrap().push((request.clone(), self.clock.now()));
            let (status, headers, body) = self.answers.lock().unwrap().pop().unwrap_or(Ok((200, Vec::new(), Vec::new())))?;
            Ok(Response { status, headers, body: Box::new(std::io::Cursor::new(body)) })
        }
    }

    fn http(script: Arc<Script>, clock: Arc<FakeClock>, mailto: Option<&str>) -> Http {
        Http::new(script, Arc::new(Gates::default()), clock, mailto)
    }

    #[test]
    fn a_host_is_asked_no_oftener_than_it_asks() {
        let clock = Arc::new(FakeClock::default());
        let script = Arc::new(Script::new(clock.clone(), Vec::new()));
        let http = http(script.clone(), clock.clone(), None);
        let cancel = AtomicBool::new(false);
        for _ in 0..3 {
            http.text("https://export.arxiv.org/api/query?search_query=all:sky", &[], false, &cancel).unwrap();
        }
        http.text("https://api.crossref.org/works/10.1/x", &[], false, &cancel).unwrap();
        http.text("https://arxiv.org/pdf/1706.03762", &[], false, &cancel).unwrap();
        let asked = script.asked.lock().unwrap();
        let gaps: Vec<Duration> = asked.windows(2).take(2).map(|w| w[1].1 - w[0].1).collect();
        assert!(gaps.iter().all(|g| *g >= Duration::from_secs(3)), "arXiv asked sooner than every 3 s: {gaps:?}");
        assert_eq!(asked[3].1, asked[2].1, "another host does not wait on arXiv's turn");
        assert!(asked[4].1 - asked[2].1 >= Duration::from_secs(3), "a PDF from arXiv waits on its API's turn");
        assert!(asked[0].0.headers.iter().any(|(k, v)| k == "User-Agent" && v.starts_with("Quark/") && !v.contains("mailto")));
    }

    #[test]
    fn a_busy_host_is_asked_again_after_it_says_and_a_gone_one_once() {
        let clock = Arc::new(FakeClock::default());
        let busy = |after: &str| Ok((429, vec![("Retry-After".to_string(), after.to_string())], Vec::new()));
        let script = Arc::new(Script::new(clock.clone(), vec![busy("2"), Ok((200, Vec::new(), b"ok".to_vec()))]));
        let http = http(script.clone(), clock.clone(), None);
        let cancel = AtomicBool::new(false);
        assert_eq!(http.text("https://api.openalex.org/works?search=sky", &[], false, &cancel).unwrap(), "ok");
        {
            let asked = script.asked.lock().unwrap();
            assert_eq!(asked.len(), 2);
            assert!(asked[1].1 - asked[0].1 >= Duration::from_secs(2), "Retry-After was not waited out");
        }

        let script = Arc::new(Script::new(clock.clone(), vec![Ok((503, Vec::new(), Vec::new())); 4]));
        let failure = super::Http::new(script.clone(), Arc::new(Gates::default()), clock.clone(), None)
            .text("https://api.crossref.org/works/10.1/x", &[], false, &cancel)
            .unwrap_err();
        assert_eq!(failure, Failure::Status { host: "api.crossref.org".into(), status: 503 });
        assert_eq!(script.asked.lock().unwrap().len(), 4, "three times more, and no more");

        let script = Arc::new(Script::new(clock.clone(), vec![busy("600")]));
        let failure = super::Http::new(script, Arc::new(Gates::default()), clock.clone(), None).text("https://api.openalex.org/works", &[], false, &cancel).unwrap_err();
        assert_eq!(failure, Failure::Busy { host: "api.openalex.org".into(), seconds: 600 }, "ten minutes is not worth waiting");

        let script = Arc::new(Script::new(clock.clone(), vec![Err("reset".into()), Err("connection refused for https://api.openalex.org/works?api_key=SECRET&search=x".into())]));
        let failure = super::Http::new(script.clone(), Arc::new(Gates::default()), clock.clone(), None)
            .text("https://api.openalex.org/works?api_key=SECRET", &[], false, &cancel)
            .unwrap_err();
        assert_eq!(script.asked.lock().unwrap().len(), 2, "asked once more after a network error");
        assert!(!failure.to_string().contains("SECRET"), "{failure}");
        let missing = Arc::new(Script::new(clock.clone(), vec![Ok((404, Vec::new(), Vec::new()))]));
        assert_eq!(super::Http::new(missing, Arc::new(Gates::default()), clock, None).text("https://api.crossref.org/works/x", &[], false, &cancel), Err(Failure::NotFound));
    }

    #[test]
    fn a_cancel_is_heard_while_waiting_a_turn() {
        let clock = Arc::new(FakeClock::default());
        let gates = Gates::default();
        let cancel = AtomicBool::new(false);
        assert!(gates.wait("export.arxiv.org", Duration::from_secs(3), &*clock, &cancel));
        cancel.store(true, Ordering::Relaxed);
        assert!(!gates.wait("export.arxiv.org", Duration::from_secs(3), &*clock, &cancel));
    }

    #[test]
    fn keys_and_addresses_never_reach_a_message_and_a_contact_is_said_only_when_given() {
        assert_eq!(scrub("GET https://api.openalex.org/works?api_key=abc123&search=sky failed"), "GET https://api.openalex.org/works?api_key=…&search=sky failed");
        assert_eq!(scrub("mailto=me@example.org) and api_key=k"), "mailto=…) and api_key=…");
        assert_eq!(host_of("https://api.semanticscholar.org:443/graph/v1"), "api.semanticscholar.org");
        assert_eq!(encode("rayleigh \"sky\" & ozone"), "rayleigh%20%22sky%22%20%26%20ozone");
        assert_eq!(encode_path("DOI:10.1175/1520-0426(1999)016<1854:ORODC>2.0.CO;2"), "DOI:10.1175/1520-0426(1999)016%3C1854:ORODC%3E2.0.CO;2");
        let clock = Arc::new(FakeClock::default());
        let script = Arc::new(Script::new(clock.clone(), Vec::new()));
        assert!(http(script, clock, Some("scientist@example.org")).agent().ends_with("; mailto:scientist@example.org)"));
    }
}

