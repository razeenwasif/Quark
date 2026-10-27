//! LaTeX's visual editor: the file in focus drawn as a document — its
//! headings, paragraphs, equations, lists and figures — in place of the
//! source. From Proton's `ui/tex-visual.js`, drawn natively.
//!
//! The file is cut into blocks (`quark_tex::blocks`), each with its span. A
//! paragraph or a heading's title is edited in place: what is typed is
//! written back as LaTeX into the block's span — only what changed, one
//! step to undo — once typing rests. Commands it does not draw are chips
//! that go back as they came, and a block that would not come back exactly
//! is drawn as its source, not edited here. An equation is edited as its
//! source, typeset again when it is put down.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    Color32, CornerRadius, FontId, Key, Modifiers, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};
use quark_modal::action::{Action, Object, Verb};
use quark_modal::session::Session;
use quark_tex::blocks::Block;
use quark_ui::theme::{self, Palette};

use crate::texinline::{self, Body, Chip, Piece, Typed};

/// How long typing rests before it is written into the source.
pub(crate) const REST: Duration = Duration::from_millis(700);
/// The page's widest column of words.
const PAGE_W: f32 = 700.0;
/// What a paragraph's words are drawn at.
const WORDS: f32 = 15.0;

/// How a block is drawn.
#[derive(Debug, Clone)]
enum Drawn {
    /// A blank line, or `\end{document}` and after.
    Nothing,
    Preamble,
    Heading {
        head: String,
        title: Vec<Piece>,
        tail: String,
    },
    Paragraph(Vec<Piece>),
    Math,
    List {
        numbered: bool,
        items: Vec<Result<Vec<Piece>, String>>,
    },
    Figure {
        caption: Option<Result<Vec<Piece>, String>>,
    },
    /// As its source.
    Raw,
}

impl Drawn {
    fn of(block: &Block) -> Drawn {
        let drawn =
            |text: &str| texinline::parse(text.trim()).ok_or_else(|| text.trim().to_string());
        match block.kind.as_str() {
            "space" | "end" => Drawn::Nothing,
            "preamble" => Drawn::Preamble,
            "heading" => match texinline::heading(&block.text) {
                Some((head, title, tail)) => Drawn::Heading { head, title, tail },
                None => Drawn::Raw,
            },
            "paragraph" => texinline::parse(&block.text).map_or(Drawn::Raw, Drawn::Paragraph),
            "math" => Drawn::Math,
            "list" => {
                let (numbered, items) = texinline::list_items(&block.text);
                Drawn::List {
                    numbered,
                    items: items.iter().map(|i| drawn(i)).collect(),
                }
            }
            "figure" => Drawn::Figure {
                caption: texinline::caption(&block.text).map(|c| drawn(&c)),
            },
            _ => Drawn::Raw,
        }
    }

    /// Whether it takes room on the page.
    fn shows(&self) -> bool {
        !matches!(self, Drawn::Nothing)
    }

    /// Its words, when they are edited here.
    fn pieces(&self) -> Option<&[Piece]> {
        match self {
            Drawn::Heading { title, .. } => Some(title),
            Drawn::Paragraph(pieces) => Some(pieces),
            _ => None,
        }
    }

    /// Its source, with `pieces` for its words.
    fn written(&self, pieces: &[Piece]) -> Option<String> {
        match self {
            Drawn::Heading { head, tail, .. } => {
                Some(format!("{head}{}{tail}", texinline::to_latex(pieces)))
            }
            Drawn::Paragraph(_) => Some(texinline::to_latex(pieces)),
            _ => None,
        }
    }

    /// How far into its source its words begin.
    fn words_from(&self) -> usize {
        match self {
            Drawn::Heading { head, .. } => head.chars().count(),
            _ => 0,
        }
    }
}

/// The block being typed in, and what is typed not yet written.
#[derive(Debug, Clone)]
struct Typing {
    block: usize,
    pieces: Vec<Piece>,
    /// When it was last typed in, while what is typed is not yet written.
    since: Option<Instant>,
}

/// What the editor's area shows: the source, the page, or both side by
/// side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Pane {
    Source,
    #[default]
    Both,
    Visual,
}

impl Pane {
    /// Whether the page shows.
    pub(crate) fn page(self) -> bool {
        self != Pane::Source
    }

    /// ⌘E: both, then the page alone, then the source alone.
    pub(crate) fn next(self) -> Pane {
        match self {
            Pane::Both => Pane::Visual,
            Pane::Visual => Pane::Source,
            Pane::Source => Pane::Both,
        }
    }
}

