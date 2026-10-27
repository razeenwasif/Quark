//! Claude in a document.
//!
//! Four ways in, one way back:
//! - a selection and an instruction ("shorten this"): the reply replaces the
//!   selection, as a suggestion;
//! - the dock's conversation: questions answered there, and any changes asked
//!   for come back as suggestions in the text;
//! - `@Claude` in a comment: an answer in the thread;
//! - a draft: new text put in at the caret.
//!
//! Nothing Claude writes changes the document until the person accepts it,
//! except a draft they asked to have written, which is ordinary text that
//! ⌘Z takes back.
//!
//! # Which Claude
//!
//! The PDF assistant defaults to a local model; this is "Ask Claude", so it
//! uses Claude: the `claude` CLI when it is installed (the person's own
//! subscription), otherwise the Anthropic API with a stored key. Claude gets
//! the document's text and nothing else — no files, no tools, no edits of its
//! own. That it leaves the machine is said once, before the first question.

use egui::{Align2, CornerRadius, Id, Rect, Sense, Ui, Vec2, pos2, vec2};
use quark_ai::{AiService, Backend, ChatRequest, Delta, Message, Provider, StopReason};
use quark_doc::{Author, Document, Note, Pos, markdown};
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

use super::view::{Action, paper_id};
use super::{DocTab, DockTab};
use crate::app::App;

/// What Claude was asked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Asked {
    /// A question or request in the dock.
    Chat(String),
    /// An instruction about the selection.
    Edit(String),
    /// An answer in a comment's thread.
    Comment(u32),
    /// New text at the caret.
    Draft(String),
}

/// One side of the dock's conversation.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChatTurn {
    pub(crate) mine: bool,
    pub(crate) text: String,
    pub(crate) activity: Vec<String>,
    pub(crate) note: Option<String>,
    /// Suggestions the answer made, still pending or not.
    pub(crate) edits: Vec<u32>,
    /// Changes Claude proposed that could not be found in the text.
    pub(crate) missed: usize,
}

/// Whether Claude can be asked, and through what.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Status {
    Ready(Backend),
    /// Neither the CLI nor a key: what to do about it.
    Missing,
}

/// A question in flight.
struct Pending {
    turn: u64,
    /// Which document asked, by its tab's id — its path may change while
    /// Claude is answering, if its first line does.
    doc: u64,
    asked: Asked,
    /// For an edit: an anchor on the selection, so the reply finds its text
    /// however the document has changed meanwhile.
    anchor: Option<u32>,
    text: String,
    activity: Vec<String>,
}

#[derive(Default)]
pub(crate) struct DocsClaude {
    service: Option<AiService>,
    pending: Option<Pending>,
    /// Told that the document goes to Anthropic, and said to go ahead.
    pub(crate) acknowledged: bool,
    claude: Option<Option<std::path::PathBuf>>,
}

impl DocsClaude {
    pub(crate) fn busy_with(&self, doc: u64) -> bool {
        self.pending.as_ref().is_some_and(|p| p.doc == doc)
    }

    pub(crate) fn activity(&self) -> Option<&str> {
        self.pending.as_ref().and_then(|p| p.activity.last().map(String::as_str))
    }

    fn cli(&mut self) -> Option<std::path::PathBuf> {
        self.claude.get_or_insert_with(quark_ai::ClaudeCode::locate).clone()
    }
}

const SYSTEM: &str = "You are Claude, working inside Quark, a desktop writing app. The person is writing a Markdown document, given between <document> tags. Write in the document's own voice, tense and language, and match its conventions. Use only the Markdown it uses: # headings, - and 1. lists, - [ ] checklists, > quotes, **bold**, *italic*, `code`, [links](url). Be concise.";

