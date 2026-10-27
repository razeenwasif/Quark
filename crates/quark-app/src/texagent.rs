//! An agent in the LaTeX project: Claude Code or Antigravity, working on
//! the project's files.
//!
//! # A run
//!
//! The open files are saved and the project's text files snapshotted. The
//! agent then runs headless in the project folder — reading, searching and,
//! in Edit mode, changing files, but never running a command: Quark
//! typesets afterwards itself. When it ends, what it changed is read back
//! against the snapshot as changes, the buffers holding those files are read
//! again, and each change waits to be kept or undone.
//!
//! # Keep and Undo
//!
//! Each changed file has a baseline — the text before the run, with every
//! change kept since folded in — and its text now. Its changes are the
//! difference between the two, worked out again after every step. Keeping a
//! change folds it into the baseline; undoing one writes the baseline's
//! lines back over it. When baseline and file agree, the file is done.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Ui, Vec2, pos2, vec2};
use quark_ai::{AiService, ChatRequest, Delta, Message, StopReason};
use quark_modal::session::Session;
use quark_ui::icons::{self, Icon};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

/// Which agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Who {
    #[default]
    Claude,
    Antigravity,
}

impl Who {
    fn label(self) -> &'static str {
        match self {
            Who::Claude => "Claude Code",
            Who::Antigravity => "Antigravity",
        }
    }

    fn colour(self) -> Color32 {
        match self {
            Who::Claude => Color32::from_rgb(0xe8, 0x95, 0x7a),
            Who::Antigravity => Color32::from_rgb(0x6f, 0x9b, 0xf5),
        }
    }
}

/// How far it may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    /// Reads and answers; changes nothing.
    Ask,
    /// Changes the project's files.
    #[default]
    Edit,
    /// Reads, and says what it would change.
    Plan,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::Ask, Mode::Edit, Mode::Plan];

    fn label(self) -> &'static str {
        match self {
            Mode::Ask => "Ask",
            Mode::Edit => "Edit",
            Mode::Plan => "Plan",
        }
    }

    fn says(self) -> &'static str {
        match self {
            Mode::Ask => "reads only",
            Mode::Edit => "edits in this folder only",
            Mode::Plan => "plans, changes nothing",
        }
    }
}

/// One side of the conversation.
#[derive(Debug, Clone, Default)]
pub(crate) struct Turn {
    pub(crate) mine: bool,
    pub(crate) who: Who,
    pub(crate) text: String,
    pub(crate) tools: Vec<String>,
    pub(crate) note: Option<String>,
    /// Lines added and taken away, when the run changed files.
    pub(crate) changed: Option<(usize, usize, usize)>,
}

/// What a run began from.
struct Run {
    turn: u64,
    who: Who,
    mode: Mode,
    before: Snapshot,
}

/// The project's text files as they were, by path.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) files: BTreeMap<PathBuf, String>,
}

/// Folders that are not the project's own writing.
const PASSED: [&str; 7] = ["node_modules", "target", "build", "_minted", "__pycache__", "out", ".git"];
/// Files larger than this are not the kind an agent edits by hand.
const MOST_BYTES: u64 = 2 << 20;
const MOST_FILES: usize = 3_000;

impl Snapshot {
    /// Every UTF-8 file under `root`, build output and hidden folders apart.
    pub(crate) fn take(root: &Path) -> Snapshot {
        fn visit(dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                if out.len() >= MOST_FILES {
                    return;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
                if name.starts_with('.') || meta.file_type().is_symlink() {
                    continue;
                }
                if meta.is_dir() {
                    if !PASSED.contains(&name.as_str()) {
                        visit(&e.path(), out);
                    }
                } else if meta.len() <= MOST_BYTES {
                    if let Ok(text) = std::fs::read_to_string(e.path()) {
                        out.insert(e.path(), text);
                    }
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(root, &mut files);
        Snapshot { files }
    }
}

/// A run of lines that differ: lines `base` of the baseline became lines
/// `now` of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub(crate) base: (usize, usize),
    pub(crate) now: (usize, usize),
}

/// One file the agent changed, while it is being looked at.
#[derive(Debug, Clone)]
pub(crate) struct Change {
    pub(crate) path: PathBuf,
    /// `None` for a file the run made.
    pub(crate) baseline: Option<String>,
    /// `None` for a file the run deleted.
    pub(crate) now: Option<String>,
    pub(crate) hunks: Vec<Hunk>,
}

impl Change {
    fn new(path: PathBuf, baseline: Option<String>, now: Option<String>) -> Change {
        let mut c = Change { path, baseline, now, hunks: Vec::new() };
        c.rediff();
        c
    }

    fn rediff(&mut self) {
        self.hunks = match (&self.baseline, &self.now) {
            (Some(a), Some(b)) => hunks(&lines(a), &lines(b)),
            _ => Vec::new(),
        };
    }

    /// Whether nothing is left to decide.
    pub(crate) fn settled(&self) -> bool {
        self.baseline == self.now
    }

    /// Made or deleted whole: decided as one.
    pub(crate) fn whole(&self) -> bool {
        self.baseline.is_none() || self.now.is_none()
    }

    pub(crate) fn counts(&self) -> (usize, usize) {
        match (&self.baseline, &self.now) {
            (None, Some(b)) => (lines(b).len(), 0),
            (Some(a), None) => (0, lines(a).len()),
            _ => self.hunks.iter().fold((0, 0), |(p, m), h| (p + h.now.1 - h.now.0, m + h.base.1 - h.base.0)),
        }
    }
}

/// Lines, each with its ending, so joining them gives the text back.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Where two lists of lines differ, as hunks. A longest common subsequence,
/// which for files people write by hand is quick; past a size where it would
/// not be, the whole file is one hunk.
pub(crate) fn hunks(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    // Common head and tail first: an edit is usually a few lines in a long file.
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    if am.is_empty() && bm.is_empty() {
        return Vec::new();
    }
    if am.len().saturating_mul(bm.len()) > 4_000_000 {
        return vec![Hunk { base: (head, head + am.len()), now: (head, head + bm.len()) }];
    }
    // lcs[i][j]: the common length of am[i..] and bm[j..].
    let (n, m) = (am.len(), bm.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * (m + 1) + j] = if am[i] == bm[j] { lcs[(i + 1) * (m + 1) + j + 1] + 1 } else { lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1]) };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut open: Option<(usize, usize)> = None;
    let close = |open: &mut Option<(usize, usize)>, i: usize, j: usize, out: &mut Vec<Hunk>| {
        if let Some((si, sj)) = open.take() {
            out.push(Hunk { base: (head + si, head + i), now: (head + sj, head + j) });
        }
    };
    while i < n || j < m {
        if i < n && j < m && am[i] == bm[j] {
            close(&mut open, i, j, &mut out);
            i += 1;
            j += 1;
        } else {
            open.get_or_insert((i, j));
            if j < m && (i == n || lcs[i * (m + 1) + j + 1] >= lcs[(i + 1) * (m + 1) + j]) {
                j += 1;
            } else {
                i += 1;
            }
        }
    }
    close(&mut open, i, j, &mut out);
    out
}

