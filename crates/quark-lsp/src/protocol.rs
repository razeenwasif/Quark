//! The slice of LSP that Quark actually speaks.
//!
//! Hand-written rather than pulled from a crate, for two reasons. Nearly every
//! response shape here needs an `untagged` enum, and each of those subtleties
//! is worth stating explicitly instead of inheriting — the language has
//! accumulated older spellings of the same answer and kept them all legal, so
//! reading one means knowing which shapes it has. And the ambient crate keeps
//! changing how it spells a URI, which is a lot of churn to import for
//! `{ line, character }`.

use serde::{Deserialize, Serialize};

/// LSP counts columns in **UTF-16 code units**, not bytes and not characters —
/// which is why `quark-text` carries a UTF-16 index at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/* ── diagnostics ──────────────────────────────────────────────────── */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint. Absent means "the server
    /// declined to say", which the spec leaves to the client — we treat it as
    /// information rather than as an error, so an unlabelled note cannot make
    /// a clean file look broken.
    #[serde(default)]
    pub severity: Option<u8>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub code: Option<Code>,
    pub message: String,
}

/// `E0308` and `2` are both legal codes.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Code {
    Number(i64),
    Text(String),
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Code::Number(n) => write!(f, "{n}"),
            Code::Text(s) => f.write_str(s),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublishDiagnostics {
    pub uri: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/* ── completion ───────────────────────────────────────────────────── */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    #[serde(default)]
    pub kind: Option<u8>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub insert_text: Option<String>,
    #[serde(default)]
    pub sort_text: Option<String>,
    #[serde(default)]
    pub text_edit: Option<Edit>,
    #[serde(default)]
    pub label_details: Option<LabelDetails>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelDetails {
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A server may send a plain edit or an insert/replace pair; both carry the
/// same replacement text and differ only in how much they cover.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Edit {
    Plain {
        range: Range,
        #[serde(rename = "newText")]
        new_text: String,
    },
    InsertReplace {
        insert: Range,
        replace: Range,
        #[serde(rename = "newText")]
        new_text: String,
    },
}

impl CompletionItem {
    /// What to put in the buffer, and over what.
    ///
    /// A `textEdit` is authoritative — it is how a server says "you typed
    /// `HashM`, replace all of it, not just append". Falling back to appending
    /// `insertText` at the cursor is what produces `HashMHashMap`.
    pub fn edit(&self) -> (Option<Range>, String) {
        match &self.text_edit {
            // The insert variant covers what was typed; the replace variant
            // also eats the identifier to the right of the cursor. Insert is
            // the less destructive default.
            Some(Edit::Plain { range, new_text }) => (Some(*range), new_text.clone()),
            Some(Edit::InsertReplace {
                insert, new_text, ..
            }) => (Some(*insert), new_text.clone()),
            None => (
                None,
                self.insert_text
                    .clone()
                    .unwrap_or_else(|| self.label.clone()),
            ),
        }
    }

    /// The dim text beside the label: `-> Vec<T>`, `std::collections`.
    pub fn hint(&self) -> Option<String> {
        if let Some(details) = &self.label_details {
            let joined: String = [details.detail.as_deref(), details.description.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        self.detail.clone()
    }
}

/// `textDocument/completion` answers with a bare array or with a list that
/// also says whether it was truncated. Untagged handles both; the flag is
/// dropped because we re-request on every keystroke anyway.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CompletionResponse {
    Items(Vec<CompletionItem>),
    List {
        #[serde(default)]
        items: Vec<CompletionItem>,
    },
}

impl CompletionResponse {
    pub fn into_items(self) -> Vec<CompletionItem> {
        match self {
            CompletionResponse::Items(items) | CompletionResponse::List { items } => items,
        }
    }
}

/* ── go to definition ─────────────────────────────────────────────── */

#[derive(Debug, Clone, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    pub target_uri: String,
    /// Where to actually put the cursor — the name, not the whole item. A
    /// server that sends a link is telling you it knows the difference.
    pub target_selection_range: Range,
}

/// Three legal shapes for one answer. Order matters: a `LocationLink` has no
/// `uri` field, so it fails to parse as a `Location` and falls through — but
/// only if `Location` is tried first.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DefinitionResponse {
    One(Location),
    Many(Vec<Location>),
    Links(Vec<LocationLink>),
}