const EDITS: &str = "When the person asks you to change the document, do not rewrite it in your reply. Say in a sentence or two what you changed and why, then give each change as\n<edit>\n<find>the exact text to replace, copied from the document as plain text</find>\n<replace>the new text, as Markdown</replace>\n</edit>\nKeep each <find> short and unique: a sentence, or a paragraph at most. To add text, find the paragraph it should follow and replace it with that paragraph and the new text. Changes are shown as suggestions the person accepts or rejects. When they only ask a question, answer it, quoting the document where that helps.";

impl App {
    /// Whether Claude can be asked, and through which backend.
    pub(crate) fn docs_claude_status(&mut self) -> Status {
        if self.docs_claude.cli().is_some() {
            return Status::Ready(Backend::ClaudeCode);
        }
        if !self.stored_key(Backend::Anthropic).is_empty() {
            return Status::Ready(Backend::Anthropic);
        }
        Status::Missing
    }

    fn docs_provider(&mut self, backend: Backend, doc: &std::path::Path) -> Box<dyn Provider> {
        match backend {
            Backend::ClaudeCode => {
                let cli = self.docs_claude.cli().unwrap_or_else(|| "claude".into());
                // The text is in the prompt; Claude needs nothing on disk.
                let access = quark_ai::claude_code::Access {
                    files: false,
                    edits: false,
                    web: self.prefs.ai_web,
                    folder: doc.parent().map(std::path::Path::to_path_buf),
                    document: None,
                };
                Box::new(quark_ai::ClaudeCode::new(cli).with_access(access))
            }
            _ => Box::new(quark_ai::Anthropic::new(self.stored_key(Backend::Anthropic))),
        }
    }

    fn docs_model(&self, backend: Backend) -> String {
        // The person's own choice of model where it was made for this
        // backend; its default otherwise.
        if crate::assistant::backend_of(&self.prefs) == backend && !self.prefs.ai_model.is_empty() {
            self.prefs.ai_model.clone()
        } else {
            backend.default_model().to_string()
        }
    }

    /// Asks Claude something about document `index`.
    pub(crate) fn ask_claude_in_doc(&mut self, index: usize, asked: Asked) {
        let Status::Ready(backend) = self.docs_claude_status() else {
            if let Some(tab) = self.docs.tabs.get_mut(index) {
                tab.dock = DockTab::Claude;
            }
            self.dock_open = true;
            return;
        };
        if !self.docs_claude.acknowledged {
            if let Some(tab) = self.docs.tabs.get_mut(index) {
                tab.dock = DockTab::Claude;
                tab.waiting_ask = Some(asked);
            }
            self.dock_open = true;
            return;
        }
        if self.docs_claude.pending.is_some() {
            self.toast("Claude is still answering — one thing at a time", false);
            return;
        }
        let Some(tab) = self.docs.tabs.get_mut(index) else { return };
        let (request, anchor) = build(tab, &asked);
        let doc = tab.uid;
        let path = tab.path.clone();
        if let Asked::Chat(q) = &asked {
            tab.chat.push(ChatTurn { mine: true, text: q.clone(), ..Default::default() });
            tab.dock = DockTab::Claude;
        }
        let model = self.docs_model(backend);
        let provider = self.docs_provider(backend, &path);
        let service = self.docs_claude.service.get_or_insert_with(AiService::start);
        let turn = service.new_turn();
        service.ask(turn, provider, ChatRequest { model, ..request });
        self.docs_claude.pending = Some(Pending { turn, doc, asked, anchor, text: String::new(), activity: Vec::new() });
    }

