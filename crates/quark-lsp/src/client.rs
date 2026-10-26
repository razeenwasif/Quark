//! One language server: spawned, supervised, and spoken to without blocking.
//!
//! Everything public here returns immediately. Requests go out through a
//! writer thread so a server that has stopped reading its stdin — which
//! rust-analyzer does, for whole seconds, while it indexes — cannot wedge a
//! keystroke. Replies come back through a mailbox the editor drains when it
//! feels like it. The core stays synchronous and total; the waiting happens
//! out here.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::protocol::{
    Changes, CodeAction, CompletionItem, CompletionResponse, DefinitionResponse, Diagnostic, Hover,
    Position, PublishDiagnostics, ReferencesResponse, Symbol, SymbolResponse, TextEdit,
    WorkspaceEdit,
};
use crate::{transport, uri};

/// What a reply from the server turned out to be.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// `initialize` came back. Nothing may be requested before this.
    Ready {
        server: String,
        can: Capable,
    },
    Diagnostics {
        path: PathBuf,
        items: Vec<Diagnostic>,
    },
    Completion {
        id: i64,
        items: Vec<CompletionItem>,
    },
    Definition {
        id: i64,
        target: Option<Jump>,
    },
    /// Already plain text, and already empty if there was nothing to say —
    /// the difference between "no hover" and "a hover of only code fences"
    /// is not one a caller should have to make.
    Hover {
        id: i64,
        text: String,
    },
    References {
        id: i64,
        places: Vec<Jump>,
    },
    /// The whole document, rewritten. Empty means it was already formatted,
    /// which is the answer most saves get.
    Formatted {
        id: i64,
        edits: Vec<TextEdit>,
    },
    /// What a rename would change, across every file it touches.
    Renamed {
        id: i64,
        changes: Changes,
    },
    CodeActions {
        id: i64,
        actions: Vec<CodeAction>,
    },
    /// A chosen action, with its edit filled in — the expensive ones arrive
    /// without one and are resolved only once somebody picks them.
    Resolved {
        id: i64,
        action: Box<CodeAction>,
    },
    Symbols {
        id: i64,
        symbols: Vec<Symbol>,
    },
    /// Long work the user should be told about — rust-analyzer spends its
    /// first half-minute here, and an editor that says nothing looks broken.
    Progress {
        title: String,
        message: Option<String>,
        done: bool,
    },
    Log {
        text: String,
    },
    /// The server said no, or said something we could not read.
    Failed {
        text: String,
    },
    /// The process is gone. Nothing more will arrive.
    Exited,
}

/// What a server said it would do, read from its `initialize` reply.
///
/// Asked once, here, rather than discovered by sending a request and reading
/// the error. Declining to format or rename is a legitimate answer for a
/// server — most languages' servers have no formatter behind them — and an
/// editor that finds out by failing turns every save into an error notice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capable {
    pub format: bool,
    pub rename: bool,
    pub code_action: bool,
    /// The characters after which it offers completions unasked — texlab's
    /// `\\` and `{` — as a set of ASCII codes. Whitespace is left out: a
    /// popup at every space is noise.
    pub triggers: u128,
}

impl Capable {
    /// `boolean | SomethingOptions` for every one of these, so anything
    /// present that is not an explicit `false` means yes.
    fn read(result: &Value) -> Self {
        let has = |name: &str| {
            !matches!(
                result.pointer(&format!("/capabilities/{name}")),
                None | Some(Value::Null) | Some(Value::Bool(false))
            )
        };
        let mut triggers = 0u128;
        for c in result
            .pointer("/capabilities/completionProvider/triggerCharacters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|s| s.chars().next())
        {
            if c.is_ascii() && !c.is_ascii_whitespace() {
                triggers |= 1u128 << (c as u32);
            }
        }
        Capable {
            format: has("documentFormattingProvider"),
            rename: has("renameProvider"),
            code_action: has("codeActionProvider"),
            triggers,
        }
    }

    /// Whether typing `c` asks for completions.
    pub fn triggers_on(&self, c: char) -> bool {
        c.is_ascii() && self.triggers & (1u128 << (c as u32)) != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jump {
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
}

/// What a request id was asking for, so a reply can be shaped without the
/// caller having to remember what it sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Initialize,
    Completion,
    Definition,
    Hover,
    References,
    Symbols,
    Format,
    Rename,
    CodeActions,
    Resolve,
    Shutdown,
}

type Pending = Arc<Mutex<HashMap<i64, Kind>>>;

pub struct Client {
    name: String,
    child: Child,
    outgoing: Sender<String>,
    incoming: Receiver<Incoming>,
    pending: Pending,
    next_id: i64,
    ready: bool,
    exited: bool,
    /// Notifications raised before `initialize` returned. The spec forbids
    /// sending them early, and a file opened during startup is the common
    /// case, not an edge one.
    queued: Vec<String>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("name", &self.name)
            .field("ready", &self.ready)
            .field("exited", &self.exited)
            .field("queued", &self.queued.len())
            .finish()
    }
}

impl Client {
    /// Spawn a server and send `initialize`. Returns as soon as the process
    /// exists — it is not usable until [`Incoming::Ready`] arrives, which is
    /// what `queued` is for.
    pub fn spawn(
        name: &str,
        program: &str,
        args: &[String],
        root: &std::path::Path,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Without this a console window flashes up behind the editor every
        // time a server starts.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");

        let (to_server, from_editor) = mpsc::channel::<String>();
        let (to_editor, from_server) = mpsc::channel::<Incoming>();
        let pending: Pending = Arc::default();

        spawn_named(&format!("lsp-{name}-write"), move || {
            let mut stdin = stdin;
            for body in from_editor {
                if transport::write_message(&mut stdin, body.as_bytes()).is_err() {
                    break;
                }
            }
            // Closing stdin is how a server is told to stop waiting.
            drop(stdin);
        });

        {
            let to_editor = to_editor.clone();
            let to_server = to_server.clone();
            let pending = Arc::clone(&pending);
            let name = name.to_string();
            spawn_named(&format!("lsp-{name}-read"), move || {
                read_loop(BufReader::new(stdout), &to_editor, &to_server, &pending);
                let _ = to_editor.send(Incoming::Exited);
            });
        }

        spawn_named(&format!("lsp-{name}-err"), move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if to_editor.send(Incoming::Log { text: line }).is_err() {
                    break;
                }
            }
        });

