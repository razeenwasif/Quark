//! Google's Antigravity, through its `agy` CLI, as an agent in a folder.
//!
//! Run headless — `agy -p … --output-format stream-json` — in the project's
//! folder. It streams one JSON object a line:
//!
//! ```text
//! {"event":"init", …}
//! {"event":"step_update","step_update":{"step_type":"tool","tool_name":"read_file",
//!   "state":"ACTIVE","tool_info":{"parameters":{"Path":"/…/main.tex"}}}}
//! {"event":"step_update","step_update":{"step_type":"agent_response","text_delta":"I changed …"}}
//! {"event":"result","result":{"status":"SUCCESS","response":"I changed …"}}
//! ```
//!
//! # What it may do
//!
//! Headless, `agy` cannot ask before running a command, so it refuses every
//! command — the shell stays shut without a flag to say so. `--mode
//! accept-edits` lets it change files, and `plan` asks it only to read;
//! `--sandbox` restricts its terminal besides. Its file tools take absolute
//! paths, so a file outside the folder is not impossible: those are reported
//! as such, and the app's own snapshot of the folder is what undoes a run.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::provider::{AiError, ChatRequest, Delta, Provider, Role, StopReason};

/// How long one turn may take before the CLI gives up on it.
const TIMEOUT: &str = "900s";

/// The models it offered when this was written; `agy models` lists them.
pub const MODELS: [&str; 6] = [
    "gemini-4-argon-high",
    "gemini-4-argon-medium",
    "gemini-3.8-flash-high",
    "gemini-3.8-flash-medium",
    "claude-opus-5-5-high",
    "claude-sonnet-5-5-medium",
];

pub struct Antigravity {
    program: PathBuf,
    folder: PathBuf,
    edits: bool,
}

impl Antigravity {
    pub fn new(program: PathBuf, folder: PathBuf, edits: bool) -> Self {
        Self { program, folder, edits }
    }