    /// Pulls Claude's answer in as it streams, and acts on it when it ends.
    pub(crate) fn tend_docs_claude(&mut self, ctx: &egui::Context) {
        let Some(service) = &self.docs_claude.service else { return };
        let mut finished: Option<Result<StopReason, String>> = None;
        while let Some(event) = service.try_recv() {
            let Some(p) = &mut self.docs_claude.pending else { continue };
            match event {
                quark_ai::AiEvent::Delta { id, delta } if id == p.turn => match delta {
                    Delta::Text(t) => p.text.push_str(&t),
                    Delta::Tool(t) => p.activity.push(t),
                    Delta::Thinking(_) => {
                        if p.activity.is_empty() {
                            p.activity.push("Thinking".into());
                        }
                    }
                    Delta::Done(reason) => finished = Some(Ok(reason)),
                },
                quark_ai::AiEvent::Failed { id, message, .. } if id == p.turn => finished = Some(Err(message)),
                _ => {}
            }
        }
        if self.docs_claude.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
        let Some(result) = finished else { return };
        let Some(pending) = self.docs_claude.pending.take() else { return };
        let Some(tab) = self.docs.tabs.iter_mut().find(|t| t.uid == pending.doc) else { return };
        let mut toast = None;
        match result {
            Err(message) => {
                if let Some(a) = pending.anchor {
                    tab.editor.doc.clear_anchor(a);
                }
                match &pending.asked {
                    Asked::Chat(_) => tab.chat.push(ChatTurn { note: Some(message), ..Default::default() }),
                    _ => toast = Some(format!("Claude could not answer: {message}")),
                }
            }
            Ok(reason) => {
                let cut = match reason {
                    StopReason::MaxTokens => Some("The answer was cut off at its length limit.".to_string()),
                    StopReason::Refusal(why) => Some(format!("Claude declined: {why}")),
                    StopReason::Other(why) => Some(why),
                    StopReason::EndTurn | StopReason::Cancelled => None,
                };
                toast = finish(tab, pending, cut);
            }
        }
        if let Some(t) = toast {
            self.toast(t, true);
        }
    }
}