impl DefinitionResponse {
    /// The first target, which is the one to jump to. A list happens for a
    /// trait method with several impls; picking one beats refusing to move.
    pub fn first(self) -> Option<(String, Position)> {
        match self {
            DefinitionResponse::One(l) => Some((l.uri, l.range.start)),
            DefinitionResponse::Many(mut ls) => ls.drain(..).next().map(|l| (l.uri, l.range.start)),
            DefinitionResponse::Links(mut ls) => ls
                .drain(..)
                .next()
                .map(|l| (l.target_uri, l.target_selection_range.start)),
        }
    }
}

/* ── hover ────────────────────────────────────────────────────────── */

/// One block of hover text. Three legal shapes for what is conceptually a
/// string, because the field predates `MarkupContent` and the old forms were
/// never removed.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Marked {
    Text(String),
    /// The deprecated `MarkedString` object — a fenced block, pre-split.
    Fenced {
        language: String,
        value: String,
    },
    /// `MarkupContent`, which is what a server sends today.
    Markup {
        kind: String,
        value: String,
    },
}

impl Marked {
    fn value(&self) -> &str {
        match self {
            Marked::Text(text) => text,
            Marked::Fenced { value, .. } | Marked::Markup { value, .. } => value,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum HoverContents {
    One(Marked),
    Many(Vec<Marked>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hover {
    pub contents: HoverContents,
    /// What the hover is *about* — the identifier, not the cursor. Servers
    /// often omit it, so nothing may depend on having it.
    #[serde(default)]
    pub range: Option<Range>,
}

impl Hover {
    /// The whole hover as plain text.
    ///
    /// We ask for plaintext and most servers oblige, but `contentFormat` is a
    /// preference rather than a contract and markdown arrives anyway — from
    /// servers that ignore it, and from the `MarkedString` shape, which is
    /// fenced by definition. So the fences come off here rather than being
    /// rendered: a panel that shows a row of backticks around a signature is
    /// worse than one that shows the signature.
    pub fn text(&self) -> String {
        let blocks: Vec<&str> = match &self.contents {
            HoverContents::One(marked) => vec![marked.value()],
            HoverContents::Many(all) => all.iter().map(Marked::value).collect(),
        };
        unfence(&blocks.join("\n\n"))
    }
}

/// Drop the markdown that has no plain-text spelling, and any run of blank
/// lines longer than one.
fn unfence(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            continue;
        }
        // A horizontal rule is a section break, and so is the blank line it
        // already sits between. Without a rule to draw, keeping it leaves a
        // row of stray dashes doing nothing the whitespace does not.
        if matches!(line.trim(), "---" | "***" | "___") {
            continue;
        }
        // A fence removed from between two paragraphs leaves three blank
        // lines where the text had one.
        if line.trim().is_empty() && out.last().is_some_and(|l| l.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/* ── references ───────────────────────────────────────────────────── */

/// `textDocument/references` answers with a flat array, or with nothing.
/// Unlike definition there is no link form — a reference has no "target
/// selection" to be more precise about.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ReferencesResponse {
    Many(Vec<Location>),
    One(Location),
}

impl ReferencesResponse {
    pub fn into_locations(self) -> Vec<Location> {
        match self {
            ReferencesResponse::Many(all) => all,
            ReferencesResponse::One(one) => vec![one],
        }
    }
}

/* ── formatting ───────────────────────────────────────────────────── */

/// One replacement, in the server's coordinates.
///
/// Its own type rather than a reuse of completion's `Edit`: that one is an
/// untagged enum because a completion may send an insert/replace *pair*, and
/// nothing but completion does. A formatting edit is only ever this.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/* ── document symbols ─────────────────────────────────────────────── */

/// The modern shape: a tree, with the nesting the file actually has.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub kind: Option<u8>,
    /// Where the *name* is, which is where to land. `range` covers the whole
    /// item, body and all, and jumping to it would put the cursor on the doc
    /// comment above the function.
    pub selection_range: Range,
    #[serde(default)]
    pub children: Vec<DocumentSymbol>,
}

/// The older shape: flat, and carrying a whole location because it was also
/// used for workspace-wide symbol search.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    #[serde(default)]
    pub kind: Option<u8>,
    pub location: Location,
    #[serde(default)]
    pub container_name: Option<String>,
}

/// The two shapes are disjoint on required fields — a `DocumentSymbol` has no
/// `location` and a `SymbolInformation` has no `selectionRange` — so untagged
/// separates them whichever order they are tried in.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SymbolResponse {
    Tree(Vec<DocumentSymbol>),
    Flat(Vec<SymbolInformation>),
}

/// One row of the symbol list, with the tree flattened into an indent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: Option<u8>,
    /// Absent for a document symbol, which is always in the file we asked
    /// about. Present for the flat shape, which says so explicitly.
    pub uri: Option<String>,
    pub line: u32,
    pub character: u32,
    /// How deep in the tree, for indenting the row.
    pub depth: usize,
}

impl SymbolResponse {
    /// Depth-first, which is reading order — the symbols in the order they
    /// appear in the file, each item's children directly under it.
    pub fn flatten(self) -> Vec<Symbol> {
        fn walk(items: Vec<DocumentSymbol>, depth: usize, out: &mut Vec<Symbol>) {
            for item in items {
                out.push(Symbol {
                    name: item.name,
                    detail: item.detail,
                    kind: item.kind,
                    uri: None,
                    line: item.selection_range.start.line,
                    character: item.selection_range.start.character,
                    depth,
                });
                walk(item.children, depth + 1, out);
            }
        }

        let mut out = Vec::new();
        match self {
            SymbolResponse::Tree(items) => walk(items, 0, &mut out),
            SymbolResponse::Flat(items) => {
                out.extend(items.into_iter().map(|item| Symbol {
                    name: item.name,
                    detail: item.container_name,
                    kind: item.kind,
                    uri: Some(item.location.uri),
                    line: item.location.range.start.line,
                    character: item.location.range.start.character,
                    depth: 0,
                }));
            }
        }
        out
    }
}

/* ── edits across files ───────────────────────────────────────────── */

/// A change to one document, with the version it was computed against.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentEdit {
    pub text_document: VersionedDocument,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionedDocument {
    pub uri: String,
    /// Null for a document the server is not tracking a version of.
    #[serde(default)]
    pub version: Option<i64>,
}

/// Creating, renaming or deleting a file — recognised so it can be named in a
/// refusal, not so it can be carried out.
#[derive(Debug, Clone, Deserialize)]
pub struct FileOperation {
    pub kind: String,
}

/// `documentChanges` is a list of either, and the two are disjoint on their
/// required fields: an edit has `edits`, an operation has `kind`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DocumentChange {
    Edits(TextDocumentEdit),
    File(FileOperation),
}

