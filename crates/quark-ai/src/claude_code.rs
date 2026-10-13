//! Claude Code, as a backend.
//!
//! Runs the `claude` CLI headless (`claude -p`) and reads its `stream-json`
//! output. It answers with whatever account Claude Code is signed into — for
//! most people a Claude subscription — so there is no API key to paste and
//! nothing for Quark to store. Quark never sees the credentials: the CLI owns
//! them, and Quark only ever talks to the CLI.
//!
//! # What the CLI is allowed to do
//!
//! Exactly what [`Access`] grants, and nothing that could prompt: Quark has no
//! way to show Claude Code's permission dialogs, so every tool offered is
//! approved up front and anything else is refused outright rather than left
//! waiting for an answer that will never come.
//!
//! - **Files** — read and search the open document's folder. `--restricted`
//!   confines the file tools to the working directory, which is that folder,
//!   and removes the shell and every other code-running tool.
//! - **Web** — search, and open pages.
//! - **Edits** — create and change files in that same folder. Off unless the
//!   reader turns it on, and never PDFs: the model cannot write a valid one,
//!   so a request to "add a note to this PDF" could only destroy the
//!   document. A deny rule makes every PDF in the folder read-only to it.
//!
//! With nothing granted the CLI gets `--tools ""` and can only answer. Every
//! run also gets `--safe-mode` (none of the user's plugins, hooks, MCP servers
//! or `CLAUDE.md` — a reading question should not inherit a coding setup) and
//! `--no-session-persistence`, so asking about a PDF does not leave sessions
//! behind in the user's Claude Code history.
//!
//! # Why the conversation is flattened
//!
//! `claude -p` takes one prompt. The earlier turns are written into it as a
//! transcript rather than replayed as a resumed session, because the document
//! context changes from turn to turn (the reader moves between pages) and a
//! resumed session would keep answering from the old pages.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::provider::{AiError, ChatRequest, Delta, Provider, Role, StopReason};

/// Models offered in the picker. `default` defers to whatever the user chose
/// in Claude Code itself (`/model`), which is the natural meaning of "use my
/// subscription"; the rest are the CLI's own aliases for the latest of each.
pub const MODELS: &[&str] = &["default", "opus", "sonnet", "haiku", "fable"];

pub const DEFAULT_MODEL: &str = "default";

/// Environment variable that overrides where the CLI is looked for.
pub const PATH_OVERRIDE: &str = "QUARK_CLAUDE";

/// The most tool-using rounds one question may take, so a question that
/// sends the model down a rabbit hole ends rather than running on the plan's
/// usage indefinitely.
const MAX_TURNS: u32 = 24;

/// Variables a parent Claude Code session sets for its own children.
///
/// Stripped from the child so a Quark started from a Claude Code terminal does
/// not hand the CLI a session it does not own. Configuration the user set on
/// purpose (`CLAUDE_CODE_USE_BEDROCK` and friends) is left alone.
const PARENT_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_PID",
];

/// Credentials that would make the CLI bill an API key instead of the
/// subscription. Someone who wants API billing has the Anthropic backend.
const API_KEY_VARS: &[&str] = &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

/// What the CLI may do beyond answering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// Read and search files in [`Self::folder`].
    pub files: bool,
    /// Create and change files in [`Self::folder`]. Implies reading them.
    pub edits: bool,
    /// Search the web and open pages.
    pub web: bool,
    /// The folder the file tools are confined to: the open document's. With
    /// none — an unsaved document — there is nowhere to confine them to, so
    /// file access is withheld whatever the switches say.
    pub folder: Option<PathBuf>,
    /// The open document itself, so the model can be told it may read the
    /// whole file rather than only the pages quoted to it.
    pub document: Option<PathBuf>,
}

impl Access {
    fn files_allowed(&self) -> bool {
        (self.files || self.edits) && self.folder.is_some()
    }

    fn edits_allowed(&self) -> bool {
        self.edits && self.folder.is_some()
    }