/// The request for what was asked, and the anchor an edit's reply goes back to.
fn build(tab: &mut DocTab, asked: &Asked) -> (ChatRequest, Option<u32>) {
    let doc = &tab.editor.doc;
    let whole = markdown::write_blocks(&doc.blocks);
    let mut anchor = None;
    let (system, messages) = match asked {
        Asked::Chat(q) => {
            let system = format!("{SYSTEM}\n\n{EDITS}\n\n<document title=\"{}\">\n{whole}</document>", tab.title());
            let mut messages: Vec<Message> = tab
                .chat
                .iter()
                .rev()
                .take(12)
                .rev()
                .filter(|t| !t.text.is_empty())
                .map(|t| if t.mine { Message::user(t.text.clone()) } else { Message::assistant(t.text.clone()) })
                .collect();
            messages.push(Message::user(q.clone()));
            (system, messages)
        }
        Asked::Edit(instruction) => {
            let (a, b) = tab.editor.selection().ordered();
            let marked = with_marks(doc, a, b);
            let id = tab.editor.doc.fresh_id();
            tab.editor.doc.restyle(a, b, |s| s.add_anchor(id));
            anchor = Some(id);
            let system = format!("{SYSTEM}\n\n<document title=\"{}\">\n{marked}</document>", tab.title());
            let ask = format!(
                "The passage between ⟦ and ⟧ is selected. {instruction}\n\nReply with only the text that should replace the selected passage, as Markdown: no preamble, no explanation, no code fence, and without the ⟦ ⟧ marks."
            );
            (system, vec![Message::user(ask)])
        }
        Asked::Comment(id) => {
            let comment = doc.comment(*id).cloned();
            let system = format!("{SYSTEM}\n\n<document title=\"{}\">\n{whole}</document>", tab.title());
            let thread = comment
                .as_ref()
                .map(|c| {
                    c.notes
                        .iter()
                        .map(|n| format!("{}: {}", if n.author == Author::You { "Writer" } else { "Claude" }, n.text))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let quote = comment.map(|c| c.quote).unwrap_or_default();
            let ask = format!(
                "A comment thread on the passage “{quote}”:\n\n{thread}\n\nReply in the thread as Claude: a few sentences of plain prose, no Markdown headings. If a change to the document would help, say what it is; do not rewrite the passage."
            );
            (system, vec![Message::user(ask)])
        }
        Asked::Draft(prompt) => {
            let empty = doc.is_empty();
            let system = format!("{SYSTEM}\n\n<document title=\"{}\">\n{whole}</document>", tab.title());
            let ask = if empty {
                format!("Write a first draft of this document: {prompt}\n\nReply with only the document, in Markdown, beginning with a # title.")
            } else {
                format!("{prompt}\n\nThis goes into the document where the writer's cursor is. Reply with only the text to put there, in Markdown.")
            };
            (system, vec![Message::user(ask)])
        }
    };
    (ChatRequest { model: String::new(), system, messages, max_tokens: 16_000 }, anchor)
}

/// The document as Markdown, with the selection between ⟦ and ⟧.
fn with_marks(doc: &Document, a: Pos, b: Pos) -> String {
    let mut copy = doc.clone();
    copy.blocks[b.block].insert(b.offset, "⟧", &Default::default());
    copy.blocks[a.block].insert(a.offset, "⟦", &Default::default());
    markdown::write_blocks(&copy.blocks)
}

/// Acts on a finished answer. Returns something to tell the person.
fn finish(tab: &mut DocTab, pending: Pending, cut: Option<String>) -> Option<String> {
    let text = pending.text.trim().to_string();
    match pending.asked {
        Asked::Chat(_) => {
            let (prose, edits) = split_edits(&text);
            let mut made = Vec::new();
            let mut missed = 0;
            for (find, replace) in edits {
                match locate(&tab.editor.doc, &find) {
                    Some((a, b)) => {
                        if let Some(id) = tab.editor.suggest(a, b, replace, String::new()) {
                            made.push(id);
                        }
                    }
                    None => missed += 1,
                }
            }
            tab.chat.push(ChatTurn { mine: false, text: prose, activity: pending.activity, note: cut, edits: made, missed });
            None
        }
        Asked::Edit(instruction) => {
            let anchor = pending.anchor?;
            let range = tab.editor.doc.anchor_range(anchor);
            tab.editor.doc.clear_anchor(anchor);
            let replacement = clean_reply(&text);
            match range {
                Some((a, b)) if !replacement.is_empty() => {
                    tab.editor.suggest(a, b, replacement, instruction);
                    tab.reveal = true;
                    cut
                }
                Some(_) => Some("Claude did not suggest anything".into()),
                None => {
                    tab.chat.push(ChatTurn { mine: false, text: replacement, note: Some("The selected text was deleted before the answer came, so here it is instead.".into()), ..Default::default() });
                    tab.dock = DockTab::Claude;
                    None
                }
            }
        }
        Asked::Comment(id) => {
            if let Some(c) = tab.editor.doc.comment_mut(id) {
                c.notes.push(Note::now(Author::Claude, text));
                tab.editor.touch_comments();
                tab.active_comment = Some(id);
                tab.dock = DockTab::Comments;
            }
            cut
        }
        Asked::Draft(_) => {
            let draft = clean_reply(&text);
            if draft.is_empty() {
                return Some("Claude did not write anything".into());
            }
            if tab.editor.doc.is_empty() {
                tab.editor.select_all();
            }
            tab.editor.insert_markdown(&draft);
            tab.reveal = true;
            cut
        }
    }
}

/// The prose of an answer, and the edits in it.
pub(crate) fn split_edits(text: &str) -> (String, Vec<(String, String)>) {
    let mut prose = String::new();
    let mut edits = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<edit>") {
        prose.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let Some(end) = after.find("</edit>") else {
            rest = after;
            break;
        };
        let body = &after[..end];
        if let (Some(find), Some(replace)) = (between(body, "<find>", "</find>"), between(body, "<replace>", "</replace>")) {
            edits.push((find.trim().to_string(), replace.trim().to_string()));
        }
        rest = &after[end + 7..];
    }
    if !rest.contains("<edit>") {
        prose.push_str(rest);
    }
    let prose = prose.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    (prose.trim().to_string(), edits)
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = text.find(open)? + open.len();
    let b = text[a..].find(close)? + a;
    Some(&text[a..b])
}

/// A reply meant to be put in as it is: without a code fence or the
/// selection marks round it, if Claude added them anyway.
pub(crate) fn clean_reply(text: &str) -> String {
    let mut t = text.trim();
    if t.starts_with("```") && t.ends_with("```") && t.len() > 6 {
        let inner = &t[3..t.len() - 3];
        let inner = inner.split_once('\n').map_or(inner, |(first, rest)| if first.trim().chars().all(char::is_alphanumeric) { rest } else { inner });
        t = inner.trim();
    }
    t.replace(['⟦', '⟧'], "").trim().to_string()
}

/// Where `find` is in the document: copied exactly, or with its Markdown
/// read as text, or ignoring case and runs of spaces.
pub(crate) fn locate(doc: &Document, find: &str) -> Option<(Pos, Pos)> {
    let plain = doc.plain_text();
    let as_text = markdown::read_blocks(find).iter().map(|b| b.text().to_string()).collect::<Vec<_>>().join("\n");
    for needle in [find.to_string(), as_text] {
        let needle = needle.trim();
        if needle.is_empty() {
            continue;
        }
        if let Some(byte) = plain.find(needle) {
            return Some(span(doc, &plain, byte, needle.len()));
        }
    }
    // Forgiving: case and whitespace.
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let target = squash(find);
    if target.is_empty() {
        return None;
    }
    for (i, block) in doc.blocks.iter().enumerate() {
        let text = block.text();
        let lower = text.to_lowercase();
        if lower.chars().count() != text.chars().count() {
            continue;
        }
        if squash(text) == target {
            return Some((Pos::new(i, 0), Pos::new(i, block.len())));
        }
        if let Some(byte) = lower.find(&target) {
            let start = text[..byte].chars().count();
            return Some((Pos::new(i, start), Pos::new(i, start + target.chars().count())));
        }
    }
    None
}

/// Positions of a byte range of the plain text, whose blocks are joined by
/// one newline each.
fn span(doc: &Document, plain: &str, byte: usize, len: usize) -> (Pos, Pos) {
    let at = |b: usize| -> Pos {
        let before = &plain[..b];
        let block = before.matches('\n').count();
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        // Breaks inside a block are newlines too; count blocks properly.
        let mut remaining = before.chars().count();
        for (i, blk) in doc.blocks.iter().enumerate() {
            let n = blk.len();
            if remaining <= n {
                return Pos::new(i, remaining);
            }
            remaining -= n + 1;
        }
        let _ = (block, line_start);
        doc.end()
    };
    (at(byte), at(byte + len))
}

// --- the dock's panel ---

pub(crate) fn panel(ui: &mut Ui, p: &Palette, tab: &mut DocTab, status: &Status, actions: &mut Vec<Action>) {
    let backend = match status {
        Status::Ready(b) => *b,
        Status::Missing => {
            setup(ui, p);
            return;
        }
    };
    if tab.waiting_ask.is_some() {
        egress(ui, p, tab, backend, actions);
        return;
    }

    ui.horizontal(|ui| {
        chip(ui, p, "Whole document");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(backend.label()).font(theme::strong(11.5)).color(p.text_muted));
        });
    });
    ui.add_space(8.0);

    let composer_h = 182.0;
    let list_h = (ui.available_height() - composer_h).max(80.0);
    egui::ScrollArea::vertical().id_salt("doc-claude").max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        if tab.chat.is_empty() && !tab.claude_busy {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Ask about this document, or ask Claude to change it. Changes come back as suggestions you accept or reject.").font(theme::body(12.5)).color(p.text_muted));
        }
        let mut accept_all = None;
        let mut reject_all = None;
        let mut show = None;
        for (ti, turn) in tab.chat.iter().enumerate() {
            if turn.mine {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    egui::Frame::new().fill(p.accent_soft).corner_radius(CornerRadius { nw: 15, ne: 15, sw: 15, se: 5 }).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
                        ui.set_max_width(230.0);
                        ui.add(egui::Label::new(egui::RichText::new(&turn.text).font(theme::body(12.5)).color(p.text)).wrap());
                    });
                });
                continue;
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for a in &turn.activity {
                    ui.horizontal(|ui| {
                        let (tick, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        icons::draw(ui.painter(), tick, Icon::Check, p.text_faint);
                        ui.label(egui::RichText::new(a).font(theme::body(11.0)).color(p.text_faint));
                    });
                }
                if !turn.text.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(&turn.text).font(theme::body(12.5)).color(p.text)).wrap());
                }
                if let Some(note) = &turn.note {
                    ui.label(egui::RichText::new(note).font(theme::body(11.5)).color(p.warning));
                }
                let pending: Vec<u32> = turn.edits.iter().copied().filter(|id| tab.editor.doc.suggestion(*id).is_some()).collect();
                if !turn.edits.is_empty() || turn.missed > 0 {
                    egui::Frame::new().fill(p.wash(0x08)).stroke(egui::Stroke::new(1.0, p.wash(0x0c))).corner_radius(CornerRadius::same(14)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let n = turn.edits.len();
                        let head = if pending.is_empty() {
                            format!("{n} suggested edit{} · dealt with", if n == 1 { "" } else { "s" })
                        } else {
                            format!("{n} suggested edit{} · {} waiting", if n == 1 { "" } else { "s" }, pending.len())
                        };
                        ui.label(egui::RichText::new(head).font(theme::strong(12.0)).color(p.text));
                        if turn.missed > 0 {
                            ui.label(egui::RichText::new(format!("{} more could not be matched to the text", turn.missed)).font(theme::body(11.0)).color(p.text_faint));
                        }
                        if !pending.is_empty() {
                            ui.horizontal(|ui| {
                                if widgets::primary_button(ui, p, "Accept", true).clicked() {
                                    accept_all = Some(ti);
                                }
                                if widgets::text_button(ui, p, "Reject", false).clicked() {
                                    reject_all = Some(ti);
                                }
                                if widgets::ghost_button(ui, p, "Show").clicked() {
                                    show = pending.first().copied();
                                }
                            });
                        }
                    });
                }
            });
        }
        if tab.claude_busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(p.text_faint));
                let doing = tab.claude_doing.clone().unwrap_or_else(|| "Claude is writing…".into());
                ui.label(egui::RichText::new(doing).font(theme::body(12.0)).color(p.text_faint));
            });
        }
        let turn_edits = |i: usize| -> Vec<u32> { tab.chat[i].edits.clone() };
        if let Some(i) = accept_all {
            for id in turn_edits(i) {
                tab.editor.accept(id);
            }
        }
        if let Some(i) = reject_all {
            for id in turn_edits(i) {
                tab.editor.reject(id);
            }
        }
        if let Some(id) = show {
            if let Some((a, _)) = tab.editor.doc.anchor_range(id) {
                tab.editor.caret_to(a);
                tab.reveal = true;
            }
        }
    });

    ui.add_space(6.0);
    for prompt in ["Summarise this for an email", "Find gaps in the argument", "Fix spelling and grammar throughout"] {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        ui.painter().rect_stroke(rect, CornerRadius::same(11), egui::Stroke::new(1.0, p.wash(if r.hovered() { 0x16 } else { 0x0c })), egui::StrokeKind::Inside);
        ui.painter().text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, prompt, theme::strong(12.0), p.text_muted);
        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !tab.claude_busy {
            actions.push(Action::Claude(Asked::Chat(prompt.to_string())));
        }
        ui.add_space(2.0);
    }
    ui.add_space(4.0);
    let mut send = false;
    egui::Frame::new().fill(p.recess).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin { left: 14, right: 6, top: 6, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut tab.chat_draft).hint_text("Ask about this document…").frame(egui::Frame::NONE).desired_width(ui.available_width() - 44.0);
            let r = ui.add(field);
            if std::mem::take(&mut tab.focus_chat) {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                send = true;
            }
            if super::comments::send_button(ui, p, !tab.chat_draft.trim().is_empty() && !tab.claude_busy).clicked() {
                send = true;
            }
        });
    });
    if send && !tab.claude_busy && !tab.chat_draft.trim().is_empty() {
        let q = std::mem::take(&mut tab.chat_draft).trim().to_string();
        actions.push(Action::Claude(Asked::Chat(q)));
    }
}