/// Two spellings of the same idea. `changes` is the older map of uri to edits;
/// `documentChanges` is the list that can also carry file operations and, more
/// usefully, a version per document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEdit {
    #[serde(default)]
    pub changes: Option<std::collections::HashMap<String, Vec<TextEdit>>>,
    #[serde(default)]
    pub document_changes: Option<Vec<DocumentChange>>,
}

/// One file's worth of a workspace edit.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdits {
    pub uri: String,
    pub version: Option<i64>,
    pub edits: Vec<TextEdit>,
}

/// A workspace edit, flattened into the form a caller can act on.
#[derive(Debug, Clone, Default)]
pub struct Changes {
    pub files: Vec<FileEdits>,
    /// How many create/rename/delete operations came with it. Counted rather
    /// than carried out: renaming a Rust module renames its file, and a file
    /// operation touches things that are not open, cannot be taken back with
    /// `u`, and may be under version control.
    pub file_operations: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.files.iter().all(|f| f.edits.is_empty())
    }
}

impl WorkspaceEdit {
    /// Flatten to a list of files, in a stable order.
    ///
    /// `documentChanges` wins when both are present, which is what the spec
    /// says and also what you want: it is the one carrying versions. The map
    /// form is sorted by uri, because a `HashMap` iterates in whatever order
    /// it feels like and a rename that reports its files differently each time
    /// is one nobody can test.
    pub fn resolve(self) -> Changes {
        if let Some(changes) = self.document_changes {
            let mut out = Changes::default();
            for change in changes {
                match change {
                    DocumentChange::Edits(one) => out.files.push(FileEdits {
                        uri: one.text_document.uri,
                        version: one.text_document.version,
                        edits: one.edits,
                    }),
                    DocumentChange::File(_) => out.file_operations += 1,
                }
            }
            return out;
        }

        let mut files: Vec<FileEdits> = self
            .changes
            .unwrap_or_default()
            .into_iter()
            .map(|(uri, edits)| FileEdits {
                uri,
                version: None,
                edits,
            })
            .collect();
        files.sort_by(|a, b| a.uri.cmp(&b.uri));
        Changes {
            files,
            file_operations: 0,
        }
    }
}