/// What the view asks of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Asked {
    None,
    /// To the source, the cursor at this character.
    Source(usize),
    /// ⌘E: the next of the panes, the cursor at this character.
    Step(usize),
    /// The cursor put here and shown in the PDF.
    Forward(usize),
    /// The cursor put here and the cite picker opened.
    Cite(usize),
}

/// The visual editor's state.
#[derive(Default)]
pub(crate) struct Visual {
    pub pane: Pane,
    /// Whether something on the page has the keys.
    pub(crate) focused: bool,
    /// The block the source's cursor was last seen in, so the page beside
    /// it follows it when it moves to another.
    followed: Option<usize>,
    /// The file and its version the blocks were cut from.
    cut: Option<(Option<PathBuf>, u64)>,
    blocks: Vec<Block>,
    drawn: Vec<Drawn>,
    pub(crate) macros: Vec<(String, String)>,
    typing: Option<Typing>,
    /// An equation's source, being edited, and whether it is new this frame.
    equation: Option<(usize, String, bool)>,
    /// Once the blocks are cut again, the caret goes to the block that
    /// begins here.
    pub(crate) caret_at: Option<usize>,
    /// Pictures, by the file they were read from — or why they were not.
    pictures: HashMap<PathBuf, Result<(egui::TextureHandle, egui::Vec2), String>>,
    /// What went wrong last, to say.
    pub said: Option<String>,
    /// The equations, typeset by TeX.
    pub math: crate::texmath::MathImages,
}

impl Visual {
    /// The blocks cut again if the file in focus has changed since.
    pub(crate) fn refresh(&mut self, session: &Session) {
        let buffer = session.editor().buffer();
        let key = (buffer.path().map(Path::to_path_buf), buffer.version());
        if self.cut.as_ref() == Some(&key) {
            return;
        }
        let same_file = self.cut.as_ref().map(|c| &c.0) == Some(&key.0);
        self.cut = Some(key);
        self.blocks = quark_tex::blocks::blocks(&buffer.contents());
        self.drawn = self.blocks.iter().map(Drawn::of).collect();
        self.macros = self
            .blocks
            .first()
            .filter(|b| b.kind == "preamble")
            .map(|b| quark_tex::blocks::macros(&b.text))
            .unwrap_or_default();
        self.math.set_macros(&self.macros);
        // The block typed in goes on being typed in, as it now stands.
        self.typing = self.typing.take().filter(|_| same_file).and_then(|t| {
            let pieces = self.drawn.get(t.block)?.pieces()?.to_vec();
            Some(Typing {
                block: t.block,
                pieces,
                since: None,
            })
        });
        if !same_file {
            self.equation = None;
        }
    }

    /// The pictures read forgotten: a shot's frame drawn nowhere read them
    /// into another context.
    #[cfg(test)]
    pub(crate) fn forget_pictures(&mut self) {
        self.pictures.clear();
    }

    /// Whether there is typing not yet in the source.
    pub(crate) fn pending(&self) -> bool {
        self.typing.as_ref().is_some_and(|t| t.since.is_some())
    }

    /// What is typed, written into the source now.
    pub(crate) fn flush(&mut self, session: &mut Session) {
        let Some(typing) = self.typing.as_mut().filter(|t| t.since.is_some()) else {
            return;
        };
        typing.since = None;
        let i = typing.block;
        let (Some(block), Some(drawn)) = (self.blocks.get(i), self.drawn.get(i)) else {
            return;
        };
        let Some(latex) = drawn.written(&typing.pieces) else {
            return;
        };
        if !session.replace(
            block.start as usize,
            block.end as usize,
            &block.text,
            &latex,
        ) {
            self.said =
                Some("The source changed under that edit: it is drawn again as it stands.".into());
        }
        self.refresh(session);
    }

    /// Typing that has rested, written.
    pub(crate) fn tend(&mut self, session: &mut Session) {
        if self
            .typing
            .as_ref()
            .and_then(|t| t.since)
            .is_some_and(|s| s.elapsed() >= REST)
        {
            self.flush(session);
        }
    }

    /// ⌘Z or ⇧⌘Z: what is typed written, then a step undone or done again.
    pub(crate) fn undo(&mut self, session: &mut Session, redo: bool) {
        self.flush(session);
        session.perform(&Action::new(
            if redo { Verb::Redo } else { Verb::Undo },
            Object::None,
            1,
        ));
        self.refresh(session);
    }

