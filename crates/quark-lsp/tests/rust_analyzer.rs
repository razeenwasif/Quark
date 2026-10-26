//! The one test that talks to a real server.
//!
//! Everything else in this crate is exercised in memory, which catches framing
//! and parsing but cannot catch the thing that actually goes wrong: a
//! handshake a real server does not accept, a capability we forgot to claim,
//! a request answered too early to mean anything. So this spawns
//! rust-analyzer against a throwaway crate and asks it the two questions M2
//! exists to answer.
//!
//! Skipped, loudly, when rust-analyzer is not installed — a contributor
//! without it should not see a red suite.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quark_lsp::client::Incoming;
use quark_lsp::{Client, Position};

const FIXTURE: &str = r#"pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn magnitude(&self) -> i32 {
        self.x + self.y
    }
}

pub fn describe(p: Point) -> i32 {
    p.magnitude()
}
"#;

/// Where a substring sits, in the line/character pair LSP wants.
fn at(text: &str, needle: &str, past: u32) -> Position {
    let index = text.find(needle).expect("fixture changed under the test");
    let before = &text[..index];
    Position {
        line: before.matches('\n').count() as u32,
        character: before.rsplit('\n').next().unwrap().chars().count() as u32 + past,
    }
}

struct Fixture {
    root: PathBuf,
    file: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        // A crate with no dependencies: rust-analyzer still loads the sysroot,
        // but `cargo metadata` returns instantly, which is most of the wait.
        let root = std::env::temp_dir().join(format!("quark-lsp-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let file = src.join("lib.rs");
        std::fs::write(&file, FIXTURE).unwrap();
        Self { root, file }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn installed() -> bool {
    let probe = std::process::Command::new("rust-analyzer")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    matches!(probe, Ok(status) if status.success())
}

/// Poll until `want` finds what it is looking for. Everything else that
/// arrives is kept, so a failure can print what the server *did* say.
fn wait<T>(
    client: &mut Client,
    what: &str,
    limit: Duration,
    mut want: impl FnMut(&Incoming) -> Option<T>,
) -> T {
    let deadline = Instant::now() + limit;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        for message in client.poll() {
            if let Some(found) = want(&message) {
                return found;
            }
            if let Incoming::Failed { text } = &message {
                panic!("server refused while waiting for {what}: {text}");
            }
            seen.push(message);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}; server said: {seen:#?}");
}

fn start(root: &Path) -> Client {
    let mut client = Client::spawn("rust", "rust-analyzer", &[], root).expect("spawn");
    let server = wait(
        &mut client,
        "initialize",
        Duration::from_secs(30),
        |m| match m {
            // rust-analyzer does all three, and a real handshake is the only
            // place that claim can be checked against the server rather than
            // against a fixture of what we think it says.
            Incoming::Ready { server, can } => {
                assert!(can.format, "rust-analyzer should offer formatting");
                assert!(can.rename, "and renaming");
                assert!(can.code_action, "and code actions");
                Some(server.clone())
            }
            _ => None,
        },
    );
    assert!(
        client.is_ready(),
        "Ready must flip the flag the queue waits on"
    );
    println!("connected to {server}");
    client
}

#[test]
#[ignore = "spawns rust-analyzer; run with --ignored or via `cargo test -- --include-ignored`"]
fn rust_analyzer_answers_completion_and_go_to_definition() {
    if !installed() {
        println!("skipping: rust-analyzer is not on PATH");
        return;
    }

    let fixture = Fixture::new();
    let mut client = start(&fixture.root);
    client.did_open(&fixture.file, "rust", 1, FIXTURE);

    // Completion just after the `.` in `p.magnitude()`. Retried rather than
    // asked once: for the first few seconds rust-analyzer is still loading the
    // sysroot and will honestly answer "nothing", which is not the same as
    // being wrong.
    let dot = at(FIXTURE, "p.magnitude()", 2);
    let items = retry(&mut client, "completion", |client| {
        let id = client.completion(&fixture.file, dot);
        collect(client, Duration::from_secs(5), |m| match m {
            Incoming::Completion { id: got, items } if *got == id => Some(items.clone()),
            _ => None,
        })
        .filter(|items: &Vec<_>| !items.is_empty())
    });

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    let found = items
        .iter()
        .find(|i| i.label == "magnitude")
        .unwrap_or_else(|| panic!("completion after `p.` should offer magnitude, got {labels:?}"));

    // The label alone would pass even if every optional field silently failed
    // to parse, so check the two that decide what actually lands in the
    // buffer. A server that sends a `textEdit` is telling us the span to
    // replace; ignoring it is how you get `p.magmagnitude`.
    let (range, text) = found.edit();
    assert_eq!(
        text, "magnitude",
        "the insertion should be plain, not a snippet"
    );
    let range = range.expect("rust-analyzer sends a textEdit; if this is None, parsing dropped it");
    assert_eq!(range.start.line, dot.line);
    assert!(
        found.hint().is_some_and(|h| h.contains("i32")),
        "the hint should carry the signature, got {:?}",
        found.hint()
    );

    // Go to definition on the call should land on the declaration.
    let call = at(FIXTURE, "p.magnitude()", 3);
    let jump = retry(&mut client, "definition", |client| {
        let id = client.definition(&fixture.file, call);
        collect(client, Duration::from_secs(5), |m| match m {
            Incoming::Definition { id: got, target } if *got == id => target.clone(),
            _ => None,
        })
    });
    assert_eq!(
        jump.path.canonicalize().unwrap(),
        fixture.file.canonicalize().unwrap()
    );
    assert_eq!(
        jump.line,
        at(FIXTURE, "pub fn magnitude", 0).line,
        "should land on the declaration, not the call"
    );
}

/// Drain for a while, looking for one thing. `None` if it did not arrive.
fn collect<T>(
    client: &mut Client,
    limit: Duration,
    mut want: impl FnMut(&Incoming) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        for message in client.poll() {
            if let Some(found) = want(&message) {
                return Some(found);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Ask again until the server has loaded enough to have an answer.
fn retry<T>(client: &mut Client, what: &str, mut ask: impl FnMut(&mut Client) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if let Some(found) = ask(client) {
            return found;
        }
        assert!(
            !client.has_exited(),
            "rust-analyzer died while we waited for {what}"
        );
    }
    panic!("rust-analyzer never answered {what}");
}