fn chip(ui: &mut Ui, p: &Palette, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.into(), theme::strong(11.5), p.text_muted);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 30.0, 26.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(9), p.wash(0x0c));
    super::icons::draw(ui.painter(), Rect::from_center_size(rect.left_center() + vec2(12.0, 0.0), Vec2::splat(12.0)), super::icons::DocIcon::Page, p.text_muted);
    ui.painter().galley(rect.left_center() + vec2(22.0, -galley.size().y / 2.0), galley, p.text_muted);
}

/// No Claude to ask: how to have one.
fn setup(ui: &mut Ui, p: &Palette) {
    ui.add_space(16.0);
    egui::Frame::new().fill(p.wash(0x08)).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new("Claude isn't set up yet").font(theme::strong(13.0)).color(p.text));
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new("Install Claude Code and sign in, and Quark will find it — it answers on your own Claude plan. Or store an Anthropic API key in the assistant's settings.").font(theme::body(12.5)).color(p.text_muted)).wrap());
        ui.add_space(6.0);
        ui.label(egui::RichText::new("npm install -g @anthropic-ai/claude-code").font(egui::FontId::monospace(11.5)).color(p.accent_text));
    });
}

/// Said once, before the first question: the document leaves the machine.
fn egress(ui: &mut Ui, p: &Palette, tab: &mut DocTab, backend: Backend, actions: &mut Vec<Action>) {
    ui.add_space(16.0);
    egui::Frame::new().fill(p.wash(0x08)).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new("Claude reads this document to answer").font(theme::strong(13.0)).color(p.text));
        ui.add_space(4.0);
        let how = match backend {
            Backend::ClaudeCode => "through Claude Code, on your Claude plan",
            _ => "through the Anthropic API",
        };
        ui.add(egui::Label::new(egui::RichText::new(format!("Its text is sent to Anthropic {how}. Nothing changes in it until you accept a suggestion.")).font(theme::body(12.5)).color(p.text_muted)).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if widgets::primary_button(ui, p, "Continue", true).clicked() {
                if let Some(asked) = tab.waiting_ask.take() {
                    actions.push(Action::ClaudeAcknowledged);
                    actions.push(Action::Claude(asked));
                }
            }
            if widgets::ghost_button(ui, p, "Not now").clicked() {
                tab.waiting_ask = None;
            }
        });
    });
}