    /// Where in the file a place in the words typed in is.
    fn source_of(&self, block: usize, caret: usize) -> Option<usize> {
        let typing = self.typing.as_ref().filter(|t| t.block == block)?;
        let start = self.blocks.get(block)?.start as usize;
        Some(
            start
                + self.drawn.get(block)?.words_from()
                + texinline::source_at(&typing.pieces, caret),
        )
    }

    /// The page: each block in turn, in a column the width of a page.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        session: &mut Session,
        root: &Path,
        area: Rect,
    ) -> Asked {
        self.refresh(session);
        self.tend(session);
        let mut asked = Asked::None;
        // The block the source's cursor is in: marked beside the source,
        // and scrolled to when the cursor goes to another — unless the
        // page has the keys, when the page leads.
        let beside = self.pane == Pane::Both;
        let cursor = session.editor().buffer().cursor.offset;
        let here_block = self
            .blocks
            .iter()
            .position(|b| (b.start as usize) <= cursor && cursor < b.end as usize)
            .or(self.blocks.len().checked_sub(1));
        let mut scroll_to = None;
        if beside && here_block != self.followed {
            if !self.focused {
                scroll_to = here_block;
            }
            self.followed = here_block;
        }
        self.focused = self.equation.is_some();
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
        let buffer = session.editor().buffer();
        let file = buffer.path().map(Path::to_path_buf);
        if !file.as_deref().is_some_and(crate::texvisual::draws) {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("The visual editor draws .tex files: open one from the list.")
                        .font(theme::body(13.0))
                        .color(p.text_faint),
                );
            });
            return asked;
        }
        let here = file
            .as_deref()
            .and_then(Path::parent)
            .unwrap_or(root)
            .to_path_buf();
        egui::ScrollArea::vertical()
            .id_salt("tex-visual")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                let width = (ui.available_width() - 48.0).clamp(200.0, PAGE_W);
                let margin = ((ui.available_width() - width) / 2.0).max(0.0);
                ui.add_space(28.0);
                ui.horizontal(|ui| {
                    ui.add_space(margin);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for i in 0..self.blocks.len() {
                            let top = ui.cursor().top();
                            let got = self.block(ui, p, session, i, width, &here, root);
                            if got != Asked::None {
                                asked = got;
                            }
                            let rect = Rect::from_x_y_ranges(
                                ui.max_rect().x_range(),
                                top..=ui.cursor().top().max(top + 1.0),
                            );
                            if beside && here_block == Some(i) && self.drawn[i].shows() {
                                let bar = Rect::from_min_size(
                                    pos2(rect.left() - 14.0, rect.top()),
                                    vec2(3.0, rect.height() - 6.0),
                                );
                                ui.painter().rect_filled(
                                    bar,
                                    CornerRadius::same(2),
                                    p.accent_alpha(0x90),
                                );
                            }
                            if scroll_to == Some(i) {
                                ui.scroll_to_rect(rect, Some(egui::Align::Center));
                            }
                        }
                        ui.add_space(120.0);
                    });
                });
            });
        // Typing that waits to be written wakes the window when it should be.
        if self.pending() {
            ui.ctx().request_repaint_after(REST);
        }
        asked
    }

    fn block(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        session: &mut Session,
        i: usize,
        width: f32,
        here: &Path,
        root: &Path,
    ) -> Asked {
        let drawn = self.drawn[i].clone();
        let block = self.blocks[i].clone();
        let source = |r: &egui::Response| {
            if r.double_clicked() {
                Asked::Source(block.start as usize)
            } else {
                Asked::None
            }
        };
        match &drawn {
            Drawn::Nothing => {
                // A blank line between blocks: the space between paragraphs.
                if block.kind == "space" {
                    ui.add_space(4.0);
                }
                Asked::None
            }
            Drawn::Preamble => {
                let lines = block.text.lines().count();
                let r = egui::CollapsingHeader::new(
                    RichText::new(format!("Preamble · {lines} lines"))
                        .font(theme::strong(12.0))
                        .color(p.text_faint),
                )
                .id_salt(("tex-visual-preamble", i))
                .show(ui, |ui| {
                    raw(ui, p, &block.text);
                });
                ui.add_space(12.0);
                source(&r.header_response)
            }
            Drawn::Heading { .. } | Drawn::Paragraph(_) => {
                let (size, strong) = match (&drawn, block.level) {
                    (Drawn::Paragraph(_), _) => (WORDS, false),
                    (_, Some(0)) => (28.0, true),
                    (_, Some(1)) => (25.0, true),
                    (_, Some(2)) => (21.0, true),
                    (_, Some(3)) => (18.0, true),
                    (_, Some(4)) => (16.0, true),
                    _ => (WORDS, true),
                };
                if matches!(drawn, Drawn::Heading { .. }) {
                    ui.add_space(size * 0.7);
                }
                let asked = self.editable(ui, p, session, i, width, size, strong);
                ui.add_space(if strong { 6.0 } else { 8.0 });
                asked
            }
            Drawn::Math => self.equation_block(ui, p, session, i, width),
            Drawn::List { numbered, items } => {
                let r = ui
                    .vertical(|ui| {
                        for (n, item) in items.iter().enumerate() {
                            ui.horizontal_top(|ui| {
                                let mark = if *numbered {
                                    format!("{}.", n + 1)
                                } else {
                                    "•".into()
                                };
                                ui.add_sized(
                                    vec2(22.0, WORDS * 1.4),
                                    egui::Label::new(
                                        RichText::new(mark)
                                            .font(theme::body(WORDS))
                                            .color(p.text_muted),
                                    ),
                                );
                                ui.vertical(|ui| {
                                    ui.set_width(width - 30.0);
                                    words(ui, p, item, WORDS, false);
                                });
                            });
                            ui.add_space(3.0);
                        }
                    })
                    .response
                    .interact(Sense::click());
                ui.add_space(8.0);
                source(&r)
            }
            Drawn::Figure { caption } => {
                let r = ui
                    .vertical_centered(|ui| {
                        match block
                            .image
                            .as_deref()
                            .map(|name| self.picture(ui.ctx(), here, root, name))
                        {
                            Some(Ok((texture, size))) => {
                                let w = size.x.min(width * 0.8);
                                let h = size.y * w / size.x.max(1.0);
                                ui.add(egui::Image::new((texture.id(), vec2(w, h))));
                            }
                            Some(Err(why)) => {
                                ui.label(
                                    RichText::new(why)
                                        .font(theme::body(12.0))
                                        .color(p.text_faint),
                                );
                            }
                            None => {
                                ui.label(
                                    RichText::new("a figure")
                                        .font(theme::body(12.0))
                                        .color(p.text_faint),
                                );
                            }
                        }
                        if let Some(caption) = caption {
                            ui.add_space(6.0);
                            let caption = caption.clone().map(|mut pieces| {
                                pieces.insert(
                                    0,
                                    Piece {
                                        body: Body::Text("Figure: ".into()),
                                        marks: vec![texinline::Mark {
                                            cmd: "textbf".into(),
                                            id: 0,
                                        }],
                                    },
                                );
                                pieces
                            });
                            words(ui, p, &caption, 13.0, false);
                        }
                    })
                    .response
                    .interact(Sense::click());
                ui.add_space(12.0);
                source(&r)
            }
            Drawn::Raw => {
                let r = raw(ui, p, block.text.trim_end_matches('\n'));
                ui.add_space(8.0);
                source(&r)
            }
        }
    }

    /// A paragraph or a heading's title, typed in.
    #[allow(clippy::too_many_arguments)]
    fn editable(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        session: &mut Session,
        i: usize,
        width: f32,
        size: f32,
        strong: bool,
    ) -> Asked {
        let id = ui.id().with(("tex-visual-text", i));
        let heading = matches!(self.drawn[i], Drawn::Heading { .. });
        let mine = self.typing.as_ref().filter(|t| t.block == i);
        let pieces: Vec<Piece> = match mine {
            Some(t) => t.pieces.clone(),
            None => self.drawn[i]
                .pieces()
                .map(<[Piece]>::to_vec)
                .unwrap_or_default(),
        };
        // The caret, put here once a new paragraph is cut.
        if self.caret_at == Some(self.blocks[i].start as usize) {
            self.caret_at = None;
            ui.memory_mut(|m| m.request_focus(id));
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), id);
        }
        let focused = ui.memory(|m| m.has_focus(id));
        let mut asked = Asked::None;
        if focused {
            self.focused = true;
            if self.typing.as_ref().map(|t| t.block) != Some(i) {
                self.flush(session);
                // Typing in the source beside ends, as one step to undo; its
                // cursor comes here, so it shows what is typed.
                if session.mode() != quark_modal::vim::VimMode::Normal {
                    session.key(&quark_modal::key::Key::named("Escape"));
                }
                session.put_cursor(self.blocks[i].start as usize);
                self.followed = Some(i);
                let pieces = self.drawn[i]
                    .pieces()
                    .map(<[Piece]>::to_vec)
                    .unwrap_or_default();
                self.typing = Some(Typing {
                    block: i,
                    pieces,
                    since: None,
                });
            }
            let range = egui::text_edit::TextEditState::load(ui.ctx(), id)
                .and_then(|s| s.cursor.char_range());
            let caret = range.map_or(0, |r| r.primary.index.0);
            let (from, to) = range.map_or((0, 0), |r| {
                let (a, b) = (r.primary.index.0, r.secondary.index.0);
                (a.min(b), a.max(b))
            });
            let keys = ui.input_mut(|input| {
                let mut k = [false; 9];
                k[0] = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
                k[1] = input.consume_key(Modifiers::COMMAND, Key::Z);
                k[2] = input.consume_key(Modifiers::COMMAND, Key::B);
                k[3] = input.consume_key(Modifiers::COMMAND, Key::I);
                k[4] = input.consume_key(Modifiers::COMMAND, Key::Enter);
                k[5] = input.consume_key(Modifiers::NONE, Key::Enter);
                k[6] = input.consume_key(Modifiers::NONE, Key::Escape);
                k[7] = input.consume_key(Modifiers::COMMAND, Key::E);
                k[8] = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C);
                k
            });
            let at = |me: &Self| {
                me.source_of(i, caret)
                    .unwrap_or(me.blocks[i].start as usize)
            };
            if keys[0] || keys[1] {
                self.undo(session, keys[0]);
                return Asked::None;
            }
            if keys[2] || keys[3] {
                if let Some(t) = self.typing.as_mut() {
                    t.pieces = texinline::toggle(
                        &t.pieces,
                        from,
                        to,
                        if keys[2] { "textbf" } else { "emph" },
                    );
                    t.since = Some(Instant::now());
                }
            }
            if keys[4] {
                asked = Asked::Forward(at(self));
            }
            if keys[6] {
                asked = Asked::Source(at(self));
            }
            if keys[7] {
                asked = Asked::Step(at(self));
            }
            if keys[8] {
                asked = Asked::Cite(at(self));
            }
            if keys[5] && !heading {
                // A new paragraph: a blank line where the caret is, and the
                // caret on what follows it once the blocks are cut again.
                let into = self
                    .typing
                    .as_ref()
                    .map_or(0, |t| texinline::source_at(&t.pieces, caret));
                self.flush(session);
                let block = &self.blocks[i];
                let text: Vec<char> = block.text.chars().collect();
                let split: String = text[..into.min(text.len())]
                    .iter()
                    .chain(['\n', '\n'].iter())
                    .chain(text[into.min(text.len())..].iter())
                    .collect();
                let start = block.start as usize;
                if session.replace(start, block.end as usize, &block.text.clone(), &split) {
                    self.caret_at = Some(start + into + 2);
                    self.refresh(session);
                }
                return Asked::None;
            }
            if asked != Asked::None {
                self.flush(session);
                return asked;
            }
        }
        let pieces = match self.typing.as_ref().filter(|t| t.block == i) {
            Some(t) => t.pieces.clone(),
            None => pieces,
        };
        let mut text = texinline::display(&pieces);
        let shown = text.clone();
        let palette = p.clone();
        let mut layouter = |ui: &Ui, buffer: &dyn egui::TextBuffer, wrap: f32| {
            let mut job = if buffer.as_str() == shown {
                layout(&pieces, size, strong, &palette)
            } else {
                LayoutJob::simple(
                    buffer.as_str().to_string(),
                    font(size, strong, false),
                    palette.text,
                    wrap,
                )
            };
            job.wrap.max_width = wrap;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let out = egui::TextEdit::multiline(&mut text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(vec2(0.0, 0.0))
            .desired_width(width)
            .desired_rows(1)
            .layouter(&mut layouter)
            .show(ui);
        if text != shown {
            let caret = out
                .cursor_range
                .map_or(text.chars().count(), |r| r.primary.index.0);
            let current = self
                .typing
                .as_ref()
                .filter(|t| t.block == i)
                .map(|t| t.pieces.clone())
                .unwrap_or(pieces);
            match texinline::typed(&current, &text, caret) {
                Typed::Changed(new) => {
                    self.typing = Some(Typing {
                        block: i,
                        pieces: new,
                        since: Some(Instant::now()),
                    });
                }
                Typed::Refused => {
                    self.said =
                        Some("A command is changed in the source: double-click it, or ⌘E.".into());
                }
                Typed::Same => {}
            }
        }
        if out.response.lost_focus() {
            self.flush(session);
        }
        asked
    }

    /// A displayed equation: typeset, or its source; a click edits it.
    fn equation_block(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        session: &mut Session,
        i: usize,
        width: f32,
    ) -> Asked {
        let block = self.blocks[i].clone();
        if let Some((at, text, first)) = self.equation.as_mut().filter(|e| e.0 == i) {
            let _ = at;
            let id = ui.id().with(("tex-visual-equation", i));
            let keep = ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::Enter));
            let cancel = ui.memory(|m| m.has_focus(id))
                && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
            let r = egui::Frame::new()
                .fill(p.recess)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .id(id)
                            .font(FontId::monospace(13.0))
                            .frame(egui::Frame::NONE)
                            .desired_width(width - 16.0)
                            .desired_rows(2),
                    )
                });
            if std::mem::take(first) {
                r.inner.request_focus();
            }
            let done = keep || cancel || r.inner.lost_focus();
            if done {
                let (_, text, _) = self.equation.take().expect("an equation");
                let latex = format!(
                    "{text}{}",
                    if block.text.ends_with('\n') { "\n" } else { "" }
                );
                if !cancel && latex != block.text {
                    self.flush(session);
                    if !session.replace(
                        block.start as usize,
                        block.end as usize,
                        &block.text,
                        &latex,
                    ) {
                        self.said = Some(
                            "The source changed under that edit: it is drawn again as it stands."
                                .into(),
                        );
                    }
                    self.refresh(session);
                }
            }
            ui.add_space(10.0);
            return Asked::None;
        }
        let tex = texinline::display_math(&block.text);
        let shown = self.math.shown(ui.ctx(), &tex);
        let r = ui
            .vertical_centered(|ui| {
                ui.add_space(4.0);
                match shown {
                    crate::texmath::Shown::Ready(texture, size) => {
                        let size = if size.x > width {
                            size * (width / size.x)
                        } else {
                            size
                        };
                        ui.add(egui::Image::new((texture.id(), size)).tint(p.text));
                    }
                    crate::texmath::Shown::Waiting => {
                        ui.label(
                            RichText::new(&tex)
                                .font(FontId::monospace(13.0))
                                .color(p.text_faint),
                        );
                    }
                    crate::texmath::Shown::Failed(why) => {
                        ui.label(
                            RichText::new(&tex)
                                .font(FontId::monospace(13.0))
                                .color(p.text_muted),
                        );
                        ui.label(RichText::new(why).font(theme::body(11.5)).color(p.warning));
                    }
                }
                ui.add_space(4.0);
            })
            .response
            .interact(Sense::click())
            .on_hover_text("Click to edit its source");
        if r.clicked() {
            self.flush(session);
            self.equation = Some((i, block.text.trim_end_matches('\n').to_string(), true));
        }
        ui.add_space(10.0);
        Asked::None
    }

    /// A figure's picture, read once: PNG or JPEG, or a PDF's first page.
    fn picture(
        &mut self,
        ctx: &egui::Context,
        here: &Path,
        root: &Path,
        name: &str,
    ) -> Result<(egui::TextureHandle, egui::Vec2), String> {
        let Some(path) = picture_file(here, root, name) else {
            return Err(format!("{name}: not found"));
        };
        if let Some(found) = self.pictures.get(&path) {
            return found.clone();
        }
        let read = read_picture(&path).map(|image| {
            let size = vec2(image.size[0] as f32, image.size[1] as f32) / 2.0;
            (
                ctx.load_texture(
                    format!("tex-picture-{}", path.display()),
                    image,
                    egui::TextureOptions::LINEAR,
                ),
                size,
            )
        });
        self.pictures.insert(path, read.clone());
        read
    }
}