/// `text` with lines `range` replaced by `with`.
fn splice(text: &str, range: (usize, usize), with: &[&str]) -> String {
    let mut ls = lines(text);
    ls.splice(range.0..range.1, with.iter().copied());
    ls.concat()
}

/// The changes a run made, against what was there before it.
pub(crate) fn changes(before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut out = Vec::new();
    for (path, was) in &before.files {
        match after.files.get(path) {
            Some(now) if now != was => out.push(Change::new(path.clone(), Some(was.clone()), Some(now.clone()))),
            None => out.push(Change::new(path.clone(), Some(was.clone()), None)),
            _ => {}
        }
    }
    for (path, now) in &after.files {
        if !before.files.contains_key(path) {
            out.push(Change::new(path.clone(), None, Some(now.clone())));
        }
    }
    out
}

/// The changes waiting to be kept or undone, and which one is shown.
#[derive(Debug, Clone, Default)]
pub(crate) struct Review {
    pub(crate) changes: Vec<Change>,
    /// (change, hunk) the bar is on.
    pub(crate) at: (usize, usize),
    pub(crate) who: Who,
}

impl Review {
    pub(crate) fn remaining(&self) -> usize {
        self.changes.iter().map(|c| if c.whole() && !c.settled() { 1 } else { c.hunks.len() }).sum()
    }

    /// The position of the current change among all that remain, from one.
    pub(crate) fn ordinal(&self) -> usize {
        let before: usize = self.changes[..self.at.0.min(self.changes.len())].iter().map(|c| if c.whole() && !c.settled() { 1 } else { c.hunks.len() }).sum();
        before + self.at.1 + 1
    }

    fn clamp(&mut self) {
        self.changes.retain(|c| !c.settled());
        if self.changes.is_empty() {
            self.at = (0, 0);
            return;
        }
        self.at.0 = self.at.0.min(self.changes.len() - 1);
        let n = self.changes[self.at.0].hunks.len().max(1);
        self.at.1 = self.at.1.min(n - 1);
    }

    /// Steps to the next change, or back.
    pub(crate) fn step(&mut self, back: bool) {
        let flat: Vec<(usize, usize)> = self
            .changes
            .iter()
            .enumerate()
            .flat_map(|(ci, c)| (0..c.hunks.len().max(1)).map(move |hi| (ci, hi)))
            .collect();
        if flat.is_empty() {
            return;
        }
        let i = flat.iter().position(|x| *x == self.at).unwrap_or(0);
        let n = flat.len();
        self.at = flat[if back { (i + n - 1) % n } else { (i + 1) % n }];
    }

    /// Where the current change is: its file and first line (zero-based).
    pub(crate) fn here(&self) -> Option<(PathBuf, usize)> {
        let c = self.changes.get(self.at.0)?;
        let line = c.hunks.get(self.at.1).map_or(0, |h| h.now.0);
        Some((c.path.clone(), line))
    }

    /// Keeps or undoes the current change. Undoing writes the file; an
    /// error says why it could not.
    pub(crate) fn decide(&mut self, keep: bool) -> Result<(), String> {
        let Some(c) = self.changes.get_mut(self.at.0) else { return Ok(()) };
        // The file as it is now, in case it was saved since.
        if c.now.is_some() {
            c.now = std::fs::read_to_string(&c.path).ok();
            c.rediff();
        }
        if c.whole() || c.hunks.is_empty() {
            if keep {
                c.baseline = c.now.clone();
            } else {
                match &c.baseline {
                    Some(text) => std::fs::write(&c.path, text).map_err(|e| e.to_string())?,
                    None => std::fs::remove_file(&c.path).map_err(|e| e.to_string())?,
                }
                c.now = c.baseline.clone();
            }
        } else if let (Some(h), Some(base), Some(now)) = (c.hunks.get(self.at.1).copied(), c.baseline.clone(), c.now.clone()) {
            if keep {
                let with: Vec<&str> = lines(&now)[h.now.0..h.now.1].to_vec();
                c.baseline = Some(splice(&base, h.base, &with));
            } else {
                let with: Vec<&str> = lines(&base)[h.base.0..h.base.1].to_vec();
                let undone = splice(&now, h.now, &with);
                std::fs::write(&c.path, &undone).map_err(|e| e.to_string())?;
                c.now = Some(undone);
            }
        }
        c.rediff();
        self.clamp();
        Ok(())
    }

    /// Keeps, or undoes, everything left.
    pub(crate) fn decide_all(&mut self, keep: bool) -> Result<(), String> {
        let mut guard = 0;
        while !self.changes.is_empty() && guard < 10_000 {
            self.at = (0, 0);
            self.decide(keep)?;
            guard += 1;
        }
        Ok(())
    }

    /// Hunks in a file, as its current lines: (first, last, lines removed).
    pub(crate) fn marks_for(&self, path: &Path) -> Vec<(usize, usize, usize, bool)> {
        let Some((ci, c)) = self.changes.iter().enumerate().find(|(_, c)| same_file(&c.path, path)) else { return Vec::new() };
        if c.baseline.is_none() {
            let n = c.now.as_deref().map_or(0, |t| lines(t).len());
            return vec![(0, n, 0, self.at.0 == ci)];
        }
        c.hunks.iter().enumerate().map(|(hi, h)| (h.now.0, h.now.1, h.base.1 - h.base.0, self.at == (ci, hi))).collect()
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(x, y)| x == y)
}