// --- asking about a selection ---

/// The quick instructions offered under a selection.
const QUICK: [(&str, &str); 5] = [
    ("Improve writing", "Improve the writing: clearer and tighter, same meaning and voice."),
    ("Shorten", "Make it shorter, keeping what matters."),
    ("Expand", "Expand it with a little more detail and explanation."),
    ("Fix spelling & grammar", "Fix spelling, grammar and punctuation only; change nothing else."),
    ("Make it formal", "Rewrite it in a more formal register."),
];

/// The box under a selection that asks Claude to change it.
pub(crate) fn ask_box(ctx: &egui::Context, p: &Palette, tab: &mut DocTab, actions: &mut Vec<Action>) {
    let Some(draft) = &mut tab.ask_box else { return };
    let (_, end) = tab.editor.selection().ordered();
    let caret = tab.sheet.caret(end).translate(tab.sheet_origin.to_vec2());
    let left = tab.sheet_origin.x + super::layout::MARGIN_X * tab.zoom - 16.0;
    let mut chosen: Option<String> = None;
    let mut close = false;
    egui::Area::new(Id::new("doc-ask-claude")).order(egui::Order::Foreground).fixed_pos(pos2(left, caret.bottom() + 10.0)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(p.popover)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(egui::Margin::same(8))
            .shadow(theme::float_shadow(p))
            .show(ui, |ui| {
                ui.set_width(520.0);
                egui::Frame::new().fill(p.recess).stroke(egui::Stroke::new(1.0, p.accent_alpha(0x80))).corner_radius(CornerRadius::same(13)).inner_margin(egui::Margin { left: 10, right: 4, top: 4, bottom: 4 }).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (spark, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        icons::draw(ui.painter(), spark, Icon::Sparkle, egui::Color32::from_rgb(0xe8, 0x95, 0x7a));
                        ui.add_space(4.0);
                        let field = egui::TextEdit::singleline(draft).hint_text("Ask Claude to edit the selection…").frame(egui::Frame::NONE).desired_width(ui.available_width() - 44.0);
                        let r = ui.add(field);
                        if !r.has_focus() && !r.lost_focus() {
                            r.request_focus();
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !draft.trim().is_empty() {
                            chosen = Some(draft.trim().to_string());
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                        if super::comments::send_button(ui, p, !draft.trim().is_empty()).clicked() {
                            chosen = Some(draft.trim().to_string());
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (label, instruction) in QUICK {
                        if widgets::text_button(ui, p, label, false).clicked() {
                            chosen = Some(instruction.to_string());
                        }
                    }
                    if widgets::text_button(ui, p, "Explain", false).clicked() {
                        close = true;
                        actions.push(Action::ExplainSelection);
                    }
                });
            });
    });
    if let Some(instruction) = chosen {
        tab.ask_box = None;
        actions.push(Action::Claude(Asked::Edit(instruction)));
        ctx.memory_mut(|m| m.request_focus(paper_id()));
    } else if close {
        tab.ask_box = None;
        ctx.memory_mut(|m| m.request_focus(paper_id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_are_taken_out_of_the_prose() {
        let reply = "I tightened it.\n<edit>\n<find>or is the scaling enough?</find>\n<replace>or is linear growth enough on its own?</replace>\n</edit>\nThat's all.";
        let (prose, edits) = split_edits(reply);
        assert_eq!(prose, "I tightened it.\n\nThat's all.");
        assert_eq!(edits, vec![("or is the scaling enough?".to_string(), "or is linear growth enough on its own?".to_string())]);
    }

    #[test]
    fn a_reply_loses_the_fence_and_marks_claude_added() {
        assert_eq!(clean_reply("```markdown\nNew **text**\n```"), "New **text**");
        assert_eq!(clean_reply("⟦Shorter.⟧"), "Shorter.");
        assert_eq!(clean_reply("  plain  "), "plain");
    }

    #[test]
    fn found_text_is_found_across_blocks_and_through_markdown() {
        let doc = markdown::read("# Plan\n\nThe **scaling** argument.\n\n- one\n- two\n");
        assert_eq!(locate(&doc, "scaling argument"), Some((Pos::new(1, 4), Pos::new(1, 20))));
        assert_eq!(locate(&doc, "The **scaling** argument."), Some((Pos::new(1, 0), Pos::new(1, 21))));
        assert_eq!(locate(&doc, "argument.\none"), Some((Pos::new(1, 12), Pos::new(2, 3))));
        assert_eq!(locate(&doc, "THE   scaling ARGUMENT."), Some((Pos::new(1, 0), Pos::new(1, 21))));
        assert_eq!(locate(&doc, "not there"), None);
    }

    #[test]
    fn the_selection_is_marked_for_claude() {
        let doc = markdown::read("Get the room to **why** quarks hide.\n");
        let md = with_marks(&doc, Pos::new(0, 4), Pos::new(0, 8));
        assert!(md.starts_with("Get ⟦the ⟧room"), "{md}");
    }
}