/// Whether a file is one the visual editor draws.
pub(crate) fn draws(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tex"))
}

/// The file a figure names: beside the file that names it, or in the
/// project, as named or with the extensions graphicx tries.
pub(crate) fn picture_file(here: &Path, root: &Path, name: &str) -> Option<PathBuf> {
    let named = Path::new(name);
    let mut tries = Vec::new();
    for dir in [here, root] {
        tries.push(dir.join(named));
        if named.extension().is_none() {
            for ext in ["pdf", "png", "jpg", "jpeg"] {
                tries.push(dir.join(named).with_extension(ext));
            }
        }
    }
    tries.into_iter().find(|p| p.is_file())
}

/// A picture as pixels, at twice the points it is drawn at.
fn read_picture(path: &Path) -> Result<egui::ColorImage, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let rgba = if ext == "pdf" {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let doc = quark_pdf::doc::Document::from_bytes(bytes, None).map_err(|e| e.to_string())?;
        let drawn = quark_pdf::render::render_page(
            &doc,
            &quark_pdf::render::RenderRequest {
                page: 0,
                scale: 2.0,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        image::RgbaImage::from_raw(drawn.width, drawn.height, drawn.rgba)
            .ok_or("a page drawn wrong")?
    } else {
        image::open(path).map_err(|e| e.to_string())?.to_rgba8()
    };
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

/// The font words are drawn in.
fn font(size: f32, strong: bool, mono: bool) -> FontId {
    if mono {
        FontId::monospace(size * 0.9)
    } else if strong {
        theme::heavy(size)
    } else {
        theme::body(size)
    }
}

/// Pieces laid out: words in their marks, chips as what they hold.
pub(crate) fn layout(pieces: &[Piece], size: f32, strong: bool, p: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    for piece in pieces {
        let mut format = TextFormat {
            font_id: font(size, strong || piece.bold(), piece.mono()),
            color: p.text,
            italics: piece.italic(),
            ..Default::default()
        };
        if piece.underline() {
            format.underline = Stroke::new(1.0, p.text_muted);
        }
        match &piece.body {
            Body::Text(text) => {
                let shown: String = text
                    .chars()
                    .map(|c| if c == '\n' { ' ' } else { c })
                    .collect();
                job.append(&shown, 0.0, format);
            }
            Body::Chip { label, chip, .. } => {
                match chip {
                    Chip::Pointing => {
                        format.color = p.accent_text;
                        format.background = p.accent_soft;
                    }
                    Chip::Math => {
                        format.italics = true;
                        format.color = p.text;
                        format.background = p.wash(0x10);
                    }
                    Chip::Command | Chip::Group => {
                        format.font_id = FontId::monospace(size * 0.82);
                        format.color = p.text_muted;
                        format.background = p.wash(0x10);
                    }
                    Chip::Comment => format.color = p.text_faint,
                    Chip::Tie => {}
                }
                job.append(label, 0.0, format);
            }
        }
    }
    job
}

/// Words drawn, not typed in: pieces, or their source when they are not.
fn words(ui: &mut Ui, p: &Palette, words: &Result<Vec<Piece>, String>, size: f32, strong: bool) {
    match words {
        Ok(pieces) => {
            let mut job = layout(pieces, size, strong, p);
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).selectable(false));
        }
        Err(src) => {
            ui.add(
                egui::Label::new(
                    RichText::new(src)
                        .font(FontId::monospace(size * 0.85))
                        .color(p.text_muted),
                )
                .selectable(false),
            );
        }
    }
}