/* ── code actions ─────────────────────────────────────────────────── */

/// One offer from the server.
///
/// The response is a list of `CodeAction` *or* `Command`, and both have a
/// `title`. They are not told apart here because the distinction does not
/// change what we can do with them: an action is applicable when it carries an
/// edit or can be resolved into one, and everything else needs
/// `workspace/executeCommand`, which Quark does not run.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub edit: Option<WorkspaceEdit>,
    /// Opaque, and handed straight back in `codeAction/resolve`. The expensive
    /// actions arrive without an edit and with this instead — computing one
    /// for every offer in a list of twenty would make the list slow to appear.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// The whole thing as it arrived, because resolving means sending it back.
    #[serde(skip)]
    pub raw: serde_json::Value,
}

impl CodeAction {
    /// Whether choosing this would do anything.
    pub fn is_applicable(&self) -> bool {
        self.edit.is_some() || self.data.is_some()
    }

    /// Read a `textDocument/codeAction` result, keeping each entry's original
    /// JSON alongside the parsed form.
    pub fn parse(result: &serde_json::Value) -> Vec<CodeAction> {
        result
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| {
                let mut action: CodeAction = serde_json::from_value(entry.clone()).ok()?;
                action.raw = entry.clone();
                Some(action)
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn hover(json: &str) -> String {
        serde_json::from_str::<Hover>(json).expect("a hover").text()
    }

    #[test]
    fn hover_reads_all_three_of_its_content_shapes() {
        assert_eq!(hover(r#"{"contents":"a plain string"}"#), "a plain string");
        assert_eq!(
            hover(r#"{"contents":{"kind":"markdown","value":"markup"}}"#),
            "markup"
        );
        assert_eq!(
            hover(r#"{"contents":{"language":"rust","value":"fn f()"}}"#),
            "fn f()"
        );
        assert_eq!(
            hover(r#"{"contents":["one",{"language":"rust","value":"two"}]}"#),
            "one\n\ntwo"
        );
    }

    #[test]
    fn hover_comes_back_without_its_fences() {
        // rust-analyzer sends markdown even when only plaintext was asked for.
        let text = hover(
            r#"{"contents":{"kind":"markdown","value":"```rust\nfn len(&self) -> usize\n```\n\n---\n\nThe length."}}"#,
        );
        // The rule goes with the fences: the blank line it sat between is
        // already the section break it was drawing.
        assert_eq!(text, "fn len(&self) -> usize\n\nThe length.");
    }

    #[test]
    fn a_hover_that_is_only_fences_comes_back_empty() {
        // Which is what lets the caller say "nothing to show" rather than
        // opening an empty panel.
        assert_eq!(hover(r#"{"contents":"```\n\n```"}"#), "");
    }

    #[test]
    fn symbols_read_both_shapes_and_flatten_in_reading_order() {
        let range = r#"{"start":{"line":4,"character":7},"end":{"line":4,"character":9}}"#;
        let tree: SymbolResponse = serde_json::from_str(&format!(
            r#"[{{"name":"Buffer","kind":23,"range":{range},"selectionRange":{range},
                 "children":[{{"name":"len","detail":"fn() -> usize","kind":6,
                               "range":{range},"selectionRange":{range}}}]}}]"#
        ))
        .expect("a tree");
        let flat = tree.flatten();
        assert_eq!(
            flat.iter()
                .map(|s| (s.name.as_str(), s.depth))
                .collect::<Vec<_>>(),
            vec![("Buffer", 0), ("len", 1)]
        );
        assert_eq!(flat[1].line, 4);
        assert!(flat[0].uri.is_none(), "a document symbol names no file");

        let old: SymbolResponse = serde_json::from_str(&format!(
            r#"[{{"name":"main","kind":12,"containerName":"crate",
                 "location":{{"uri":"file:///a.rs","range":{range}}}}}]"#
        ))
        .expect("the flat shape");
        let flat = old.flatten();
        assert_eq!(flat[0].name, "main");
        assert_eq!(flat[0].detail.as_deref(), Some("crate"));
        assert_eq!(flat[0].uri.as_deref(), Some("file:///a.rs"));
    }

    fn workspace_edit(json: &str) -> Changes {
        serde_json::from_str::<WorkspaceEdit>(json)
            .expect("a workspace edit")
            .resolve()
    }

    #[test]
    fn a_workspace_edit_reads_both_of_its_spellings() {
        let one = r#"{"range":{"start":{"line":1,"character":0},
                      "end":{"line":1,"character":3}},"newText":"new"}"#;

        // The old map form. Sorted by uri, because a HashMap iterates in
        // whatever order it likes and a rename that lists its files
        // differently each run is one nobody can test.
        let changes = workspace_edit(&format!(
            r#"{{"changes":{{"file:///z.rs":[{one}],"file:///a.rs":[{one}]}}}}"#
        ));
        assert_eq!(
            changes
                .files
                .iter()
                .map(|f| f.uri.as_str())
                .collect::<Vec<_>>(),
            vec!["file:///a.rs", "file:///z.rs"]
        );
        assert!(changes.files[0].version.is_none(), "the map form has none");

        // The modern list form, which carries a version per document.
        let changes = workspace_edit(&format!(
            r#"{{"documentChanges":[
                {{"textDocument":{{"uri":"file:///a.rs","version":7}},"edits":[{one}]}}]}}"#
        ));
        assert_eq!(changes.files[0].version, Some(7));
        assert_eq!(changes.files[0].edits.len(), 1);
    }

    #[test]
    fn document_changes_win_when_a_server_sends_both() {
        // The spec's rule, and the one you would want anyway: the list form is
        // the one carrying versions.
        let one = r#"{"range":{"start":{"line":0,"character":0},
                      "end":{"line":0,"character":1}},"newText":"x"}"#;
        let changes = workspace_edit(&format!(
            r#"{{"changes":{{"file:///old.rs":[{one}]}},
                 "documentChanges":[{{"textDocument":{{"uri":"file:///new.rs","version":1}},
                                      "edits":[{one}]}}]}}"#
        ));
        assert_eq!(changes.files.len(), 1);
        assert_eq!(changes.files[0].uri, "file:///new.rs");
    }

    #[test]
    fn file_operations_are_counted_not_mistaken_for_edits() {
        // Renaming a Rust module renames its file, and rust-analyzer says so
        // in the same list as the text edits. Reading one as an edit with no
        // edits in it would silently drop the half of the change that moves
        // the file.
        let one = r#"{"range":{"start":{"line":0,"character":0},
                      "end":{"line":0,"character":1}},"newText":"x"}"#;
        let changes = workspace_edit(&format!(
            r#"{{"documentChanges":[
                {{"textDocument":{{"uri":"file:///a.rs","version":1}},"edits":[{one}]}},
                {{"kind":"rename","oldUri":"file:///a.rs","newUri":"file:///b.rs"}},
                {{"kind":"create","uri":"file:///c.rs"}}]}}"#
        ));
        assert_eq!(changes.files.len(), 1);
        assert_eq!(changes.file_operations, 2);
    }

    #[test]
    fn an_empty_workspace_edit_is_empty_however_it_is_spelled() {
        assert!(workspace_edit(r#"{}"#).is_empty());
        assert!(workspace_edit(r#"{"changes":{}}"#).is_empty());
        assert!(workspace_edit(r#"{"documentChanges":[]}"#).is_empty());
        assert!(workspace_edit(
            r#"{"documentChanges":[{"textDocument":{"uri":"file:///a.rs"},"edits":[]}]}"#
        )
        .is_empty());
    }

    #[test]
    fn a_code_action_knows_whether_choosing_it_would_do_anything() {
        let list = serde_json::json!([
            { "title": "Fill match arms", "edit": { "changes": {} } },
            { "title": "Extract into function", "data": { "id": 3 } },
            // A bare Command: a title and a server-side name, no edit and
            // nothing to resolve. Nothing we can run.
            { "title": "Run test", "command": "rust-analyzer.runSingle" },
        ]);
        let actions = CodeAction::parse(&list);
        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions
                .iter()
                .map(CodeAction::is_applicable)
                .collect::<Vec<_>>(),
            vec![true, true, false]
        );
        // The original JSON is kept, because resolving means sending it back.
        assert_eq!(actions[1].raw["data"]["id"], 3);
    }
}