        let mut client = Self {
            name: name.to_string(),
            child,
            outgoing: to_server,
            incoming: from_server,
            pending,
            next_id: 0,
            ready: false,
            exited: false,
            queued: Vec::new(),
        };
        client.initialize(root);
        Ok(client)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Take everything that has arrived. Never blocks, and never waits for a
    /// reply that has not come — a slow server costs nothing but a later
    /// answer.
    pub fn poll(&mut self) -> Vec<Incoming> {
        let mut out = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(message) => {
                    match &message {
                        Incoming::Ready { .. } => {
                            self.ready = true;
                            for body in std::mem::take(&mut self.queued) {
                                let _ = self.outgoing.send(body);
                            }
                        }
                        Incoming::Exited => self.exited = true,
                        _ => {}
                    }
                    out.push(message);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.exited {
                        self.exited = true;
                        out.push(Incoming::Exited);
                    }
                    break;
                }
            }
        }
        out
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.to_string()).spawn(f);
}

/* ── talking ──────────────────────────────────────────────────────── */

impl Client {
    fn initialize(&mut self, root: &std::path::Path) {
        let root_uri = uri::from_path(root);
        let params = json!({
            // Telling the server our pid is what makes it exit if the editor
            // is killed rather than lingering as an orphan eating a core.
            "processId": std::process::id(),
            "clientInfo": { "name": "Quark", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            "capabilities": {
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "didChangeConfiguration": { "dynamicRegistration": false },
                    // The list form of a workspace edit, which is the one that
                    // carries a version per document — the thing that makes a
                    // stale rename detectable rather than silently wrong.
                    // Claiming it also means file operations arrive labelled
                    // instead of as edits with nothing in them.
                    "workspaceEdit": { "documentChanges": true }
                },
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "completion": {
                        "completionItem": {
                            // Deliberately not claiming snippet support. Claim
                            // it and rust-analyzer answers with `${1:arg}`
                            // placeholders, which we would then have to strip
                            // — better to be told the plain text we can use.
                            "snippetSupport": false,
                            "insertReplaceSupport": true,
                            "labelDetailsSupport": true,
                            "documentationFormat": ["plaintext"]
                        }
                    },
                    "definition": { "linkSupport": true },
                    // Plaintext for the same reason snippets are declined
                    // above: it is the format we can actually show. A hover
                    // panel that renders markdown is a markdown renderer, and
                    // the fences come off in `Hover::text` for the servers
                    // that send them anyway.
                    "hover": { "contentFormat": ["plaintext"] },
                    "references": { "dynamicRegistration": false },
                    "formatting": { "dynamicRegistration": false },
                    "rename": { "dynamicRegistration": false },
                    "codeAction": {
                        // Without this a server may answer with bare
                        // `Command`s, which need `workspace/executeCommand` to
                        // do anything. Asking for literals is asking for
                        // actions that carry their own edit.
                        "codeActionLiteralSupport": {
                            "codeActionKind": { "valueSet": [
                                "quickfix", "refactor", "refactor.extract",
                                "refactor.inline", "refactor.rewrite", "source"
                            ]}
                        },
                        // The expensive actions arrive without an edit and are
                        // resolved once one is chosen — computing twenty of
                        // them to draw a list nobody has read yet is what
                        // makes a code-action menu slow to appear.
                        "dataSupport": true,
                        "resolveSupport": { "properties": ["edit"] }
                    },
                    // Ask for the tree. A server that cannot build one falls
                    // back to the flat shape on its own, and both are read.
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "publishDiagnostics": { "relatedInformation": false }
                },
                "window": { "workDoneProgress": true }
            }
        });