/// A block as its source.
fn raw(ui: &mut Ui, p: &Palette, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(p.wash(0x08))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .font(FontId::monospace(12.5))
                        .color(p.text_muted),
                )
                .selectable(false),
            );
        })
        .response
        .interact(Sense::click())
        .on_hover_text("Double-click to edit it in the source")
}

/// The panes' switch, at `rect`: which was chosen, if one was.
pub(crate) fn switch(ui: &mut Ui, p: &Palette, rect: Rect, pane: Pane) -> Option<Pane> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), p.recess);
    let third = rect.width() / 3.0;
    let mut chosen = None;
    for (n, (label, this)) in [
        ("Source", Pane::Source),
        ("Both", Pane::Both),
        ("Visual", Pane::Visual),
    ]
    .into_iter()
    .enumerate()
    {
        let r = Rect::from_min_size(
            pos2(rect.left() + third * n as f32, rect.top()),
            vec2(third, rect.height()),
        )
        .shrink(2.0);
        let response = ui.interact(r, ui.id().with(("tex-switch", n)), Sense::click());
        let mine = this == pane;
        if mine {
            painter.rect_filled(r, CornerRadius::same(5), p.accent_soft);
        }
        let colour: Color32 = if mine {
            p.accent_text
        } else if response.hovered() {
            p.text
        } else {
            p.text_muted
        };
        painter.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::strong(11.0),
            colour,
        );
        if response.clicked() && !mine {
            chosen = Some(this);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_on(name: &str, text: &str) -> (Session, PathBuf) {
        let root = crate::tex::tests::project(name);
        let file = root.join("visual.tex");
        std::fs::write(&file, text).unwrap();
        let mut session = Session::new(Some(&root));
        session.open(&file).unwrap();
        (session, root)
    }

    const DOC: &str = "\\documentclass{article}\n\\newcommand{\\R}{\\mathbb{R}}\n\\begin{document}\n\\section{Intro}\nSome \\emph{words}.\n\n\\begin{equation}\n  x \\in \\R\n\\end{equation}\n\\end{document}\n";

    #[test]
    fn the_file_is_cut_into_blocks_each_drawn_as_it_can_be() {
        let (session, _root) = session_on("visual-cut", DOC);
        let mut v = Visual::default();
        v.refresh(&session);
        let kinds: Vec<&str> = v
            .drawn
            .iter()
            .map(|d| match d {
                Drawn::Nothing => "nothing",
                Drawn::Preamble => "preamble",
                Drawn::Heading { .. } => "heading",
                Drawn::Paragraph(_) => "paragraph",
                Drawn::Math => "math",
                Drawn::List { .. } => "list",
                Drawn::Figure { .. } => "figure",
                Drawn::Raw => "raw",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "preamble",
                "heading",
                "paragraph",
                "nothing",
                "math",
                "nothing"
            ]
        );
        assert_eq!(v.macros, [("\\R".to_string(), "\\mathbb{R}".to_string())]);
    }

    #[test]
    fn typing_is_written_into_its_block_once_it_rests_and_undone_in_one_step() {
        let (mut session, _root) = session_on("visual-typing", DOC);
        let mut v = Visual::default();
        v.refresh(&session);
        let i = 2;
        let pieces = v.drawn[i].pieces().unwrap().to_vec();
        let Typed::Changed(new) = texinline::typed(&pieces, "Some more words. ", 10) else {
            panic!()
        };
        v.typing = Some(Typing {
            block: i,
            pieces: new,
            since: Some(Instant::now()),
        });
        v.tend(&mut session);
        assert!(v.pending(), "not yet: typing has not rested");
        v.typing.as_mut().unwrap().since = Some(Instant::now() - REST);
        v.tend(&mut session);
        assert!(!v.pending());
        assert!(
            session
                .editor()
                .buffer()
                .contents()
                .contains("Some more \\emph{words}.\n")
        );
        assert_eq!(v.blocks[i].text, "Some more \\emph{words}.\n", "cut again");
        assert_eq!(
            v.typing.as_ref().map(|t| t.block),
            Some(i),
            "still typed in"
        );
        v.undo(&mut session, false);
        assert_eq!(session.editor().buffer().contents(), DOC);
        v.undo(&mut session, true);
        assert!(session.editor().buffer().contents().contains("Some more"));
    }

    #[test]
    fn a_caret_in_a_heading_is_a_place_in_the_source() {
        let (session, _root) = session_on("visual-caret", DOC);
        let mut v = Visual::default();
        v.refresh(&session);
        let pieces = v.drawn[1].pieces().unwrap().to_vec();
        v.typing = Some(Typing {
            block: 1,
            pieces,
            since: None,
        });
        let at = v.source_of(1, 2).unwrap();
        let text = session.editor().buffer().contents();
        assert_eq!(text.chars().skip(at).take(3).collect::<String>(), "tro");
    }

    #[test]
    fn a_figure_s_picture_is_found_beside_the_file_or_in_the_project_with_or_without_its_extension()
    {
        let root = crate::tex::tests::project("pictures");
        std::fs::create_dir_all(root.join("chapters/img")).unwrap();
        std::fs::write(root.join("chapters/img/a.png"), b"x").unwrap();
        std::fs::write(root.join("figures/b.pdf"), b"x").unwrap();
        let here = root.join("chapters");
        assert_eq!(
            picture_file(&here, &root, "img/a.png"),
            Some(here.join("img/a.png"))
        );
        assert_eq!(
            picture_file(&here, &root, "img/a"),
            Some(here.join("img/a.png"))
        );
        assert_eq!(
            picture_file(&here, &root, "figures/b"),
            Some(root.join("figures/b.pdf"))
        );
        assert_eq!(picture_file(&here, &root, "none"), None);
    }
}