    /// The built-in tools this grants, in the CLI's names.
    pub fn tools(&self) -> Vec<&'static str> {
        let mut tools = Vec::new();
        if self.files_allowed() {
            tools.extend(["Read", "Glob", "Grep"]);
        }
        if self.edits_allowed() {
            tools.extend(["Edit", "Write"]);
        }
        if self.web {
            tools.extend(["WebSearch", "WebFetch"]);
        }
        tools
    }

    /// What the model is told about its tools, appended to the system prompt.
    ///
    /// The reading rules in the base prompt assume the quoted pages are all
    /// there is; with tools that stops being true, and saying so is what makes
    /// the model reach for them instead of answering "not in the text".
    pub fn note(&self) -> String {
        if self.tools().is_empty() {
            return String::new();
        }
        let mut s = String::from(
            "\n\n# Tools\nThe reading rules above assume you only have the quoted pages. You \
             also have tools: use them when those pages are not enough, and say where each \
             fact came from.\n",
        );
        if let (true, Some(folder)) = (self.files_allowed(), &self.folder) {
            s.push_str(&format!(
                "- Files: you can read and search files in {}, the document's folder.",
                folder.display()
            ));
            if let Some(name) = self.document.as_ref().and_then(|d| d.file_name()) {
                s.push_str(&format!(
                    " The open document is {}; read it directly for pages not quoted above.",
                    name.to_string_lossy()
                ));
            }
            s.push('\n');
        }
        if self.edits_allowed() {
            s.push_str(
                "- Edits: you can create and change files in that folder. Only do so when the \
                 reader asks, and say exactly which files you changed. PDFs are read-only to \
                 you: to annotate one, write your notes to a separate file.\n",
            );
        }
        if self.web {
            s.push_str("- Web: you can search the web and open pages. Cite each web source by its URL.\n");
        }
        s
    }
}

pub struct ClaudeCode {
    program: PathBuf,
    access: Access,
}

impl ClaudeCode {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            access: Access::default(),
        }
    }

    /// Grants the CLI tools for this run.
    pub fn with_access(mut self, access: Access) -> Self {
        self.access = access;
        self
    }

    /// Finds the `claude` executable.
    ///
    /// Not just `PATH`: an app launched from Finder or the Dock gets a bare
    /// system `PATH` that leaves out `~/.local/bin` and Homebrew, which is
    /// exactly where the CLI installs. So the usual install locations are
    /// tried too, after an explicit override.
    pub fn locate() -> Option<PathBuf> {
        candidates().into_iter().find(|p| is_executable(p))
    }

    /// The arguments for one run. Split out so the flags that bound what the
    /// CLI may do can be asserted without spawning anything.
    pub fn args(model: &str, system_prompt_file: &Path, access: &Access) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-session-persistence",
            "--safe-mode",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let tools = access.tools();
        if tools.is_empty() {
            // The empty list is what disables every tool.
            args.extend(["--tools".into(), String::new()]);
        } else {
            let list = tools.join(",");
            args.extend([
                // File tools stay inside the working directory; the shell and
                // every other code-running tool are removed.
                "--restricted".into(),
                "--tools".into(),
                list.clone(),
                // Approved up front: there is no one to answer a prompt.
                "--allowedTools".into(),
                list,
                // Anything else that would ask is refused, not left hanging.
                "--permission-prompts".into(),
                "none".into(),
                "--max-turns".into(),
                MAX_TURNS.to_string(),
            ]);
            if access.edits_allowed() {
                args.extend([
                    // `Edit(...)` rules cover every file-changing tool,
                    // `Write` included; `**/` reaches into subfolders.
                    "--disallowedTools".into(),
                    "Edit(**/*.pdf)".into(),
                    "Edit(**/*.PDF)".into(),
                    "--permission-mode".into(),
                    "acceptEdits".into(),
                ]);
            }
        }

        // A file rather than `--system-prompt`: with the whole document in
        // scope the prompt can pass the platform's command-line size limit.
        args.push("--system-prompt-file".into());
        args.push(system_prompt_file.to_string_lossy().into_owned());
        let model = model.trim();
        if !model.is_empty() && model != DEFAULT_MODEL {
            args.push("--model".into());
            args.push(model.into());
        }
        args
    }

    /// The prompt sent on stdin: the earlier turns as a transcript, then the
    /// latest question.
    pub fn prompt(req: &ChatRequest) -> String {
        let Some((last, earlier)) = req.messages.split_last() else {
            return String::new();
        };
        if earlier.is_empty() {
            return last.text.clone();
        }
        let mut out = String::from(
            "Earlier in this conversation (for context; answer only the latest message):\n\n<conversation>\n",
        );
        for m in earlier {
            let tag = match m.role {
                Role::User => "reader",
                Role::Assistant => "you",
            };
            out.push_str(&format!("<{tag}>\n{}\n</{tag}>\n", m.text.trim()));
        }
        out.push_str("</conversation>\n\nThe reader's latest message:\n\n");
        out.push_str(&last.text);
        out
    }

    /// Where the CLI runs.
    ///
    /// The document's folder when files are granted — that is what confines
    /// the file tools to it. Otherwise an empty directory of Quark's own, so
    /// the CLI picks up nothing from wherever Quark happened to be launched.
    fn workdir(&self) -> PathBuf {
        match (&self.access.folder, self.access.files_allowed()) {
            (Some(folder), true) => folder.clone(),
            _ => {
                let dir = std::env::temp_dir().join("quark-claude");
                let _ = std::fs::create_dir_all(&dir);
                dir
            }
        }
    }
}