        self.next_id += 1;
        let id = self.next_id;
        lock(&self.pending).insert(id, Kind::Initialize);
        // The one message that bypasses the queue, being the thing the queue
        // is waiting for.
        let _ = self.outgoing.send(
            json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": params })
                .to_string(),
        );
    }

    fn send(&mut self, body: Value) {
        let text = body.to_string();
        if self.ready {
            let _ = self.outgoing.send(text);
        } else {
            self.queued.push(text);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, kind: Kind, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        lock(&self.pending).insert(id, kind);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    pub fn did_open(
        &mut self,
        path: &std::path::Path,
        language_id: &str,
        version: u64,
        text: &str,
    ) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri::from_path(path),
                "languageId": language_id,
                "version": version,
                "text": text,
            }}),
        );
    }

    /// Full-document sync. Incremental would ship less, but it also means
    /// getting ranges right on both sides of every edit, and a server that
    /// disagrees with the editor about the text produces diagnostics on the
    /// wrong lines — the worst kind of wrong. Revisit when edits are batched.
    pub fn did_change(&mut self, path: &std::path::Path, version: u64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri::from_path(path), "version": version },
                "contentChanges": [{ "text": text }],
            }),
        );
    }

    pub fn did_save(&mut self, path: &std::path::Path, text: &str) {
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri::from_path(path) }, "text": text }),
        );
    }

    pub fn did_close(&mut self, path: &std::path::Path) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri::from_path(path) } }),
        );
    }

    pub fn completion(&mut self, path: &std::path::Path, at: Position) -> i64 {
        self.request(
            Kind::Completion,
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri::from_path(path) },
                "position": at,
                // 1 = invoked by the user. Saying "triggered by `.`" would be
                // more honest but makes some servers return a narrower set.
                "context": { "triggerKind": 1 },
            }),
        )
    }

    pub fn definition(&mut self, path: &std::path::Path, at: Position) -> i64 {
        self.request(
            Kind::Definition,
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri::from_path(path) }, "position": at }),
        )
    }

    pub fn hover(&mut self, path: &std::path::Path, at: Position) -> i64 {
        self.request(
            Kind::Hover,
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri::from_path(path) }, "position": at }),
        )
    }

    /// Every use of the symbol, the declaration included — leaving it out
    /// makes the list disagree with itself when the cursor is already on it.
    pub fn references(&mut self, path: &std::path::Path, at: Position) -> i64 {
        self.request(
            Kind::References,
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri::from_path(path) },
                "position": at,
                "context": { "includeDeclaration": true },
            }),
        )
    }

    /// Format a whole document.
    ///
    /// The options are a floor, not a policy. A server with a real formatter
    /// behind it — rustfmt, gofmt — uses the project's own configuration and
    /// ignores these entirely, which is the point: the style that wins should
    /// be the repository's, not the editor's.
    pub fn formatting(&mut self, path: &std::path::Path) -> i64 {
        self.request(
            Kind::Format,
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": uri::from_path(path) },
                "options": {
                    "tabSize": 4,
                    "insertSpaces": true,
                    "trimTrailingWhitespace": true,
                    "insertFinalNewline": true,
                },
            }),
        )
    }

    pub fn rename(&mut self, path: &std::path::Path, at: Position, to: &str) -> i64 {
        self.request(
            Kind::Rename,
            "textDocument/rename",
            json!({
                "textDocument": { "uri": uri::from_path(path) },
                "position": at,
                "newName": to,
            }),
        )
    }

    /// What the server would offer for a span of the file.
    ///
    /// The diagnostics in that span go with the request. A quick fix *is* a
    /// fix for a particular diagnostic, and a server given none has nothing to
    /// offer a fix for — which is why asking with an empty context on a line
    /// with an error comes back with refactors and no fixes.
    pub fn code_actions(
        &mut self,
        path: &std::path::Path,
        range: crate::protocol::Range,
        diagnostics: &[Value],
    ) -> i64 {
        self.request(
            Kind::CodeActions,
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri::from_path(path) },
                "range": range,
                "context": { "diagnostics": diagnostics },
            }),
        )
    }

    /// Fill in a chosen action's edit. The action goes back exactly as it
    /// arrived — the server's `data` is opaque and round-trips.
    pub fn resolve_action(&mut self, action: Value) -> i64 {
        self.request(Kind::Resolve, "codeAction/resolve", action)
    }

    pub fn symbols(&mut self, path: &std::path::Path) -> i64 {
        self.request(
            Kind::Symbols,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri::from_path(path) } }),
        )
    }

    /// Ask the server to stop, then stop waiting for it to.
    pub fn shutdown(&mut self) {
        self.request(Kind::Shutdown, "shutdown", json!(null));
        self.notify("exit", json!(null));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // A polite shutdown needs a round trip we are not going to wait for,
        // so: ask, close stdin, and if it is still there, end it. An orphaned
        // rust-analyzer will happily index a workspace forever.
        self.shutdown();
        let (dead, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.outgoing, dead));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic in a reader thread must not take the editor with it.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/* ── listening ────────────────────────────────────────────────────── */