/// The agent, its conversation, and the run or review in hand.
#[derive(Default)]
pub(crate) struct Agent {
    pub(crate) open: bool,
    pub(crate) who: Who,
    pub(crate) mode: Mode,
    pub(crate) turns: Vec<Turn>,
    pub(crate) draft: String,
    pub(crate) focus: bool,
    service: Option<AiService>,
    run: Option<Run>,
    pub(crate) review: Option<Review>,
    /// The CLIs, looked for once each.
    claude: Option<Option<PathBuf>>,
    agy: Option<Option<PathBuf>>,
    /// Told that the project goes to the agent's makers, and said to go on.
    pub(crate) acknowledged: bool,
    /// A question waiting on that.
    pub(crate) waiting: Option<String>,
}

/// What the agent asks of the project after a step.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentAsk {
    /// Open this file at this line (zero-based).
    Show(PathBuf, usize),
    /// Files changed: typeset to check them.
    Typeset,
}

/// Where the writer is, for the agent.
pub(crate) struct Context {
    pub(crate) root: PathBuf,
    pub(crate) file: Option<PathBuf>,
    pub(crate) line: usize,
    /// What the last typesetting complained of, in lines.
    pub(crate) log: Vec<String>,
}

impl Agent {
    pub(crate) fn running(&self) -> bool {
        self.run.is_some()
    }

    fn cli(&mut self, who: Who) -> Option<PathBuf> {
        match who {
            Who::Claude => self.claude.get_or_insert_with(quark_ai::ClaudeCode::locate).clone(),
            Who::Antigravity => self.agy.get_or_insert_with(quark_ai::Antigravity::locate).clone(),
        }
    }

    /// Starts a run. Saves the open files first, so the agent reads what the
    /// writer sees.
    pub(crate) fn ask(&mut self, question: String, session: &mut Session, cx: &Context) -> Result<(), String> {
        if self.run.is_some() {
            return Err("The agent is still working".into());
        }
        if self.review.as_ref().is_some_and(|r| !r.changes.is_empty()) {
            return Err("Keep or undo the last changes first".into());
        }
        let who = self.who;
        let Some(cli) = self.cli(who) else {
            return Err(match who {
                Who::Claude => "Claude Code was not found. Install it from claude.com/claude-code.".into(),
                Who::Antigravity => "Antigravity's CLI, agy, was not found. Install Antigravity and run `agy install`.".into(),
            });
        };
        let failed = session.editor_mut().save_unsaved();
        if let Some((name, why)) = failed.first() {
            return Err(format!("{name} could not be saved first: {why}"));
        }
        let mode = self.mode;
        let system = instructions(mode, cx);
        let mut messages: Vec<Message> = self
            .turns
            .iter()
            .rev()
            .take(10)
            .rev()
            .filter(|t| !t.text.is_empty())
            .map(|t| if t.mine { Message::user(t.text.clone()) } else { Message::assistant(t.text.clone()) })
            .collect();
        messages.push(Message::user(question.clone()));
        let request = ChatRequest { model: String::new(), system, messages, max_tokens: 32_000 };
        let edits = mode == Mode::Edit;
        let provider: Box<dyn quark_ai::Provider> = match who {
            Who::Claude => Box::new(quark_ai::ClaudeCode::new(cli).with_access(quark_ai::claude_code::Access {
                files: true,
                edits,
                web: false,
                folder: Some(cx.root.clone()),
                document: cx.file.clone(),
            })),
            Who::Antigravity => Box::new(quark_ai::Antigravity::new(cli, cx.root.clone(), edits)),
        };
        let before = Snapshot::take(&cx.root);
        let service = self.service.get_or_insert_with(AiService::start);
        let turn = service.new_turn();
        let model = match who {
            Who::Claude => "opus".to_string(),
            Who::Antigravity => String::new(),
        };
        service.ask(turn, provider, ChatRequest { model, ..request });
        self.turns.push(Turn { mine: true, who, text: question, ..Default::default() });
        self.turns.push(Turn { mine: false, who, ..Default::default() });
        self.run = Some(Run { turn, who, mode, before });
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        if let Some(s) = &self.service {
            s.cancel();
        }
    }

    /// Takes in what the agent says; when it has finished, works out what
    /// it changed and reads those files back into the editor.
    pub(crate) fn tend(&mut self, session: &mut Session, root: &Path, ctx: &egui::Context) -> Vec<AgentAsk> {
        let mut asks = Vec::new();
        let Some(service) = &self.service else { return asks };
        let mut ended: Option<Result<StopReason, String>> = None;
        while let Some(event) = service.try_recv() {
            let Some(run) = &self.run else { continue };
            let Some(turn) = self.turns.last_mut() else { continue };
            match event {
                quark_ai::AiEvent::Delta { id, delta } if id == run.turn => match delta {
                    Delta::Text(t) => turn.text.push_str(&t),
                    Delta::Tool(t) => turn.tools.push(t),
                    Delta::Thinking(_) => {}
                    Delta::Done(r) => ended = Some(Ok(r)),
                },
                quark_ai::AiEvent::Failed { id, message, .. } if id == run.turn => ended = Some(Err(message)),
                _ => {}
            }
        }
        if self.run.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
        let Some(result) = ended else { return asks };
        let Some(run) = self.run.take() else { return asks };
        let turn = self.turns.last_mut().expect("a turn was pushed with the run");
        turn.text = tidy(&turn.text);
        match result {
            Err(message) => turn.note = Some(message),
            Ok(StopReason::Cancelled) => turn.note = Some("Stopped.".into()),
            Ok(StopReason::MaxTokens) => turn.note = Some("It ran out of room and stopped part way.".into()),
            Ok(StopReason::Refusal(why)) | Ok(StopReason::Other(why)) => turn.note = Some(why),
            Ok(StopReason::EndTurn) => {}
        }
        // Whatever the mode said, the files are the truth of what it did.
        let after = Snapshot::take(root);
        let found = changes(&run.before, &after);
        if !found.is_empty() {
            let (plus, minus) = found.iter().fold((0, 0), |(p, m), c| {
                let (a, b) = c.counts();
                (p + a, m + b)
            });
            turn.changed = Some((found.len(), plus, minus));
            if run.mode != Mode::Edit {
                turn.note = Some(format!("{} changed files though it was asked not to — review them below.", run.who.label()));
            }
            session.editor_mut().reread_changed_on_disk();
            let review = Review { changes: found, at: (0, 0), who: run.who };
            if let Some((path, line)) = review.here() {
                asks.push(AgentAsk::Show(path, line));
            }
            self.review = Some(review);
            asks.push(AgentAsk::Typeset);
        }
        asks
    }