impl Provider for ClaudeCode {
    fn name(&self) -> &str {
        "Claude Code"
    }

    fn models(&self) -> Result<Vec<String>, AiError> {
        Ok(MODELS.iter().map(|s| s.to_string()).collect())
    }

    fn chat(
        &self,
        req: &ChatRequest,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<(), AiError> {
        let system = format!("{}{}", req.system, self.access.note());
        let system = TempFile::write(&system)
            .map_err(|e| AiError::Other(format!("Could not prepare the question: {e}")))?;

        let mut command = Command::new(&self.program);
        command
            .args(Self::args(&req.model, system.path(), &self.access))
            .current_dir(self.workdir())
            .env("PATH", child_path(&self.program))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in PARENT_SESSION_VARS.iter().chain(API_KEY_VARS) {
            command.env_remove(var);
        }

        let mut child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AiError::Other(format!(
                    "Claude Code was not found. Install it from claude.com/claude-code, \
                     or set {PATH_OVERRIDE} to the `claude` executable."
                ))
            } else {
                AiError::Other(format!("Could not start Claude Code: {e}"))
            }
        })?;

        // The prompt goes in and the pipe is closed, which is what tells the
        // CLI the question is complete.
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = Self::prompt(req);
            std::thread::spawn(move || {
                let _ = stdin.write_all(prompt.as_bytes());
            });
        }

        // stderr is drained on its own thread so a chatty CLI cannot fill the
        // pipe and stall, and so a failure can say why.
        let stderr = child.stderr.take().map(|err| {
            std::thread::spawn(move || {
                let mut text = String::new();
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    text.push_str(&line);
                    text.push('\n');
                }
                text
            })
        });

        // Lines are read on a thread and handed over a channel, so the loop
        // below can notice a cancel even while the model is thinking and the
        // CLI is printing nothing.
        let (tx, rx) = crossbeam_channel::unbounded::<String>();
        let stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            if let Some(out) = stdout {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        });

        let mut parser = StreamParser::default();
        let mut cancelled = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    for delta in parser.feed(&line) {
                        on(delta);
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = reader.join();
        let status = child.wait();
        let stderr = stderr.and_then(|h| h.join().ok()).unwrap_or_default();

        if cancelled {
            on(Delta::Done(StopReason::Cancelled));
            return Ok(());
        }
        match parser.outcome() {
            Some(Ok(reason)) => {
                on(Delta::Done(reason));
                Ok(())
            }
            Some(Err(message)) => Err(AiError::Other(message)),
            None => {
                // No result line at all: the CLI failed before it could
                // answer, and stderr is the only account of why.
                let detail = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
                let code = status.ok().and_then(|s| s.code());
                Err(AiError::Other(match (detail.is_empty(), code) {
                    (false, _) => format!("Claude Code: {}", detail.trim()),
                    (true, Some(c)) => format!("Claude Code exited with status {c}."),
                    (true, None) => "Claude Code stopped without answering.".into(),
                }))
            }
        }
    }
}