fn read_loop(
    mut stdout: impl BufRead,
    to_editor: &Sender<Incoming>,
    to_server: &Sender<String>,
    pending: &Pending,
) {
    // `$/progress` names its work once, in the `begin`, and then refers to it
    // by token. Remembering the title here is what lets a later `report` say
    // "indexing: 40%" instead of just "40%".
    let mut titles: HashMap<String, String> = HashMap::new();

    loop {
        let body = match transport::read_message(&mut stdout) {
            Ok(Some(body)) => body,
            Ok(None) => return,
            Err(err) => {
                let _ = to_editor.send(Incoming::Failed {
                    text: format!("transport: {err}"),
                });
                return;
            }
        };

        let message: Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(err) => {
                let _ = to_editor.send(Incoming::Failed {
                    text: format!("unreadable message: {err}"),
                });
                continue;
            }
        };

        let id = message.get("id").and_then(Value::as_i64);
        let method = message.get("method").and_then(Value::as_str);

        match (id, method) {
            // A request from the server. It is waiting on us, so every one
            // gets an answer — including methods we have never heard of. A
            // client that stays silent here hangs a server mid-startup.
            (Some(id), Some(method)) => {
                let result = match method {
                    // One null per item: "use your defaults". The array has to
                    // be the right length or the server rejects the reply.
                    "workspace/configuration" => {
                        let count = message
                            .pointer("/params/items")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; count])
                    }
                    // A server pushing an edit at us, rather than answering
                    // one we asked for. Declined honestly: the reply is read,
                    // and answering `null` — or worse, `applied: true` for an
                    // edit nobody applied — would leave the server believing
                    // the workspace says something it does not.
                    "workspace/applyEdit" => json!({
                        "applied": false,
                        "failureReason": "Quark applies only edits it asked for",
                    }),
                    _ => Value::Null,
                };
                let _ = to_server
                    .send(json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string());
            }

            (None, Some(method)) => {
                if let Some(event) = notification(method, &message, &mut titles) {
                    if to_editor.send(event).is_err() {
                        return;
                    }
                }
            }

            (Some(id), None) => {
                let kind = lock(pending).remove(&id);
                if let Some(event) = response(id, kind, &message, to_server) {
                    if to_editor.send(event).is_err() {
                        return;
                    }
                }
            }

            (None, None) => {}
        }
    }
}

fn notification(
    method: &str,
    message: &Value,
    titles: &mut HashMap<String, String>,
) -> Option<Incoming> {
    let params = message.get("params")?;
    match method {
        "textDocument/publishDiagnostics" => {
            let parsed: PublishDiagnostics = serde_json::from_value(params.clone()).ok()?;
            Some(Incoming::Diagnostics {
                path: uri::to_path(&parsed.uri)?,
                items: parsed.diagnostics,
            })
        }

        "$/progress" => {
            let token = match params.get("token")? {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let value = params.get("value")?;
            let stage = value.get("kind").and_then(Value::as_str)?;
            let note = value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string);

            match stage {
                "begin" => {
                    let title = value
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("working")
                        .to_string();
                    titles.insert(token, title.clone());
                    Some(Incoming::Progress {
                        title,
                        message: note,
                        done: false,
                    })
                }
                "report" => Some(Incoming::Progress {
                    title: titles.get(&token).cloned().unwrap_or_default(),
                    message: note,
                    done: false,
                }),
                _ => Some(Incoming::Progress {
                    title: titles.remove(&token).unwrap_or_default(),
                    message: note,
                    done: true,
                }),
            }
        }

        "window/logMessage" | "window/showMessage" => Some(Incoming::Log {
            text: params.get("message").and_then(Value::as_str)?.to_string(),
        }),

        _ => None,
    }
}