    /// Keeps or undoes, then reads the files back and moves to what is left.
    pub(crate) fn decide(&mut self, keep: bool, all: bool, session: &mut Session) -> (Option<String>, Vec<AgentAsk>) {
        let Some(review) = &mut self.review else { return (None, Vec::new()) };
        let result = if all { review.decide_all(keep) } else { review.decide(keep) };
        session.editor_mut().reread_changed_on_disk();
        let mut asks = Vec::new();
        if review.changes.is_empty() {
            self.review = None;
            if !keep {
                asks.push(AgentAsk::Typeset);
            }
        } else if let Some((path, line)) = review.here() {
            asks.push(AgentAsk::Show(path, line));
        }
        (result.err(), asks)
    }
}

/// The agent's instructions, for the mode it is in and where the writer is.
fn instructions(mode: Mode, cx: &Context) -> String {
    let mut s = String::from(
        "You are working inside Quark, a LaTeX editor, on the LaTeX project in the current folder. The writer is watching your work in the editor and the PDF beside it.\n",
    );
    if let Some(file) = &cx.file {
        let rel = file.strip_prefix(&cx.root).unwrap_or(file);
        s.push_str(&format!("They are looking at {}, line {}.\n", rel.display(), cx.line + 1));
    }
    if !cx.log.is_empty() {
        s.push_str("The last typesetting said:\n");
        for l in cx.log.iter().take(12) {
            s.push_str(&format!("- {l}\n"));
        }
    }
    s.push_str(match mode {
        Mode::Edit => "Make the change asked for by editing the project's files directly with your file tools. Do not run commands: Quark typesets the project itself when you finish, and shows the writer each of your changes to keep or undo. Keep changes small and in the project's own style and packages; never touch PDFs or build output. When done, say in two or three sentences what you changed and where.",
        Mode::Ask => "Do not change any files. Read what you need and answer, quoting the source with its file and line where that helps.",
        Mode::Plan => "Do not change any files. Read what you need, then reply with a short numbered plan of the changes you would make, each naming its file.",
    });
    s
}