    /// The `agy` executable, where an install puts it or on the PATH.
    pub fn locate() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            candidates.push(home.join(".local/bin/agy"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin/agy"));
        candidates.push(PathBuf::from("/usr/local/bin/agy"));
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|d| d.join("agy")));
        }
        candidates.into_iter().find(|p| is_executable(p))
    }

    /// The arguments for one run: split out to be asserted without spawning.
    pub fn args(model: &str, prompt: &str, edits: bool) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "stream-json".into(),
            "--mode".into(),
            if edits { "accept-edits" } else { "plan" }.into(),
            "--sandbox".into(),
            "--disable-slash-commands".into(),
            "--print-timeout".into(),
            TIMEOUT.into(),
        ];
        let model = model.trim();
        if !model.is_empty() {
            args.push("--model".into());
            args.push(model.into());
        }
        args
    }

    /// The instructions, the earlier turns and the latest message, as one
    /// prompt: `agy` takes no separate system prompt.
    pub fn prompt(req: &ChatRequest) -> String {
        let mut out = String::new();
        if !req.system.trim().is_empty() {
            out.push_str("<instructions>\n");
            out.push_str(req.system.trim());
            out.push_str("\n</instructions>\n\n");
        }
        if let Some((last, earlier)) = req.messages.split_last() {
            if !earlier.is_empty() {
                out.push_str("Earlier in this conversation:\n<conversation>\n");
                for m in earlier {
                    let tag = if m.role == Role::User { "writer" } else { "you" };
                    out.push_str(&format!("<{tag}>\n{}\n</{tag}>\n", m.text.trim()));
                }
                out.push_str("</conversation>\n\nThe writer's latest message:\n\n");
            }
            out.push_str(&last.text);
        }
        out
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Reads `agy`'s lines into deltas.
#[derive(Default)]
pub struct StreamParser {
    folder: PathBuf,
    /// Steps already reported, so a tool is said once though its state is
    /// streamed several times.
    seen: Vec<i64>,
    texted: bool,
    outcome: Option<Result<StopReason, String>>,
}

impl StreamParser {
    pub fn new(folder: &Path) -> Self {
        Self { folder: folder.to_path_buf(), ..Default::default() }
    }

    pub fn feed(&mut self, line: &str) -> Vec<Delta> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        let mut out = Vec::new();
        match v["event"].as_str() {
            Some("step_update") => {
                let s = &v["step_update"];
                if let Some(t) = s["text_delta"].as_str() {
                    self.texted = true;
                    out.push(Delta::Text(t.to_string()));
                }
                if s["step_type"] == "tool" {
                    let step = s["step_index"].as_i64().unwrap_or(-1);
                    let name = s["tool_name"].as_str().unwrap_or("a tool");
                    let params = &s["tool_info"]["parameters"];
                    match s["state"].as_str() {
                        Some("ERROR") => {
                            let why = s["tool_info"]["error"]["message"].as_str().unwrap_or("");
                            let what = describe(name, params, &self.folder);
                            if why.contains("denied") || why.contains("permission") {
                                out.push(Delta::Tool(format!("Not allowed: {what}")));
                            }
                        }
                        _ if !self.seen.contains(&step) => {
                            self.seen.push(step);
                            out.push(Delta::Tool(describe(name, params, &self.folder)));
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                let r = &v["result"];
                if !self.texted {
                    if let Some(text) = r["response"].as_str().filter(|t| !t.is_empty()) {
                        out.push(Delta::Text(text.to_string()));
                    }
                }
                self.outcome = Some(match r["status"].as_str() {
                    Some("SUCCESS") => Ok(StopReason::EndTurn),
                    Some(other) => {
                        let message = r["error"]["message"].as_str().or(r["message"].as_str()).unwrap_or(other);
                        Err(format!("Antigravity: {message}"))
                    }
                    None => Err("Antigravity ended without saying how".into()),
                });
            }
            _ => {}
        }
        out
    }

    pub fn outcome(&self) -> Option<Result<StopReason, String>> {
        self.outcome.clone()
    }
}

/// A tool use, as a line the writer reads: "Read main.tex", "Edited refs.bib".
fn describe(tool: &str, params: &Value, folder: &Path) -> String {
    let path = ["Path", "AbsolutePath", "TargetFile", "File", "DirectoryPath", "SearchPath"]
        .iter()
        .find_map(|k| params[*k].as_str())
        .map(|p| shown(Path::new(p), folder));
    let query = ["Query", "Pattern", "SearchQuery"].iter().find_map(|k| params[*k].as_str());
    let named = path.clone().unwrap_or_else(|| "a file".into());
    match tool {
        "read_file" | "view_file" | "view_file_outline" => format!("Read {named}"),
        "edit_file" | "replace_file_content" | "multi_replace_file_content" | "sed_file" => format!("Edited {named}"),
        "write_to_file" => format!("Wrote {named}"),
        "grep_search" => format!("Searched “{}”", query.unwrap_or("")),
        "find_by_name" | "list_dir" => format!("Looked in {}", path.unwrap_or_else(|| "the folder".into())),
        "search_web" => format!("Web search “{}”", query.unwrap_or("")),
        "read_url_content" => format!("Read {}", params["Url"].as_str().unwrap_or("a page")),
        "run_command" => format!("Ran `{}`", params["CommandLine"].as_str().unwrap_or("")),
        other => other.replace('_', " "),
    }
}

/// A path as the writer knows it: inside the project, from its folder;
/// outside, said to be.
fn shown(path: &Path, folder: &Path) -> String {
    let folder = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
    let path_c = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match path_c.strip_prefix(&folder).or_else(|_| path.strip_prefix(&folder)) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => format!("{} (outside the project)", path.display()),
    }
}

impl Provider for Antigravity {
    fn name(&self) -> &str {
        "Antigravity"
    }

    fn models(&self) -> Result<Vec<String>, AiError> {
        Ok(MODELS.iter().map(|s| s.to_string()).collect())
    }

    fn chat(&self, req: &ChatRequest, cancel: &AtomicBool, on: &mut dyn FnMut(Delta)) -> Result<(), AiError> {
        let prompt = Self::prompt(req);
        let mut command = Command::new(&self.program);
        command
            .args(Self::args(&req.model, &prompt, self.edits))
            .current_dir(&self.folder)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Run from Quark, which may itself be running in an agent's shell.
        for var in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"] {
            command.env_remove(var);
        }
        let mut child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AiError::Other("Antigravity's CLI, agy, was not found. Install Antigravity and run `agy install`.".into())
            } else {
                AiError::Other(format!("Could not start Antigravity: {e}"))
            }
        })?;

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

        let mut parser = StreamParser::new(&self.folder);
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
                let detail = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                let code = status.ok().and_then(|s| s.code());
                Err(AiError::Other(match (detail.is_empty(), code) {
                    (false, _) => format!("Antigravity: {detail}"),
                    (true, Some(c)) => format!("Antigravity exited with status {c}."),
                    (true, None) => "Antigravity stopped without an answer.".into(),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    /// Lines as `agy` printed them, trimmed.
    const RUN: &str = r#"{"event":"init","conversation_id":"c","init":{"cwd":"/p","tools":["read_file"]}}
{"event":"step_update","step_update":{"step_index":0,"state":"DONE","step_type":"user_input"}}
{"event":"step_update","step_update":{"step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"Path":"/p/main.tex"}}}}
{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"Path":"/p/main.tex"}}}}
{"event":"step_update","step_update":{"step_index":4,"state":"ERROR","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"ls -la"},"error":{"type":"TOOL_ERROR","message":"permission check failed: user denied permission to run command"}}}}
{"event":"step_update","step_update":{"step_index":6,"state":"ACTIVE","step_type":"tool","tool_name":"edit_file","tool_info":{"name":"edit_file","parameters":{"Path":"/p/chapters/intro.tex"}}}}
{"event":"step_update","step_update":{"step_index":7,"state":"ACTIVE","step_type":"agent_response","text_delta":"I changed "}}
{"event":"step_update","step_update":{"step_index":7,"state":"DONE","step_type":"agent_response","text_delta":"the intro."}}
{"event":"result","result":{"status":"SUCCESS","response":"I changed the intro.","num_turns":1}}"#;

    #[test]
    fn a_run_reads_as_tools_then_text() {
        let mut p = StreamParser::new(Path::new("/p"));
        let deltas: Vec<Delta> = RUN.lines().flat_map(|l| p.feed(l)).collect();
        assert_eq!(
            deltas,
            vec![
                Delta::Tool("Read main.tex".into()),
                Delta::Tool("Not allowed: Ran `ls -la`".into()),
                Delta::Tool("Edited chapters/intro.tex".into()),
                Delta::Text("I changed ".into()),
                Delta::Text("the intro.".into()),
            ]
        );
        assert_eq!(p.outcome(), Some(Ok(StopReason::EndTurn)));
    }

    #[test]
    fn a_reply_with_no_deltas_comes_from_the_result() {
        let mut p = StreamParser::new(Path::new("/p"));
        let d = p.feed(r#"{"event":"result","result":{"status":"SUCCESS","response":"ok"}}"#);
        assert_eq!(d, vec![Delta::Text("ok".into())]);
    }

    #[test]
    fn a_failed_run_says_so() {
        let mut p = StreamParser::new(Path::new("/p"));
        p.feed(r#"{"event":"result","result":{"status":"ERROR","error":{"message":"quota exceeded"}}}"#);
        assert_eq!(p.outcome(), Some(Err("Antigravity: quota exceeded".into())));
    }

    #[test]
    fn files_outside_the_project_are_said_to_be() {
        assert_eq!(shown(Path::new("/elsewhere/x.tex"), Path::new("/p")), "/elsewhere/x.tex (outside the project)");
    }

    #[test]
    fn edits_or_plan_and_never_the_shell_by_flag() {
        let a = Antigravity::args("", "hi", true);
        let at = |f: &str| a.iter().position(|x| x == f).unwrap();
        assert_eq!(a[at("--mode") + 1], "accept-edits");
        assert_eq!(a[at("--output-format") + 1], "stream-json");
        assert!(a.contains(&"--sandbox".to_string()));
        assert!(!a.iter().any(|x| x.contains("dangerously")), "commands stay refused");
        assert!(!a.contains(&"--model".to_string()));
        let b = Antigravity::args("gemini-4-argon-high", "hi", false);
        assert_eq!(b[b.iter().position(|x| x == "--mode").unwrap() + 1], "plan");
        assert_eq!(b.last().unwrap(), "gemini-4-argon-high");
    }

    #[test]
    fn the_prompt_carries_instructions_and_history() {
        let req = ChatRequest {
            model: String::new(),
            system: "Be brief.".into(),
            messages: vec![Message::user("first"), Message::assistant("done"), Message::user("second")],
            max_tokens: 0,
        };
        let p = Antigravity::prompt(&req);
        assert!(p.starts_with("<instructions>\nBe brief.\n</instructions>"));
        assert!(p.contains("<you>\ndone\n</you>"));
        assert!(p.ends_with("second"));
    }
}