fn response(
    id: i64,
    kind: Option<Kind>,
    message: &Value,
    to_server: &Sender<String>,
) -> Option<Incoming> {
    if let Some(error) = message.get("error") {
        // -32800 is "you cancelled this", which is something we asked for
        // rather than something that went wrong.
        if error.get("code").and_then(Value::as_i64) == Some(-32800) {
            return None;
        }
        return Some(Incoming::Failed {
            text: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
                .to_string(),
        });
    }

    let result = message.get("result").unwrap_or(&Value::Null);

    match kind? {
        Kind::Initialize => {
            // `initialized` must be the next thing the server hears, and it
            // goes out from here rather than from the editor so nothing the
            // editor had queued can overtake it.
            let _ = to_server.send(
                json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }).to_string(),
            );
            Some(Incoming::Ready {
                can: Capable::read(result),
                server: result
                    .pointer("/serverInfo/name")
                    .and_then(Value::as_str)
                    .unwrap_or("language server")
                    .to_string(),
            })
        }

        Kind::Completion if result.is_null() => Some(Incoming::Completion {
            id,
            items: Vec::new(),
        }),
        Kind::Completion => match serde_json::from_value::<CompletionResponse>(result.clone()) {
            Ok(response) => Some(Incoming::Completion {
                id,
                items: response.into_items(),
            }),
            Err(err) => Some(Incoming::Failed {
                text: format!("completion: {err}"),
            }),
        },

        Kind::Definition if result.is_null() => Some(Incoming::Definition { id, target: None }),
        Kind::Definition => match serde_json::from_value::<DefinitionResponse>(result.clone()) {
            Ok(response) => Some(Incoming::Definition {
                id,
                target: response.first().and_then(|(uri, at)| {
                    Some(Jump {
                        path: uri::to_path(&uri)?,
                        line: at.line,
                        character: at.character,
                    })
                }),
            }),
            Err(err) => Some(Incoming::Failed {
                text: format!("definition: {err}"),
            }),
        },

        // A null hover is the server saying "nothing here", which is the
        // common answer — on whitespace, on a keyword, on a comment.
        Kind::Hover if result.is_null() => Some(Incoming::Hover {
            id,
            text: String::new(),
        }),
        Kind::Hover => match serde_json::from_value::<Hover>(result.clone()) {
            Ok(hover) => Some(Incoming::Hover {
                id,
                text: hover.text(),
            }),
            Err(err) => Some(Incoming::Failed {
                text: format!("hover: {err}"),
            }),
        },

        Kind::References if result.is_null() => Some(Incoming::References {
            id,
            places: Vec::new(),
        }),
        Kind::References => match serde_json::from_value::<ReferencesResponse>(result.clone()) {
            Ok(response) => Some(Incoming::References {
                id,
                places: response
                    .into_locations()
                    .into_iter()
                    .filter_map(|l| {
                        Some(Jump {
                            path: uri::to_path(&l.uri)?,
                            line: l.range.start.line,
                            character: l.range.start.character,
                        })
                    })
                    .collect(),
            }),
            Err(err) => Some(Incoming::Failed {
                text: format!("references: {err}"),
            }),
        },

        // Null and an empty array mean the same thing here — "nothing to
        // change" — and a caller that had to tell them apart would be reading
        // a difference the servers do not intend.
        Kind::Format if result.is_null() => Some(Incoming::Formatted {
            id,
            edits: Vec::new(),
        }),
        Kind::Format => match serde_json::from_value::<Vec<TextEdit>>(result.clone()) {
            Ok(edits) => Some(Incoming::Formatted { id, edits }),
            Err(err) => Some(Incoming::Failed {
                text: format!("formatting: {err}"),
            }),
        },

        // A null rename is "there is nothing here to rename", which is what
        // you get on a keyword or a literal.
        Kind::Rename if result.is_null() => Some(Incoming::Renamed {
            id,
            changes: Changes::default(),
        }),
        Kind::Rename => match serde_json::from_value::<WorkspaceEdit>(result.clone()) {
            Ok(edit) => Some(Incoming::Renamed {
                id,
                changes: edit.resolve(),
            }),
            Err(err) => Some(Incoming::Failed {
                text: format!("rename: {err}"),
            }),
        },

        Kind::CodeActions => Some(Incoming::CodeActions {
            id,
            actions: CodeAction::parse(result),
        }),

        Kind::Resolve => match serde_json::from_value::<CodeAction>(result.clone()) {
            Ok(mut action) => {
                action.raw = result.clone();
                Some(Incoming::Resolved {
                    id,
                    action: Box::new(action),
                })
            }
            Err(err) => Some(Incoming::Failed {
                text: format!("code action: {err}"),
            }),
        },

        Kind::Symbols if result.is_null() => Some(Incoming::Symbols {
            id,
            symbols: Vec::new(),
        }),
        Kind::Symbols => match serde_json::from_value::<SymbolResponse>(result.clone()) {
            Ok(response) => Some(Incoming::Symbols {
                id,
                symbols: response.flatten(),
            }),
            Err(err) => Some(Incoming::Failed {
                text: format!("symbols: {err}"),
            }),
        },

        Kind::Shutdown => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_characters_a_server_completes_after_are_read_and_whitespace_is_not() {
        let capable = Capable::read(&serde_json::json!({ "capabilities": {
            "completionProvider": { "triggerCharacters": ["\\", "{", " ", "\n"] },
            "documentFormattingProvider": true
        }}));
        assert!(capable.format && capable.triggers_on('\\') && capable.triggers_on('{'));
        assert!(
            !capable.triggers_on(' ')
                && !capable.triggers_on('\n')
                && !capable.triggers_on('x')
                && !capable.triggers_on('é')
        );
        assert!(
            !Capable::read(&serde_json::json!({})).triggers_on('.'),
            "none, when it names none"
        );
    }

    use super::*;
    use std::io::Cursor;

    /// Run the reader over a scripted server. Returns what the editor would
    /// have seen, and what we would have written back — the second half
    /// matters as much as the first: a request we fail to answer is a server
    /// that stops.
    fn converse(messages: &[(&str, Option<Kind>)]) -> (Vec<Incoming>, Vec<Value>) {
        let pending: Pending = Arc::default();
        let mut wire = Vec::new();
        for (index, (body, kind)) in messages.iter().enumerate() {
            if let Some(kind) = kind {
                lock(&pending).insert(index as i64 + 1, *kind);
            }
            transport::write_message(&mut wire, body.as_bytes()).unwrap();
        }

        let (to_editor, from_server) = mpsc::channel();
        let (to_server, from_editor) = mpsc::channel();
        read_loop(Cursor::new(wire), &to_editor, &to_server, &pending);
        drop(to_editor);
        drop(to_server);

        (
            from_server.into_iter().collect(),
            from_editor
                .into_iter()
                .map(|s| serde_json::from_str(&s).unwrap())
                .collect(),
        )
    }

    #[test]
    fn initialize_is_answered_with_initialized_before_anything_else() {
        let (heard, said) = converse(&[(
            r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"rust-analyzer"}}}"#,
            Some(Kind::Initialize),
        )]);
        assert!(matches!(&heard[0], Incoming::Ready { server, .. } if server == "rust-analyzer"));
        assert_eq!(said[0]["method"], "initialized");
    }

    #[test]
    fn a_server_is_taken_at_its_word_about_formatting() {
        // `boolean | DocumentFormattingOptions`, and absent for a server that
        // does not do it at all. Guessing wrong in the permissive direction
        // means every save asks a question that comes back an error.
        let answers = [
            (
                r#"{"capabilities":{"documentFormattingProvider":true}}"#,
                true,
            ),
            (
                r#"{"capabilities":{"documentFormattingProvider":{}}}"#,
                true,
            ),
            (
                r#"{"capabilities":{"documentFormattingProvider":false}}"#,
                false,
            ),
            (r#"{"capabilities":{}}"#, false),
            (r#"{}"#, false),
        ];
        for (result, expected) in answers {
            let (heard, _) = converse(&[(
                &format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result}}}"#),
                Some(Kind::Initialize),
            )]);
            let Incoming::Ready { can, .. } = &heard[0] else {
                panic!("expected ready, got {heard:?}");
            };
            assert_eq!(can.format, expected, "for {result}");
        }
    }

    #[test]
    fn the_three_capabilities_are_read_independently() {
        // A server offering one of them and not the others is the common
        // case; reading them as a single "supports LSP" flag would offer
        // renames on servers that decline to do them.
        let (heard, _) = converse(&[(
            r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{
                "renameProvider":{"prepareProvider":true},
                "codeActionProvider":true}}}"#,
            Some(Kind::Initialize),
        )]);
        let Incoming::Ready { can, .. } = &heard[0] else {
            panic!("expected ready, got {heard:?}");
        };
        assert!(can.rename && can.code_action);
        assert!(!can.format, "this one said nothing about formatting");
    }

    #[test]
    fn an_edit_pushed_at_us_is_declined_in_the_shape_the_server_expects() {
        // `workspace/applyEdit` is a *request*: the server waits for an answer
        // and reads it. `null` is not one of the shapes it accepts, and
        // `applied: true` for an edit nobody applied would leave the server
        // believing the workspace says something it does not.
        let (_, said) = converse(&[(
            r#"{"jsonrpc":"2.0","id":4,"method":"workspace/applyEdit",
                "params":{"edit":{"changes":{}}}}"#,
            None,
        )]);
        assert_eq!(said[0]["id"], 4);
        assert_eq!(said[0]["result"]["applied"], false);
    }

    #[test]
    fn a_rename_that_moves_a_file_says_so_rather_than_dropping_it() {
        // Renaming a Rust module renames its file. Reading that operation as
        // an edit with no edits in it would apply half the change and report
        // success.
        let one = r#"{"range":{"start":{"line":0,"character":4},
                      "end":{"line":0,"character":7}},"newText":"bar"}"#;
        let (heard, _) = converse(&[(
            &format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"documentChanges":[
                    {{"textDocument":{{"uri":"file:///lib.rs","version":2}},"edits":[{one}]}},
                    {{"kind":"rename","oldUri":"file:///foo.rs","newUri":"file:///bar.rs"}}]}}}}"#
            ),
            Some(Kind::Rename),
        )]);
        let Incoming::Renamed { changes, .. } = &heard[0] else {
            panic!("expected a rename, got {heard:?}");
        };
        assert_eq!(changes.files.len(), 1);
        assert_eq!(changes.files[0].version, Some(2));
        assert_eq!(changes.file_operations, 1);
    }

    #[test]
    fn nothing_to_reformat_is_an_empty_answer_not_a_missing_one() {
        let (heard, _) = converse(&[
            (
                r#"{"jsonrpc":"2.0","id":1,"result":null}"#,
                Some(Kind::Format),
            ),
            (
                r#"{"jsonrpc":"2.0","id":2,"result":[]}"#,
                Some(Kind::Format),
            ),
            (
                r#"{"jsonrpc":"2.0","id":3,"result":[{"range":{"start":{"line":0,"character":0},
                    "end":{"line":1,"character":0}},"newText":"fn main() {}\n"}]}"#,
                Some(Kind::Format),
            ),
        ]);
        let counts: Vec<_> = heard
            .iter()
            .map(|e| match e {
                Incoming::Formatted { edits, .. } => edits.len(),
                other => panic!("expected formatting, got {other:?}"),
            })
            .collect();
        assert_eq!(counts, vec![0, 0, 1]);
    }

    #[test]
    fn a_request_from_the_server_always_gets_an_answer() {
        // Including one we have never heard of. Silence here is a hang, and a
        // hang during startup looks exactly like a broken editor.
        let (_, said) = converse(&[
            (
                r#"{"jsonrpc":"2.0","id":7,"method":"window/workDoneProgress/create","params":{"token":"x"}}"#,
                None,
            ),
            (
                r#"{"jsonrpc":"2.0","id":8,"method":"telemetry/somethingNew","params":{}}"#,
                None,
            ),
        ]);
        assert_eq!(said.len(), 2);
        assert_eq!(said[0]["id"], 7);
        assert!(said[0]["result"].is_null());
        assert_eq!(said[1]["id"], 8);
    }

    #[test]
    fn configuration_is_answered_with_one_null_per_item() {
        // The array has to be the same length as the request's, or the server
        // treats the reply as malformed and falls back to nothing.
        let (_, said) = converse(&[(
            r#"{"jsonrpc":"2.0","id":3,"method":"workspace/configuration",
                "params":{"items":[{"section":"rust-analyzer"},{"section":"other"}]}}"#,
            None,
        )]);
        assert_eq!(said[0]["result"], serde_json::json!([null, null]));
    }

    #[test]
    fn diagnostics_arrive_as_a_path_not_a_uri() {
        let (heard, _) = converse(&[(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
                "uri":"file:///C:/src/My%20Crate/main.rs",
                "diagnostics":[{"range":{"start":{"line":3,"character":4},"end":{"line":3,"character":9}},
                                "severity":1,"code":"E0308","source":"rustc","message":"mismatched types"}]}}"#,
            None,
        )]);
        let Incoming::Diagnostics { path, items } = &heard[0] else {
            panic!("expected diagnostics, got {heard:?}");
        };
        assert!(path.ends_with("main.rs"));
        assert!(path.to_string_lossy().contains("My Crate"));
        assert_eq!(items[0].severity, Some(1));
        assert_eq!(items[0].code.as_ref().unwrap().to_string(), "E0308");
    }

    #[test]
    fn completion_reads_both_of_its_legal_shapes() {
        let (heard, _) = converse(&[
            (
                r#"{"jsonrpc":"2.0","id":1,"result":[{"label":"len"}]}"#,
                Some(Kind::Completion),
            ),
            (
                r#"{"jsonrpc":"2.0","id":2,"result":{"isIncomplete":true,"items":[{"label":"push"}]}}"#,
                Some(Kind::Completion),
            ),
            (
                r#"{"jsonrpc":"2.0","id":3,"result":null}"#,
                Some(Kind::Completion),
            ),
        ]);
        let labels: Vec<_> = heard
            .iter()
            .map(|e| match e {
                Incoming::Completion { items, .. } => {
                    items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
                }
                other => panic!("expected completion, got {other:?}"),
            })
            .collect();
        assert_eq!(labels, vec![vec!["len"], vec!["push"], Vec::new()]);
    }

    #[test]
    fn definition_reads_all_three_of_its_legal_shapes() {
        let range = r#"{"start":{"line":10,"character":2},"end":{"line":10,"character":6}}"#;
        let (heard, _) = converse(&[
            (
                &format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"uri":"file:///a.rs","range":{range}}}}}"#
                ),
                Some(Kind::Definition),
            ),
            (
                &format!(
                    r#"{{"jsonrpc":"2.0","id":2,"result":[{{"uri":"file:///b.rs","range":{range}}}]}}"#
                ),
                Some(Kind::Definition),
            ),
            (
                &format!(
                    r#"{{"jsonrpc":"2.0","id":3,"result":[{{"targetUri":"file:///c.rs","targetRange":{range},"targetSelectionRange":{range}}}]}}"#
                ),
                Some(Kind::Definition),
            ),
        ]);
        for (event, name) in heard.iter().zip(["a.rs", "b.rs", "c.rs"]) {
            let Incoming::Definition {
                target: Some(jump), ..
            } = event
            else {
                panic!("expected a jump, got {event:?}");
            };
            assert!(jump.path.ends_with(name), "{jump:?} is not {name}");
            assert_eq!(jump.line, 10);
        }
    }

    #[test]
    fn nothing_under_the_cursor_is_an_empty_hover_not_a_failure() {
        // Both spellings of "nothing": a null result, and a hover whose only
        // content was fences. A caller that had to tell those apart would get
        // it wrong, and an empty panel is worse than no panel.
        let (heard, _) = converse(&[
            (
                r#"{"jsonrpc":"2.0","id":1,"result":null}"#,
                Some(Kind::Hover),
            ),
            (
                r#"{"jsonrpc":"2.0","id":2,"result":{"contents":{"kind":"markdown","value":"```rust\nfn f()\n```"}}}"#,
                Some(Kind::Hover),
            ),
        ]);
        let texts: Vec<_> = heard
            .iter()
            .map(|e| match e {
                Incoming::Hover { text, .. } => text.as_str(),
                other => panic!("expected a hover, got {other:?}"),
            })
            .collect();
        assert_eq!(texts, vec!["", "fn f()"]);
    }

    #[test]
    fn references_arrive_as_paths_in_the_order_the_server_gave_them() {
        let range = r#"{"start":{"line":2,"character":4},"end":{"line":2,"character":8}}"#;
        let (heard, _) = converse(&[(
            &format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":[
                    {{"uri":"file:///C:/src/a.rs","range":{range}}},
                    {{"uri":"file:///C:/src/My%20Crate/b.rs","range":{range}}}]}}"#
            ),
            Some(Kind::References),
        )]);
        let Incoming::References { places, .. } = &heard[0] else {
            panic!("expected references, got {heard:?}");
        };
        assert_eq!(places.len(), 2);
        assert!(places[0].path.ends_with("a.rs"));
        // The percent-encoding comes off here, not in the picker.
        assert!(places[1].path.to_string_lossy().contains("My Crate"));
        assert_eq!(places[0].line, 2);
    }

    #[test]
    fn progress_carries_its_title_past_the_message_that_named_it() {
        // `report` and `end` only carry the token, so an editor that does not
        // remember can only show a percentage with nothing attached to it.
        let (heard, _) = converse(&[
            (
                r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"idx","value":{"kind":"begin","title":"indexing"}}}"#,
                None,
            ),
            (
                r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"idx","value":{"kind":"report","message":"40%"}}}"#,
                None,
            ),
            (
                r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"idx","value":{"kind":"end"}}}"#,
                None,
            ),
        ]);
        let titles: Vec<_> = heard
            .iter()
            .map(|e| match e {
                Incoming::Progress { title, done, .. } => (title.as_str(), *done),
                other => panic!("expected progress, got {other:?}"),
            })
            .collect();
        assert_eq!(
            titles,
            vec![("indexing", false), ("indexing", false), ("indexing", true)]
        );
    }

    #[test]
    fn a_cancelled_request_is_not_reported_as_a_failure() {
        let (heard, _) = converse(&[
            (
                r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32800,"message":"cancelled"}}"#,
                Some(Kind::Completion),
            ),
            (
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":"internal error"}}"#,
                Some(Kind::Completion),
            ),
        ]);
        assert_eq!(heard.len(), 1, "only the real failure surfaces: {heard:?}");
        assert!(matches!(&heard[0], Incoming::Failed { text } if text == "internal error"));
    }

    #[test]
    fn an_unreadable_message_does_not_end_the_conversation() {
        let (heard, _) = converse(&[
            ("not json at all", None),
            (
                r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"still here"}}"#,
                None,
            ),
        ]);
        assert!(matches!(heard[0], Incoming::Failed { .. }));
        assert!(matches!(&heard[1], Incoming::Log { text } if text == "still here"));
    }
}