/// A reply as shown: Antigravity links files as `[name](file:///…)`, which
/// reads as the name.
pub(crate) fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(mid) = rest[start..].find("](file://") else { break };
        let Some(end) = rest[start + mid..].find(')') else { break };
        out.push_str(&rest[..start]);
        out.push_str(&rest[start + 1..start + mid]);
        rest = &rest[start + mid + end + 1..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

// --- drawing ---

/// The agent's island: which agent, the conversation, the review card and
/// the composer. Returns what it asks of the project.
pub(crate) fn panel(ui: &mut Ui, p: &Palette, agent: &mut Agent, session: &mut Session, cx: &Context) -> Vec<AgentAsk> {
    let mut asks = Vec::new();
    let segments = [widgets::Segment::Text("Claude Code"), widgets::Segment::Text("Antigravity")];
    let at = usize::from(agent.who == Who::Antigravity);
    if let Some(i) = widgets::segmented(ui, p, &segments, at, true, 32.0) {
        if !agent.running() {
            agent.who = if i == 1 { Who::Antigravity } else { Who::Claude };
        }
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = ui.painter().layout_no_wrap(agent.mode.label().into(), theme::strong(11.5), p.accent_text);
            let (rect, mode) = ui.allocate_exact_size(vec2(label.size().x + 26.0, 24.0), Sense::click());
            if mode.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(8), p.wash(0x0d));
            }
            ui.painter().galley(rect.left_center() + vec2(8.0, -label.size().y / 2.0), label, p.accent_text);
            icons::draw(ui.painter(), Rect::from_center_size(rect.right_center() - vec2(10.0, 0.0), Vec2::splat(10.0)), Icon::ChevronDown, p.accent_text);
            let mode = mode.on_hover_text("What the agent may do").on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::Popup::menu(&mode).show(|ui| {
                for m in Mode::ALL {
                    if ui.selectable_label(agent.mode == m, format!("{} — {}", m.label(), m.says())).clicked() {
                        agent.mode = m;
                        ui.close();
                    }
                }
            });
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let here = match &cx.file {
                    Some(file) => format!("{} · line {}", relative(file, &cx.root), cx.line + 1),
                    None => relative(&cx.root, &cx.root),
                };
                chip(ui, p, &here);
            });
        });
    });
    ui.add_space(8.0);

    // Said once: the project leaves the machine.
    if let Some(question) = agent.waiting.clone() {
        egui::Frame::new().fill(p.wash(0x08)).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("The agent reads your project to work on it").font(theme::strong(13.0)).color(p.text));
            ui.add_space(4.0);
            let to = match agent.who {
                Who::Claude => "Anthropic, through Claude Code on your Claude plan",
                Who::Antigravity => "Google, through Antigravity on your account",
            };
            ui.add(egui::Label::new(egui::RichText::new(format!("The files it opens are sent to {to}. It works only in this folder, runs no commands, and every change it makes waits for you to keep or undo it.")).font(theme::body(12.5)).color(p.text_muted)).wrap());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, p, "Continue", true).clicked() {
                    agent.acknowledged = true;
                    agent.waiting = None;
                    if let Err(e) = agent.ask(question.clone(), session, cx) {
                        agent.turns.push(Turn { mine: false, who: agent.who, note: Some(e), ..Default::default() });
                    }
                }
                if widgets::ghost_button(ui, p, "Not now").clicked() {
                    agent.draft = question.clone();
                    agent.waiting = None;
                }
            });
        });
        return asks;
    }

    // Room kept below the conversation: the composer and its suggestions,
    // and the review card while there is one.
    let composer_h = 176.0 + if agent.review.is_some() { 128.0 } else { 0.0 };
    let list_h = (ui.available_height() - composer_h).max(80.0);
    egui::ScrollArea::vertical().id_salt("tex-agent").max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        if agent.turns.is_empty() {
            ui.add_space(8.0);
            ui.add(egui::Label::new(egui::RichText::new("Ask for a change to the paper — a paragraph, a fix for the log's errors, a table made over — and watch it in the source and the PDF. Each change waits for you to keep or undo it.").font(theme::body(12.5)).color(p.text_muted)).wrap());
        }
        let running = agent.running();
        let last = agent.turns.len().saturating_sub(1);
        for (i, turn) in agent.turns.iter().enumerate() {
            if turn.mine {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    egui::Frame::new().fill(p.accent_soft).corner_radius(CornerRadius { nw: 15, ne: 15, sw: 15, se: 5 }).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
                        ui.set_max_width(250.0);
                        ui.add(egui::Label::new(egui::RichText::new(&turn.text).font(theme::body(12.5)).color(p.text)).wrap());
                    });
                });
                continue;
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for t in &turn.tools {
                    let refused = t.starts_with("Not allowed");
                    let edited = t.starts_with("Edit") || t.starts_with("Wrote") || t.starts_with("Write");
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        icons::draw(ui.painter(), r, if refused { Icon::Close } else if edited { Icon::Pen } else { Icon::Check }, if refused { p.warning } else { p.text_faint });
                        ui.add(egui::Label::new(egui::RichText::new(t).font(theme::body(11.5)).color(if edited { p.text_muted } else { p.text_faint })).truncate());
                    });
                }
                if running && i == last {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0).color(turn.who.colour()));
                        ui.label(egui::RichText::new(format!("{} is working…", turn.who.label())).font(theme::body(11.5)).color(p.text_muted));
                    });
                }
                if !turn.text.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(&turn.text).font(theme::body(12.5)).color(p.text)).wrap());
                }
                if let Some(note) = &turn.note {
                    ui.add(egui::Label::new(egui::RichText::new(note).font(theme::body(11.5)).color(p.warning)).wrap());
                }
            });
        }
    });

    // The review card.
    if let Some(review) = &agent.review {
        let left = review.remaining();
        let (files, plus, minus) = review.changes.iter().fold((0, 0, 0), |(f, a, b), c| {
            let (x, y) = c.counts();
            (f + 1, a + x, b + y)
        });
        let mut act: Option<(bool, bool)> = None;
        let mut show = false;
        egui::Frame::new().fill(p.wash(0x08)).stroke(egui::Stroke::new(1.0, p.wash(0x0c))).corner_radius(CornerRadius::same(14)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{left} change{} in {files} file{}", if left == 1 { "" } else { "s" }, if files == 1 { "" } else { "s" })).font(theme::strong(12.0)).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("−{minus}")).font(theme::strong(12.0)).color(p.error));
                    ui.label(egui::RichText::new(format!("+{plus}")).font(theme::strong(12.0)).color(p.success));
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, p, "Keep all", true).clicked() {
                    act = Some((true, true));
                }
                if widgets::text_button(ui, p, "Undo all", false).clicked() {
                    act = Some((false, true));
                }
                if widgets::ghost_button(ui, p, "Review").clicked() {
                    show = true;
                }
            });
        });
        if show {
            if let Some((path, line)) = review.here() {
                asks.push(AgentAsk::Show(path, line));
            }
        }
        if let Some((keep, all)) = act {
            let (err, more) = agent.decide(keep, all, session);
            asks.extend(more);
            if let Some(e) = err {
                agent.turns.push(Turn { mine: false, who: agent.who, note: Some(format!("Could not undo: {e}")), ..Default::default() });
            }
        }
        ui.add_space(6.0);
    }

    // Suggestions, then the composer.
    for prompt in ["Fix the errors in the log", "Tighten the abstract", "Make the tables booktabs"] {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        ui.painter().rect_stroke(rect, CornerRadius::same(10), egui::Stroke::new(1.0, p.wash(if r.hovered() { 0x16 } else { 0x0c })), egui::StrokeKind::Inside);
        ui.painter().text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, prompt, theme::strong(11.5), p.text_muted);
        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !agent.running() {
            agent.draft = prompt.to_string();
            agent.focus = true;
        }
        ui.add_space(1.0);
    }
    ui.add_space(4.0);
    let mut send = false;
    let mut stop = false;
    egui::Frame::new().fill(p.recess).corner_radius(CornerRadius::same(16)).inner_margin(egui::Margin { left: 12, right: 6, top: 6, bottom: 6 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let hint = format!("Ask {} to change the paper…", agent.who.label());
        let field = egui::TextEdit::singleline(&mut agent.draft).hint_text(hint).frame(egui::Frame::NONE).desired_width(f32::INFINITY);
        let r = ui.add(field);
        if std::mem::take(&mut agent.focus) {
            r.request_focus();
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send = true;
        }
        ui.horizontal(|ui| {
            let model = match agent.who {
                Who::Claude => "Opus",
                Who::Antigravity => "Antigravity's model",
            };
            ui.label(egui::RichText::new(format!("{model} · {}", agent.mode.says())).font(theme::body(11.0)).color(p.text_faint));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if agent.running() {
                    if widgets::text_button(ui, p, "Stop", false).clicked() {
                        stop = true;
                    }
                } else {
                    let (rect, b) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
                    let on = !agent.draft.trim().is_empty();
                    ui.painter().rect_filled(rect, CornerRadius::same(11), if on { p.accent } else { p.wash(0x0a) });
                    icons::draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(15.0)), Icon::ArrowUp, if on { p.on_accent } else { p.text_disabled });
                    if b.clicked() {
                        send = true;
                    }
                }
            });
        });
    });
    if stop {
        agent.stop();
    }
    if send && !agent.running() && !agent.draft.trim().is_empty() {
        let question = std::mem::take(&mut agent.draft).trim().to_string();
        if !agent.acknowledged {
            agent.waiting = Some(question);
        } else if let Err(e) = agent.ask(question, session, cx) {
            agent.turns.push(Turn { mine: false, who: agent.who, note: Some(e), ..Default::default() });
        }
    }
    asks
}

/// A file as the project names it, whichever spelling of the folder each has.
fn relative(file: &Path, root: &Path) -> String {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    match canon(file).strip_prefix(canon(root)).or_else(|_| file.strip_prefix(root)) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned()),
    }
}

