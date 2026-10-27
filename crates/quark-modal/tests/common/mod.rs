//! Proton's tests drove its whole workspace; these drive Quark's session
//! through a stand-in of the same shape, so they read as they did there.
#![allow(dead_code)]

use std::path::PathBuf;

pub use quark_modal::action::{Action, Object, Verb};
pub use quark_modal::key::Key;
pub use quark_modal::session::{Said, Session};
pub use quark_modal::split::Direction;
pub use quark_modal::vim::{Outcome, Vim, VimMode};

#[derive(Debug)]
pub enum Command {
    Key { key: Key },
    /// A click: the cursor put there.
    PlaceCursor { line: usize, column: usize },
    /// A drag to there: a selection.
    SelectTo { line: usize, column: usize },
    Perform { action: Action },
    CloseBuffer { index: usize, force: bool },
    /// How much of a split the window shows.
    Viewport { view: usize, first_line: usize, lines: usize },
    /// Every split, drawn again.
    Sync,
    EditorPaste { text: String },
    /// A file, as a path typed: against the project's folder.
    OpenBuffer { path: String },
    Save,
    Split { vertical: bool },
    CloseSplit,
    FocusSplit { index: usize },
    StepSplit { direction: Direction },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Notice { text: String, level: NoticeLevel },
    VimMode { mode: VimMode },
    CmdlineRun { text: String },
    Clipboard { text: String },
    /// What a split shows: its lines on screen, and where the search
    /// matches on each.
    Viewport { view: usize, first_line: usize, lines: Vec<String>, matches: Vec<Vec<Hit>> },
    /// The splits: what each shows, which has focus, and which way.
    Splits { views: Vec<ViewName>, focused: usize, vertical: bool },
    Cursor { view: usize, line: usize, column: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewName {
    pub name: String,
}

pub struct Workspace {
    session: Session,
    root: PathBuf,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new()
    }
}

impl Workspace {
    pub fn new() -> Workspace {
        Workspace { session: Session::new(None), root: PathBuf::new() }
    }

    /// An editor for the files of the project in `root`.
    pub fn in_folder(root: impl AsRef<std::path::Path>) -> Workspace {
        let root = root.as_ref();
        Workspace { session: Session::new(Some(root)), root: root.to_path_buf() }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn rooted(root: impl AsRef<std::path::Path>) -> Workspace {
        Workspace::in_folder(root)
    }

    pub fn vim_mode(&self) -> VimMode {
        self.session.mode()
    }

    pub fn is_modified(&self) -> bool {
        self.session.editor().buffer().is_modified()
    }

    pub fn editor(&self) -> &quark_modal::editor::Editor {
        self.session.editor()
    }

    pub fn paste_text(&mut self, text: &str) {
        self.session.paste(text);
    }

    pub fn apply(&mut self, command: Command) -> Vec<Event> {
        if let Command::Sync = command {
            return self.views();
        }
        let was = self.session.mode();
        let seen = self.views();
        let said = match command {
            Command::Key { key } => self.session.key(&key),
            Command::PlaceCursor { line, column } => {
                let view = self.session.splits().focused();
                self.session.click(view, line, column);
                Vec::new()
            }
            Command::SelectTo { line, column } => {
                self.session.select_to(line, column);
                Vec::new()
            }
            Command::Perform { action } => {
                self.session.perform(&action);
                Vec::new()
            }
            Command::CloseBuffer { index, force } => self.session.close_buffer(index, force),
            Command::Viewport { view, first_line, lines } => {
                self.session.set_lines(view, lines);
                let first = self.session.view_of(view).map_or(0, |v| v.2);
                self.session.scroll(view, first_line as i64 - first as i64);
                Vec::new()
            }
            Command::Sync => Vec::new(),
            Command::EditorPaste { text } => {
                self.session.paste(&text);
                Vec::new()
            }
            Command::OpenBuffer { path } => match self.session.open(&self.root.join(&path)) {
                Ok(()) => Vec::new(),
                Err(e) => vec![Said::Warn(format!("{path}: {e}"))],
            },
            Command::Save => self.session.save(),
            Command::Split { vertical } => self.session.split(vertical),
            Command::CloseSplit => self.session.close_split(),
            Command::FocusSplit { index } => {
                self.session.focus_split(index);
                Vec::new()
            }
            Command::StepSplit { direction } => self.session.step_split(direction),
        };
        let mut events: Vec<Event> = said
            .into_iter()
            .filter_map(|s| match s {
                Said::Info(text) => Some(Event::Notice { text, level: NoticeLevel::Info }),
                Said::Warn(text) => Some(Event::Notice { text, level: NoticeLevel::Warn }),
                Said::Wrote(path) => Some(Event::Notice {
                    text: format!("wrote {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    level: NoticeLevel::Info,
                }),
                Said::Ex(text) => Some(Event::CmdlineRun { text }),
                Said::Leader(_) => None,
            })
            .collect();
        if let Some(text) = self.session.take_clipboard() {
            events.push(Event::Clipboard { text });
        }
        if self.session.mode() != was {
            events.push(Event::VimMode { mode: self.session.mode() });
        }
        // The window is drawn again when anything happened — the view, the
        // mode, something said — and not when nothing did.
        let now = self.views();
        if now != seen || !events.is_empty() {
            events.extend(now);
        }
        events
    }

    /// What the window would draw now, from the session's own state.
    fn views(&self) -> Vec<Event> {
        let s = &self.session;
        let count = s.splits().len();
        let buffers = s.editor().buffers();
        let mut events = vec![Event::Splits {
            views: (0..count)
                .map(|v| ViewName { name: s.view_of(v).and_then(|(b, ..)| buffers.get(b)).map(|b| b.name()).unwrap_or_default() })
                .collect(),
            focused: s.splits().focused(),
            vertical: s.splits().is_vertical(),
        }];
        for view in 0..count {
            let Some((index, cursor, first)) = s.view_of(view) else { continue };
            let Some(buf) = buffers.get(index) else { continue };
            let height = s.splits().at(view).map_or(80, |v| v.lines);
            let lines: Vec<String> = buf
                .lines(first, height)
                .into_iter()
                .map(|l| l.trim_end_matches(['\n', '\r']).to_string())
                .collect();
            let matches = lines
                .iter()
                .map(|l| match s.pattern() {
                    Some(p) => quark_text::search::find_in(l, p)
                        .into_iter()
                        .map(|(a, b)| Hit { start: a as u32, end: b as u32 })
                        .collect(),
                    None => Vec::new(),
                })
                .collect();
            events.push(Event::Viewport { view, first_line: first, lines, matches });
            let at = buf.position(cursor.offset);
            events.push(Event::Cursor { view, line: at.line, column: at.column });
        }
        events
    }
}