/// Turns `stream-json` lines into deltas.
///
/// Kept apart from the process handling so it can be tested against captured
/// output rather than a live account.
#[derive(Default)]
pub struct StreamParser {
    /// Whether any answer text has streamed, so the final `result` is not
    /// shown a second time.
    streamed_text: bool,
    /// Set when a new model message starts after text has already streamed:
    /// the text either side of a tool call is two paragraphs, not one run-on.
    pending_break: bool,
    /// What each tool call was, by id, so a refusal can say what was refused.
    calls: HashMap<String, String>,
    stop: Option<StopReason>,
    /// Set when the CLI reported its account could not be used.
    auth_failed: bool,
    outcome: Option<Result<StopReason, String>>,
}

impl StreamParser {
    /// Folds in one line of output, returning whatever deltas it carried.
    pub fn feed(&mut self, line: &str) -> Vec<Delta> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match v.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                let Some(event) = v.get("event") else {
                    return out;
                };
                match event.get("type").and_then(Value::as_str) {
                    Some("message_start") => {
                        if self.streamed_text {
                            self.pending_break = true;
                        }
                    }
                    Some("content_block_delta") => {
                        let delta = &event["delta"];
                        match delta.get("type").and_then(Value::as_str) {
                            Some("text_delta") => {
                                if let Some(t) = delta.get("text").and_then(Value::as_str) {
                                    if !t.is_empty() {
                                        let text = if std::mem::take(&mut self.pending_break) {
                                            format!("\n\n{t}")
                                        } else {
                                            t.to_string()
                                        };
                                        self.streamed_text = true;
                                        out.push(Delta::Text(text));
                                    }
                                }
                            }
                            // Empty unless Claude Code is set to show its
                            // reasoning; passed through when it is.
                            Some("thinking_delta") => {
                                if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                                    if !t.is_empty() {
                                        out.push(Delta::Thinking(t.to_string()));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("message_delta") => {
                        if let Some(reason) = event["delta"].get("stop_reason").and_then(Value::as_str) {
                            self.stop = Some(match reason {
                                "end_turn" | "stop_sequence" => StopReason::EndTurn,
                                // Mid-loop: the model stopped to call a tool and
                                // will carry on, so this is not how it ended.
                                "tool_use" => return out,
                                "max_tokens" => StopReason::MaxTokens,
                                "refusal" => StopReason::Refusal(
                                    event["delta"]["stop_details"]["explanation"]
                                        .as_str()
                                        .unwrap_or("the request was declined")
                                        .to_string(),
                                ),
                                other => StopReason::Other(other.to_string()),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                if v.get("error").and_then(Value::as_str) == Some("authentication_failed") {
                    self.auth_failed = true;
                }
                // The complete message, block by block. Tool calls are read
                // from here rather than from the stream, where their input
                // arrives as fragments of JSON.
                let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("Tool");
                    let label = describe(name, &block["input"]);
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        self.calls.insert(id.to_string(), label.clone());
                    }
                    out.push(Delta::Tool(label));
                }
            }
            Some("system") => {
                // A tool call the restrictions turned away — a file outside
                // the folder, most often. Worth showing: the answer may say it
                // could not do something, and this says what.
                if v.get("subtype").and_then(Value::as_str) == Some("permission_denied") {
                    let what = v
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .and_then(|id| self.calls.get(id).cloned())
                        .or_else(|| v.get("tool_name").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| "a tool".into());
                    out.push(Delta::Tool(format!("Not allowed: {what}")));
                }
            }
            Some("result") => {
                let text = v.get("result").and_then(Value::as_str).unwrap_or("").trim();
                if v.get("is_error").and_then(Value::as_bool) == Some(true) {
                    self.outcome = Some(Err(if self.auth_failed {
                        "Claude Code is not signed in. Run `claude` in Terminal, sign in \
                         with /login, then ask again."
                            .to_string()
                    } else if text.is_empty() {
                        "Claude Code could not answer.".to_string()
                    } else {
                        format!("Claude Code: {text}")
                    }));
                } else {
                    // Without partial messages, or on a CLI too old to send
                    // them, the answer only arrives here.
                    if !self.streamed_text && !text.is_empty() {
                        out.push(Delta::Text(text.to_string()));
                    }
                    let reason = match v.get("subtype").and_then(Value::as_str) {
                        Some("error_max_turns") => StopReason::Other(
                            "Stopped after the most steps one question may take — ask \
                             something narrower."
                                .into(),
                        ),
                        _ => self.stop.clone().unwrap_or(StopReason::EndTurn),
                    };
                    self.outcome = Some(Ok(reason));
                }
            }
            _ => {}
        }
        out
    }

    /// How the run ended, once the `result` line has arrived.
    pub fn outcome(&self) -> Option<Result<StopReason, String>> {
        self.outcome.clone()
    }
}

/// A tool call, in a few words the reader can follow.
fn describe(name: &str, input: &Value) -> String {
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or("").trim();
    let file = |key: &str| {
        let path = field(key);
        Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string())
    };
    let quoted = |s: &str| {
        let mut short: String = s.chars().take(60).collect();
        if s.chars().count() > 60 {
            short.push('…');
        }
        format!("\u{201c}{short}\u{201d}")
    };
    match name {
        "Read" => format!("Read {}", file("file_path")),
        "Glob" => format!("Find files {}", field("pattern")),
        "Grep" => format!("Search files for {}", quoted(field("pattern"))),
        "WebSearch" => format!("Web search {}", quoted(field("query"))),
        "WebFetch" => {
            let url = field("url");
            let host = url
                .split("://")
                .nth(1)
                .unwrap_or(url)
                .split('/')
                .next()
                .unwrap_or(url);
            format!("Open {host}")
        }
        "Write" => format!("Write {}", file("file_path")),
        "Edit" => format!("Edit {}", file("file_path")),
        other => other.to_string(),
    }
}

/// Where `claude` is looked for, most specific first.
fn candidates() -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os(PATH_OVERRIDE) {
        out.push(PathBuf::from(p));
    }
    if let Some(path) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join(exe)));
    }
    if let Some(home) = home_dir() {
        out.push(home.join(".local/bin").join(exe));
        out.push(home.join(".claude/local").join(exe));
        out.push(home.join(".npm-global/bin").join(exe));
        out.push(home.join(".bun/bin").join(exe));
    }
    if !cfg!(windows) {
        out.push(PathBuf::from("/opt/homebrew/bin/claude"));
        out.push(PathBuf::from("/usr/local/bin/claude"));
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `PATH` for the child: the CLI's own folder and the usual tool folders
/// ahead of whatever Quark inherited.
///
/// An npm-installed `claude` is a script that needs `node`, and a Finder-
/// launched app's `PATH` does not reach Homebrew, so without this it would fail
/// with "env: node: No such file or directory".
fn child_path(program: &Path) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = program.parent() {
        dirs.push(dir.to_path_buf());
    }
    if !cfg!(windows) {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// The system prompt, written where the CLI can read it and removed after.
struct TempFile(PathBuf);

impl TempFile {
    fn write(text: &str) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "quark-claude-{}-{n}.txt",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // The document text is in here: readable by this user only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(text.as_bytes())?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    fn request(messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            model: "default".into(),
            system: "sys".into(),
            messages,
            max_tokens: 1024,
        }
    }

    fn at(args: &[String], flag: &str) -> Option<usize> {
        args.iter().position(|a| a == flag)
    }

    fn everything(folder: Option<&str>) -> Access {
        Access {
            files: true,
            edits: false,
            web: true,
            folder: folder.map(PathBuf::from),
            document: folder.map(|f| PathBuf::from(f).join("paper.pdf")),
        }
    }

    #[test]
    fn with_nothing_granted_the_cli_can_only_answer() {
        let args = ClaudeCode::args("default", Path::new("/tmp/s.txt"), &Access::default());
        let tools = at(&args, "--tools").expect("tools are always set");
        assert_eq!(args[tools + 1], "", "the empty list is what disables every tool");
        assert!(at(&args, "--allowedTools").is_none());
        assert!(at(&args, "--safe-mode").is_some(), "no plugins, hooks or CLAUDE.md");
        assert!(at(&args, "--no-session-persistence").is_some());
        assert_eq!(args[at(&args, "--output-format").unwrap() + 1], "stream-json");
        assert_eq!(args[at(&args, "--system-prompt-file").unwrap() + 1], "/tmp/s.txt");
    }

    #[test]
    fn granted_tools_are_restricted_preapproved_and_cannot_prompt() {
        let args = ClaudeCode::args("default", Path::new("/tmp/s.txt"), &everything(Some("/docs")));
        assert!(at(&args, "--restricted").is_some(), "file tools stay in the folder, no shell");
        let tools = &args[at(&args, "--tools").unwrap() + 1];
        assert_eq!(tools, "Read,Glob,Grep,WebSearch,WebFetch");
        assert_eq!(&args[at(&args, "--allowedTools").unwrap() + 1], tools);
        assert_eq!(args[at(&args, "--permission-prompts").unwrap() + 1], "none");
        assert!(at(&args, "--max-turns").is_some(), "a question cannot run forever");
        assert!(
            at(&args, "--permission-mode").is_none(),
            "no edit permission unless edits are granted"
        );
        assert!(!tools.contains("Bash"), "never a shell");
    }

    #[test]
    fn edits_add_the_writing_tools_and_nothing_else() {
        let mut access = everything(Some("/docs"));
        access.edits = true;
        access.web = false;
        let args = ClaudeCode::args("default", Path::new("/tmp/s.txt"), &access);
        assert_eq!(args[at(&args, "--tools").unwrap() + 1], "Read,Glob,Grep,Edit,Write");
        assert_eq!(args[at(&args, "--permission-mode").unwrap() + 1], "acceptEdits");
        // The document itself, and every other PDF there, stays untouchable.
        let deny = at(&args, "--disallowedTools").expect("PDFs are protected");
        assert_eq!(args[deny + 1], "Edit(**/*.pdf)");
        assert_eq!(args[deny + 2], "Edit(**/*.PDF)");
        assert!(args[deny + 3].starts_with("--"), "the rule list must end before the next flag");
    }

    #[test]
    fn an_unsaved_document_gets_no_file_tools() {
        // With no folder there is nothing to confine reading to.
        let mut access = everything(None);
        access.edits = true;
        assert_eq!(access.tools(), vec!["WebSearch", "WebFetch"]);
    }

    #[test]
    fn the_cli_runs_in_the_documents_folder_only_when_files_are_granted() {
        let reading = ClaudeCode::new("claude").with_access(everything(Some("/docs")));
        assert_eq!(reading.workdir(), PathBuf::from("/docs"));
        let mut web_only = everything(Some("/docs"));
        web_only.files = false;
        let web_only = ClaudeCode::new("claude").with_access(web_only);
        assert_ne!(web_only.workdir(), PathBuf::from("/docs"));
    }

    #[test]
    fn the_tools_note_names_the_folder_the_document_and_the_web() {
        let note = everything(Some("/docs")).note();
        assert!(note.contains("/docs"), "{note}");
        assert!(note.contains("paper.pdf"), "{note}");
        assert!(note.contains("URL"), "{note}");
        assert!(!note.contains("Edits"), "edits were not granted: {note}");
        assert!(Access::default().note().is_empty());
    }

    #[test]
    fn the_default_model_defers_to_claude_codes_own_choice() {
        let args = ClaudeCode::args("default", Path::new("/tmp/s.txt"), &Access::default());
        assert!(at(&args, "--model").is_none());
        let args = ClaudeCode::args("opus", Path::new("/tmp/s.txt"), &Access::default());
        assert_eq!(args[at(&args, "--model").unwrap() + 1], "opus");
    }

    #[test]
    fn a_first_question_is_sent_as_it_is() {
        let req = request(vec![Message::user("What is on this page?")]);
        assert_eq!(ClaudeCode::prompt(&req), "What is on this page?");
    }

    #[test]
    fn earlier_turns_become_a_transcript_ahead_of_the_question() {
        let req = request(vec![
            Message::user("First?"),
            Message::assistant("An answer."),
            Message::user("And then?"),
        ]);
        let p = ClaudeCode::prompt(&req);
        let first = p.find("First?").unwrap();
        let answer = p.find("An answer.").unwrap();
        let latest = p.rfind("And then?").unwrap();
        assert!(first < answer && answer < latest);
        assert!(p.ends_with("And then?"));
        assert!(p.contains("<you>"), "the model's own turns are marked as its own");
    }

    // Lines captured from `claude -p --output-format stream-json --verbose
    // --include-partial-messages`, trimmed to the fields that matter.
    const OK: &[&str] = &[
        r#"{"type":"system","subtype":"init","apiKeySource":"none","model":"claude-haiku-4-5"}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"po"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"ng"}}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"pong"}]}}"#,
        r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"}}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"pong"}"#,
    ];

    #[test]
    fn streamed_text_arrives_once_and_the_turn_ends_cleanly() {
        let mut p = StreamParser::default();
        let deltas: Vec<Delta> = OK.iter().flat_map(|l| p.feed(l)).collect();
        assert_eq!(
            deltas,
            vec![Delta::Text("po".into()), Delta::Text("ng".into())],
            "the empty thinking delta is dropped and the final result is not repeated"
        );
        assert_eq!(p.outcome(), Some(Ok(StopReason::EndTurn)));
    }

    // A tool-using run: text, two calls (one refused), then the answer in a
    // second message. Captured from the CLI with file and web tools granted.
    const TOOLS: &[&str] = &[
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Let me look."}}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/docs/notes.txt"}}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/outside.txt"}}]}}"#,
        r#"{"type":"system","subtype":"permission_denied","tool_name":"Read","tool_use_id":"t2"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"WebSearch","input":{"query":"population of Canberra"}}]}}"#,
        r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"tool_use"}}}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The codeword is velvet-heron."}}}"#,
        r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"}}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"The codeword is velvet-heron."}"#,
    ];

    #[test]
    fn tool_calls_become_activity_and_the_text_either_side_gets_a_break() {
        let mut p = StreamParser::default();
        let deltas: Vec<Delta> = TOOLS.iter().flat_map(|l| p.feed(l)).collect();
        assert_eq!(
            deltas,
            vec![
                Delta::Text("Let me look.".into()),
                Delta::Tool("Read notes.txt".into()),
                Delta::Tool("Read outside.txt".into()),
                Delta::Tool("Not allowed: Read outside.txt".into()),
                Delta::Tool("Web search \u{201c}population of Canberra\u{201d}".into()),
                Delta::Text("\n\nThe codeword is velvet-heron.".into()),
            ]
        );
        assert_eq!(
            p.outcome(),
            Some(Ok(StopReason::EndTurn)),
            "stopping to call a tool is not how the turn ended"
        );
    }

    #[test]
    fn tool_calls_are_described_in_plain_words() {
        let d = |name: &str, input: &str| describe(name, &serde_json::from_str(input).unwrap());
        assert_eq!(d("WebFetch", r#"{"url":"https://en.wikipedia.org/wiki/Canberra"}"#), "Open en.wikipedia.org");
        assert_eq!(d("Grep", r#"{"pattern":"quark"}"#), "Search files for \u{201c}quark\u{201d}");
        assert_eq!(d("Glob", r#"{"pattern":"*.pdf"}"#), "Find files *.pdf");
        assert_eq!(d("Write", r#"{"file_path":"/docs/summary.md"}"#), "Write summary.md");
        assert_eq!(d("Mystery", "{}"), "Mystery");
    }

    #[test]
    fn running_out_of_steps_says_so() {
        let mut p = StreamParser::default();
        p.feed(r#"{"type":"result","subtype":"error_max_turns","is_error":false,"result":""}"#);
        assert!(matches!(p.outcome(), Some(Ok(StopReason::Other(_)))));
    }

    #[test]
    fn without_partial_messages_the_result_is_the_answer() {
        let mut p = StreamParser::default();
        let deltas = p.feed(r#"{"type":"result","subtype":"success","is_error":false,"result":"pong"}"#);
        assert_eq!(deltas, vec![Delta::Text("pong".into())]);
    }

    #[test]
    fn a_signed_out_cli_says_how_to_sign_in() {
        let mut p = StreamParser::default();
        p.feed(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Not logged in · Please run /login"}]},"error":"authentication_failed"}"#);
        let deltas = p.feed(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#);
        assert!(deltas.is_empty(), "an error is not shown as the answer");
        let Some(Err(message)) = p.outcome() else {
            panic!("expected an error");
        };
        assert!(message.contains("/login"), "{message}");
    }

    #[test]
    fn other_errors_carry_the_clis_own_words() {
        let mut p = StreamParser::default();
        p.feed(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Usage limit reached"}"#);
        assert_eq!(p.outcome(), Some(Err("Claude Code: Usage limit reached".into())));
    }

    #[test]
    fn a_refusal_is_reported_as_one() {
        let mut p = StreamParser::default();
        p.feed(r#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"explanation":"declined"}}}}"#);
        p.feed(r#"{"type":"result","subtype":"success","is_error":false,"result":""}"#);
        assert_eq!(p.outcome(), Some(Ok(StopReason::Refusal("declined".into()))));
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let mut p = StreamParser::default();
        assert!(p.feed("not json").is_empty());
        assert!(p.feed("").is_empty());
        assert_eq!(p.outcome(), None);
    }

    #[test]
    fn the_system_prompt_file_is_private_and_cleaned_up() {
        let path = {
            let f = TempFile::write("document text").unwrap();
            assert_eq!(std::fs::read_to_string(f.path()).unwrap(), "document text");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(f.path()).unwrap().permissions().mode();
                assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
            }
            f.path().to_path_buf()
        };
        assert!(!path.exists(), "the file outlived the run");
    }

    #[test]
    fn the_child_path_puts_the_clis_folder_first() {
        let path = child_path(Path::new("/somewhere/bin/claude"));
        let first = std::env::split_paths(&path).next().unwrap();
        assert_eq!(first, PathBuf::from("/somewhere/bin"));
    }

    fn live(provider: ClaudeCode, system: &str, question: &str) -> (String, Vec<String>) {
        let req = ChatRequest {
            model: "haiku".into(),
            system: system.into(),
            messages: vec![Message::user(question)],
            max_tokens: 1024,
        };
        let cancel = AtomicBool::new(false);
        let (mut text, mut tools, mut done) = (String::new(), Vec::new(), None);
        provider
            .chat(&req, &cancel, &mut |d| match d {
                Delta::Text(t) => text.push_str(&t),
                Delta::Tool(t) => tools.push(t),
                Delta::Done(r) => done = Some(r),
                Delta::Thinking(_) => {}
            })
            .expect("the CLI answered");
        assert_eq!(done, Some(StopReason::EndTurn), "{text}");
        (text, tools)
    }

    /// Opt-in: `cargo test -p quark-ai -- --ignored live`. Needs Claude Code
    /// installed and signed in, and spends a little of that account's usage.
    #[test]
    #[ignore = "runs the real `claude` CLI on the signed-in account"]
    fn live_round_trip_through_the_installed_cli() {
        let provider = ClaudeCode::new(ClaudeCode::locate().expect("Claude Code is not installed"));
        let (text, tools) = live(
            provider,
            "Reply with exactly the word the reader writes, in lower case, and nothing else.",
            "pong",
        );
        assert!(text.to_lowercase().contains("pong"), "got {text:?}");
        assert!(tools.is_empty(), "no tools were granted: {tools:?}");
    }

    #[test]
    #[ignore = "runs the real `claude` CLI on the signed-in account"]
    fn live_file_access_stays_inside_the_documents_folder() {
        let root = std::env::temp_dir().join(format!("quark-live-{}", std::process::id()));
        let folder = root.join("docs");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("notes.txt"), "The codeword is velvet-heron.\n").unwrap();
        std::fs::write(root.join("outside.txt"), "The codeword is wrong-answer.\n").unwrap();

        let provider = ClaudeCode::new(ClaudeCode::locate().expect("Claude Code is not installed"))
            .with_access(Access {
                files: true,
                folder: Some(folder.clone()),
                ..Access::default()
            });
        let (text, tools) = live(
            provider,
            "Answer in one short line.",
            "Read notes.txt in your folder and tell me the codeword. Do not use any other file.",
        );
        let _ = std::fs::remove_dir_all(&root);
        assert!(text.contains("velvet-heron"), "got {text:?}");
        assert!(tools.iter().any(|t| t.contains("notes.txt")), "{tools:?}");
    }
}