fn chip(ui: &mut Ui, p: &Palette, text: &str) {
    let room = ((ui.available_width() - 18.0) / 6.6).max(8.0) as usize;
    let text = widgets::elide(text, room.min(40), false);
    let galley = ui.painter().layout_no_wrap(text, theme::strong(11.0), p.text_muted);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), p.wash(0x0c));
    ui.painter().galley(rect.left_center() + vec2(9.0, -galley.size().y / 2.0), galley, p.text_muted);
}

/// The changes in a view of the source: added lines tinted, removed ones
/// marked between lines, the current change outlined. `first` is the view's
/// first line and `line_h` its height.
pub(crate) fn paint_marks(painter: &egui::Painter, rect: Rect, first: usize, line_h: f32, marks: &[(usize, usize, usize, bool)], p: &Palette) {
    let green = Color32::from_rgb(0x7a, 0xd7, 0xa8);
    let red = p.error;
    let visible = (rect.height() / line_h).ceil() as usize + 1;
    for &(a, b, removed, current) in marks {
        if b < first || a > first + visible {
            continue;
        }
        let y = |line: usize| rect.top() + (line as f32 - first as f32) * line_h;
        if b > a {
            let r = Rect::from_min_max(pos2(rect.left(), y(a).max(rect.top())), pos2(rect.right(), y(b).min(rect.bottom())));
            painter.rect_filled(r, CornerRadius::ZERO, green.gamma_multiply(if current { 0.16 } else { 0.09 }));
            painter.rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), CornerRadius::ZERO, green);
        }
        if removed > 0 {
            let at = y(a).clamp(rect.top(), rect.bottom());
            painter.line_segment([pos2(rect.left(), at), pos2(rect.right(), at)], egui::Stroke::new(1.5, red.gamma_multiply(0.8)));
            painter.text(pos2(rect.right() - 8.0, at + 2.0), Align2::RIGHT_TOP, format!("−{removed}"), theme::strong(10.5), red);
        }
    }
}

/// The bar over the source while changes wait: which one, ↑ ↓, Keep, Undo.
/// Returns (keep?, step back?) for what was pressed.
pub(crate) enum BarPress {
    Keep,
    Undo,
    Step(bool),
}

pub(crate) fn review_bar(ui: &mut Ui, p: &Palette, at: egui::Pos2, review: &Review) -> Option<BarPress> {
    let mut press = None;
    egui::Area::new(egui::Id::new("tex-agent-review-bar")).order(egui::Order::Foreground).pivot(Align2::RIGHT_TOP).fixed_pos(at).show(ui.ctx(), |ui| {
        egui::Frame::new().fill(p.popover).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(CornerRadius::same(13)).inner_margin(egui::Margin::same(4)).shadow(theme::float_shadow(p)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                icons::draw(ui.painter(), r, Icon::Sparkle, review.who.colour());
                ui.label(egui::RichText::new(format!("{} of {}", review.ordinal().min(review.remaining()), review.remaining())).font(theme::strong(11.5)).color(p.text_muted));
                if widgets::icon_button(ui, p, Icon::ChevronUp, "Previous change", 26.0, false, true).clicked() {
                    press = Some(BarPress::Step(true));
                }
                if widgets::icon_button(ui, p, Icon::ChevronDown, "Next change", 26.0, false, true).clicked() {
                    press = Some(BarPress::Step(false));
                }
                if widgets::primary_button(ui, p, "Keep", true).clicked() {
                    press = Some(BarPress::Keep);
                }
                if widgets::text_button(ui, p, "Undo", false).clicked() {
                    press = Some(BarPress::Undo);
                }
            });
        });
    });
    press
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("quark-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn hunks_are_where_lines_differ() {
        let a = lines("a\nb\nc\nd\ne\n");
        let b = lines("a\nB\nc\nd\nx\ny\ne\n");
        assert_eq!(hunks(&a, &b), vec![Hunk { base: (1, 2), now: (1, 2) }, Hunk { base: (4, 4), now: (4, 6) }]);
        assert!(hunks(&a, &a).is_empty());
        assert_eq!(hunks(&lines("a\n"), &lines("")), vec![Hunk { base: (0, 1), now: (0, 0) }]);
    }

    #[test]
    fn keep_one_undo_another() {
        let dir = temp("keep-undo");
        let f = dir.join("main.tex");
        std::fs::write(&f, "one\ntwo\nthree\nfour\n").unwrap();
        let before = Snapshot::take(&dir);
        std::fs::write(&f, "one\nTWO\nthree\nfour\nfive\n").unwrap();
        let after = Snapshot::take(&dir);
        let mut r = Review { changes: changes(&before, &after), ..Default::default() };
        assert_eq!(r.remaining(), 2);
        assert_eq!(r.marks_for(&f), vec![(1, 2, 1, true), (4, 5, 0, false)]);
        // Keep TWO; undo "five".
        r.decide(true).unwrap();
        assert_eq!(r.remaining(), 1);
        r.decide(false).unwrap();
        assert_eq!(r.remaining(), 0);
        assert!(r.changes.is_empty());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one\nTWO\nthree\nfour\n");
    }

    #[test]
    fn undo_all_puts_back_made_and_deleted_files() {
        let dir = temp("undo-all");
        std::fs::write(dir.join("a.tex"), "a\n").unwrap();
        std::fs::write(dir.join("gone.tex"), "kept\n").unwrap();
        let before = Snapshot::take(&dir);
        std::fs::write(dir.join("a.tex"), "A\n").unwrap();
        std::fs::remove_file(dir.join("gone.tex")).unwrap();
        std::fs::write(dir.join("new.tex"), "new\n").unwrap();
        let after = Snapshot::take(&dir);
        let mut r = Review { changes: changes(&before, &after), ..Default::default() };
        assert_eq!(r.changes.len(), 3);
        r.decide_all(false).unwrap();
        assert_eq!(Snapshot::take(&dir), before);
    }

    #[test]
    fn a_review_steps_round_every_change() {
        let dir = temp("step");
        std::fs::write(dir.join("a.tex"), "1\n2\n3\n4\n5\n").unwrap();
        let before = Snapshot::take(&dir);
        std::fs::write(dir.join("a.tex"), "1\nX\n3\n4\nY\n").unwrap();
        std::fs::write(dir.join("b.tex"), "b\n").unwrap();
        let mut r = Review { changes: changes(&before, &Snapshot::take(&dir)), ..Default::default() };
        assert_eq!(r.remaining(), 3);
        assert_eq!(r.ordinal(), 1);
        r.step(false);
        assert_eq!(r.ordinal(), 2);
        r.step(false);
        assert_eq!(r.ordinal(), 3);
        r.step(false);
        assert_eq!(r.ordinal(), 1);
        r.step(true);
        assert_eq!(r.ordinal(), 3);
    }

    #[test]
    fn build_output_and_hidden_folders_are_not_the_project() {
        let dir = temp("passed");
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("build/x.aux"), "aux").unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(dir.join("main.tex"), "x").unwrap();
        std::fs::write(dir.join("fig.png"), [0u8, 159, 146, 150]).unwrap();
        let s = Snapshot::take(&dir);
        assert_eq!(s.files.keys().map(|k| k.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(), vec!["main.tex"]);
    }

    #[test]
    fn file_links_read_as_names() {
        assert_eq!(tidy("I changed [main.tex](file:///p/main.tex) and [refs.bib](file:///p/refs.bib)."), "I changed main.tex and refs.bib.");
        assert_eq!(tidy("a [link](https://x.org) stays"), "a [link](https://x.org) stays");
    }

    #[test]
    fn the_instructions_say_what_the_mode_allows() {
        let cx = Context { root: "/p".into(), file: Some("/p/main.tex".into()), line: 20, log: vec!["Error at main.tex:3: Undefined control sequence".into()] };
        let edit = instructions(Mode::Edit, &cx);
        assert!(edit.contains("main.tex, line 21"));
        assert!(edit.contains("Undefined control sequence"));
        assert!(edit.contains("Do not run commands"));
        assert!(instructions(Mode::Ask, &cx).contains("Do not change any files"));
    }
}

#[cfg(test)]
mod layout_tests {
    use crate::tex::place;
    use egui::{Rect, pos2, vec2};
    use quark_core::prefs::TexLayout;

    fn full(w: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 800.0))
    }

    #[test]
    fn the_islands_tile_the_width() {
        let pl = place(full(1600.0), &TexLayout::default(), 480.0, true);
        let a = pl.agent.unwrap();
        assert_eq!(pl.files.width(), 230.0);
        assert_eq!(a.width(), 340.0);
        assert_eq!(pl.dock.width(), 480.0);
        assert_eq!(pl.editor.left(), pl.files.right() + 10.0);
        assert_eq!(pl.dock.left(), pl.editor.right() + 10.0);
        assert_eq!(a.left(), pl.dock.right() + 10.0);
        assert_eq!(a.right(), 1600.0);
    }

    #[test]
    fn a_dragged_size_is_kept_within_bounds() {
        let l = TexLayout { files: 9000.0, dock: Some(10.0), agent: 50.0, ..Default::default() };
        let pl = place(full(1800.0), &l, 480.0, true);
        assert_eq!(pl.files.width(), 420.0);
        assert_eq!(pl.dock.width(), 260.0);
        assert_eq!(pl.agent.unwrap().width(), 280.0);
    }

    #[test]
    fn a_narrow_window_takes_from_the_dock_then_the_agent() {
        let pl = place(full(1200.0), &TexLayout::default(), 480.0, true);
        assert!(pl.editor.width() >= 360.0 - 0.5, "{:?}", pl.editor);
        assert_eq!(pl.dock.width(), 260.0);
        assert!(pl.agent.unwrap().width() < 340.0);
        let closed = place(full(1200.0), &TexLayout::default(), 480.0, false);
        assert!(closed.agent.is_none());
        assert_eq!(closed.dock.right(), 1200.0);
    }
}

/// Pictures of the agent at work, drawn off screen: run on purpose with
/// `QUARK_SHOTS=<folder> cargo test -p quark-app texagent::shots -- --ignored`.
#[cfg(test)]
mod shots {
    use super::*;
    use quark_ui::theme::ThemeMode;

    #[test]
    #[ignore = "draws pictures; run with QUARK_SHOTS set"]
    fn shot_the_agent() {
        let Some(dir) = crate::shot::folder() else { return };
        let root = std::env::temp_dir().join(format!("quark-shot-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let main = "\\documentclass[11pt]{article}\n\\usepackage{amsmath}\n\\newcommand{\\E}{\\mathbb{E}}\n\\title{Notes on Sampling}\n\n\\begin{document}\n\\maketitle\n\n\\section{Importance sampling}\\label{sec:is}\nWe draw $x_i \\sim q(x)$ and weight each by $w_i = p(x_i)/q(x_i)$, as in\n\\cite{owen2013}. The estimate is unbiased: $\\mathbb{E}_q[\\hat{\\mu}] = \\mu$.\n\n\\subsection{What goes wrong}\nHeavy tails, mostly:\n\\begin{itemize}\n  \\item the weights can be \\emph{heavy-tailed};\n  \\item see Section~\\ref{sec:is}.\n\\end{itemize}\n\n\\end{document}\n";
        std::fs::write(root.join("main.tex"), main).unwrap();
        std::fs::write(root.join("refs.bib"), "@book{owen2013,}\n").unwrap();
        let ready = quark_lab::tex::TexState::Ready {
            tinytex: "v2026.10".into(),
            texlab: "v5.26.0".into(),
            bundle: quark_lab::tex::Bundle::Full,
            size: 241 << 20,
            home: "/Users/you/.quark/tex".into(),
        };
        for (who, dark, name) in [(Who::Claude, true, "agent-claude.png"), (Who::Antigravity, false, "agent-antigravity.png")] {
            let ctx = egui::Context::default();
            theme::install_fonts(&ctx);
            let mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
            theme::apply(&ctx, mode);
            let p = Palette::for_mode(mode);
            std::fs::write(root.join("main.tex"), main).unwrap();
            let mut tex = crate::tex::TexProject::open(&root);
            // What a run did: a paragraph added, an expectation rewritten.
            let before = Snapshot::take(&root);
            let edited = main
                .replace("$\\mathbb{E}_q[\\hat{\\mu}] = \\mu$", "$\\E_q[\\hat{\\mu}] = \\mu$")
                .replace("\\end{itemize}\n", "\\end{itemize}\nThe variance of $\\hat{\\mu}$ is finite only when\n$\\E_q[w^2 f^2] < \\infty$; with heavy-tailed weights it is\nnot, and one draw can dominate the sum~\\cite{owen2013}.\n");
            std::fs::write(root.join("main.tex"), &edited).unwrap();
            tex.session.editor_mut().reread_changed_on_disk();
            let found = changes(&before, &Snapshot::take(&root));
            tex.agent.open = true;
            tex.agent.acknowledged = true;
            tex.agent.who = who;
            tex.agent.turns = vec![
                Turn { mine: true, who, text: "After the itemize, add a short paragraph on why heavy tails blow up the variance, and use \\E for expectations.".into(), ..Default::default() },
                Turn {
                    mine: false,
                    who,
                    text: "I added a paragraph under What goes wrong on when the variance is finite, citing owen2013, and wrote the unbiasedness claim with \\E. It typesets cleanly.".into(),
                    tools: vec!["Read main.tex".into(), "Read refs.bib".into(), "Edited main.tex".into(), "Not allowed: Ran `latexmk -pdf main.tex`".into()],
                    changed: Some((1, 5, 1)),
                    note: None,
                },
            ];
            tex.agent.review = Some(Review { changes: found, at: (0, 1), who });
            tex.set_pane(crate::texvisual::Pane::Source, None);
            if let Some((_, line)) = tex.agent.review.as_ref().and_then(|r| r.here()) {
                let b = tex.session.editor().buffer();
                let at = b.offset_of(quark_text::Position { line, column: 0 });
                tex.session.put_cursor(at);
            }
            crate::shot::shoot(&ctx, egui::vec2(1500.0, 860.0), 4, &dir.join(name), |ui| {
                egui::CentralPanel::default().frame(egui::Frame::new().fill(p.ground).inner_margin(10)).show(ui, |ui| {
                    tex.show(ui, &p, Some(&ready), &mut crate::texdock::Dock::default(), None);
                });
            })
            .unwrap();
        }
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The seams, dragged the way a pointer drags them.
#[cfg(test)]
mod seam_tests {
    use quark_ui::theme::{self, Palette, ThemeMode};

    #[test]
    fn dragging_a_seam_resizes_and_double_click_resets() {
        let root = std::env::temp_dir().join(format!("quark-seam-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\n\\begin{document}\nx\n\\end{document}\n").unwrap();
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let p = Palette::for_mode(ThemeMode::Dark);
        let mut tex = crate::tex::TexProject::open(&root);
        tex.set_pane(crate::texvisual::Pane::Both, None);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0));
        let frame = |tex: &mut crate::tex::TexProject, events: Vec<egui::Event>| {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    tex.show(ui, &p, None, &mut crate::texdock::Dock::default(), None);
                });
            });
            out.textures_delta.clear();
        };
        let press = |at: egui::Pos2, down: bool| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: down, modifiers: Default::default() };
        frame(&mut tex, vec![]);
        // The file list's seam is the gap after its 230 points.
        let at = egui::pos2(235.0, 400.0);
        let to = egui::pos2(315.0, 400.0);
        frame(&mut tex, vec![egui::Event::PointerMoved(at), press(at, true)]);
        frame(&mut tex, vec![egui::Event::PointerMoved(egui::pos2(260.0, 400.0))]);
        frame(&mut tex, vec![egui::Event::PointerMoved(to)]);
        frame(&mut tex, vec![press(to, false)]);
        assert!((tex.layout.files - 310.0).abs() < 1.0, "files now {}", tex.layout.files);
        // Two clicks on it put it back.
        let at = egui::pos2(315.0, 400.0);
        for _ in 0..2 {
            frame(&mut tex, vec![egui::Event::PointerMoved(at), press(at, true)]);
            frame(&mut tex, vec![press(at, false)]);
        }
        assert_eq!(tex.layout.files, 230.0);
    }
}

/// Both agents, for real, on a scratch project: run on purpose with
/// `cargo test -p quark-app texagent::live -- --ignored --nocapture`. Each
/// sends a short prompt on the account its CLI is signed in to.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "asks Claude Code and Antigravity for real"]
    fn each_agent_fixes_a_typo_and_undo_puts_it_back() {
        for who in [Who::Claude, Who::Antigravity] {
            let root = std::env::temp_dir().join(format!("quark-live-{}-{}", who.label().replace(' ', ""), std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let text = "\\documentclass{article}\n\\begin{document}\nHello wrld, this is a test.\n\\end{document}\n";
            std::fs::write(root.join("main.tex"), text).unwrap();
            let mut session = Session::new(Some(&root));
            session.open(&root.join("main.tex")).unwrap();
            let mut agent = Agent { who, mode: Mode::Edit, acknowledged: true, ..Default::default() };
            let cx = Context { root: root.clone(), file: Some(root.join("main.tex")), line: 2, log: Vec::new() };
            agent.ask("Fix the one spelling mistake in main.tex. Change nothing else.".into(), &mut session, &cx).unwrap();
            let ctx = egui::Context::default();
            let start = std::time::Instant::now();
            let mut asks = Vec::new();
            while agent.running() && start.elapsed() < std::time::Duration::from_secs(300) {
                asks.extend(agent.tend(&mut session, &root, &ctx));
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let reply = agent.turns.last().unwrap();
            println!("{}: tools {:?}\n  said: {}\n  note: {:?}", who.label(), reply.tools, reply.text, reply.note);
            assert!(!agent.running(), "{} did not finish", who.label());
            let now = std::fs::read_to_string(root.join("main.tex")).unwrap();
            assert!(now.contains("Hello world"), "{}: {now}", who.label());
            assert!(asks.contains(&AgentAsk::Typeset));
            // The buffer took the change too.
            assert!(session.editor().buffer().text().to_string().contains("Hello world"));
            let review = agent.review.as_ref().expect("a review");
            assert_eq!(review.remaining(), 1, "{:?}", review.changes);
            agent.decide(false, true, &mut session);
            assert_eq!(std::fs::read_to_string(root.join("main.tex")).unwrap(), text);
            assert!(agent.review.is_none());
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
