//! The modal input engine.
//!
//! This is an application service, not an editor feature: it lives in the core,
//! it knows nothing about buffers or pixels, and its only output is a resolved
//! [`Action`] plus mode transitions. Every workspace mode consumes the same
//! stream.
//!
//! M0 implements counts, registers, operator-pending state, the doubled-operator
//! forms (`dd`, `yy`, `cc`), the common motions, and the leader. Text objects
//! (`ciw`), marks, and macros land in M1 — the state machine is shaped to take
//! them without restructuring.

use serde::{Deserialize, Serialize};

use crate::action::{Action, Object, Verb};
use crate::key::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
    /// A rectangle rather than a run. The one selection whose shape a text
    /// editor cannot express as a range, which is why it is its own mode and
    /// not a flag on the others.
    VisualBlock,
    Replace,
    Command,
}

impl VimMode {
    /// What the statusline shows.
    pub fn label(self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => "VISUAL",
            VimMode::VisualLine => "V-LINE",
            VimMode::VisualBlock => "V-BLOCK",
            VimMode::Replace => "REPLACE",
            VimMode::Command => "COMMAND",
        }
    }
}

/// What a keystroke produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A complete action, ready for the active mode to execute.
    Action(Action),
    /// The key was absorbed into a pending command (`2`, `d`, `"a`). The string
    /// is what `showcmd` displays in the corner.
    Pending(String),
    /// The leader was pressed and which-key should open, or a subsequent key
    /// narrowed it. The string is the prefix so far.
    Leader(String),
    /// The command line changed. `None` means it closed without executing.
    Cmdline(Option<String>),
    /// The command line was submitted. Distinct from `Cmdline` because a
    /// consumer has to be able to tell a keystroke from an execution — they
    /// carry the same text and mean entirely different things.
    Execute(String),
    /// A search was submitted — `/pattern` or `?pattern`.
    ///
    /// Separate from `Execute` because the two are typed the same way and mean
    /// entirely different things, which is the same reason `Execute` is
    /// separate from `Cmdline`.
    Search { pattern: String, forward: bool },
    /// Move to the next or previous match of the last search — `n` and `N`.
    Repeat { forward: bool },
    /// Search for the word under the cursor — `*` and `#`.
    SearchWord { forward: bool },
    /// A literal character typed in insert mode.
    Input(char),
    /// Tab in insert mode: one level of indentation, whatever that is in the
    /// file being edited — which the workspace knows and the engine does not.
    Indent,
    /// `gv`: the last selection again. The workspace kept it, and says which
    /// kind it was with [`Vim::reselect`].
    Reselect,
    /// A screen's worth of lines down or up — `⌃d` and `⌃u` half of one,
    /// `⌃f`, `⌃b` and the page keys all of it. How many lines that is only
    /// the workspace knows, because only it has been told how tall the view
    /// is.
    Page { down: bool, half: bool, count: u32 },
    /// Nothing happened. Deliberately distinct from an action with no effect.
    Ignored,
}

#[derive(Debug, Default)]
struct Pending {
    count: Option<u32>,
    operator: Option<Verb>,
    /// The count typed *after* an operator: the 3 in `d3w`.
    operator_count: Option<u32>,
    register: Option<String>,
    /// Set after `"` while waiting for the register name.
    awaiting_register: bool,
    /// Set after `i` or `a` in operator-pending state, while waiting for the
    /// object key. `true` is `a` (around), `false` is `i` (inner).
    text_object: Option<bool>,
    /// Set after `g`, which is a prefix rather than a command: `gg`, `gU`, `gq`.
    awaiting_g: bool,
    /// Set after `m`, ``` or `'`, while waiting for the mark's name. The next
    /// key is taken literally whatever else is pending, which is what lets
    /// `d'a` work.
    awaiting_mark: Option<MarkUse>,
    /// Set after `f`, `F`, `t` or `T`: which way, and whether to stop short.
    /// The next key is the character to find, taken literally.
    awaiting_find: Option<(bool, bool)>,
    /// Set after `r`, while waiting for the character to write.
    awaiting_replace: bool,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.count.is_none()
            && self.operator.is_none()
            && self.operator_count.is_none()
            && self.register.is_none()
            && !self.awaiting_register
            && self.text_object.is_none()
            && !self.awaiting_g
            && self.awaiting_mark.is_none()
            && self.awaiting_find.is_none()
            && !self.awaiting_replace
    }

    /// vim multiplies the two counts: `2d3w` deletes six words.
    fn total_count(&self) -> u32 {
        self.count
            .unwrap_or(1)
            .saturating_mul(self.operator_count.unwrap_or(1))
            .max(1)
    }

    fn clear(&mut self) {
        *self = Pending::default();
    }

    fn showcmd(&self) -> String {
        let mut s = String::new();
        if let Some(r) = &self.register {
            s.push('"');
            s.push_str(r);
        } else if self.awaiting_register {
            s.push('"');
        }
        if let Some(c) = self.count {
            s.push_str(&c.to_string());
        }
        if let Some(op) = self.operator {
            s.push_str(match op {
                Verb::Delete => "d",
                Verb::Yank => "y",
                Verb::Change => "c",
                Verb::Indent => ">",
                Verb::Outdent => "<",
                Verb::Lowercase => "gu",
                Verb::Uppercase => "gU",
                Verb::ToggleCase => "g~",
                _ => "?",
            });
        }
        if let Some(c) = self.operator_count {
            s.push_str(&c.to_string());
        }
        if let Some(around) = self.text_object {
            s.push(if around { 'a' } else { 'i' });
        }
        if self.awaiting_g {
            s.push('g');
        }
        if let Some(use_of) = self.awaiting_mark {
            s.push(match use_of {
                MarkUse::Set => 'm',
                MarkUse::Exact => '`',
                MarkUse::Line => '\x27',
            });
        }
        if let Some((forward, till)) = self.awaiting_find {
            s.push(match (forward, till) {
                (true, false) => 'f',
                (false, false) => 'F',
                (true, true) => 't',
                (false, true) => 'T',
            });
        }
        if self.awaiting_replace {
            s.push('r');
        }
        s
    }
}

/// One replayable step. Macros record these rather than keystrokes, which is
/// what lets a macro recorded in Codex replay in Video: an `Action` is
/// mode-agnostic, a keystroke is not.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Act(Action),
    Type(char),
    Escape,
}

#[derive(Debug, Default)]
pub struct Vim {
    mode: VimMode,
    pending: Pending,
    cmdline: String,
    /// Which line is open: ':' for an ex command, '/' or '?' for a search.
    ///
    /// They share the line at the bottom because they are the same
    /// interaction, and differ in exactly one thing — what pressing Enter
    /// means. Keeping the character is cheaper than a second mode that would
    /// duplicate every key in it.
    prompt: char,
    /// The mode the command line was opened from, so a search can give it
    /// back. See [`Vim::after_prompt`].
    resume: VimMode,
    /// Keys typed after the leader, before which-key resolves.
    leader: Option<String>,
    /// Set after `⌃w`, while waiting for the key that names what to do with
    /// the splits. Its own field rather than part of `Pending`, because it
    /// swallows the next key whatever else is half-typed.
    awaiting_window: bool,

    /// The last change, for `.`. A change is the action *plus* whatever was
    /// typed in the insert mode it opened — repeating `ciwfoo` has to retype
    /// `foo`, which is why this is not just an `Action`.
    last_change: Option<Vec<Step>>,
    /// The change being accumulated right now.
    recording_change: Option<Vec<Step>>,

    /// `q{reg}` … `q`
    macro_register: Option<char>,
    macro_steps: Vec<Step>,
    macros: std::collections::HashMap<char, Vec<Step>>,
    /// Set after `q` or `@` while waiting for the register name.
    awaiting_macro: Option<MacroWait>,
    /// The last `f`, `t`, `F` or `T`, for `;` and `,` to repeat.
    last_find: Option<(char, bool, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacroWait {
    Record,
    Replay,
}

impl Vim {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> VimMode {
        self.mode
    }

    pub fn cmdline(&self) -> &str {
        &self.cmdline
    }

    /// Whether a selection is being held.
    ///
    /// True in any visual mode, and also while a *search* line is open from
    /// one — the mode is `Command` for the length of typing it, but the
    /// selection has not gone anywhere and `v/foo` has to keep extending it.
    pub fn is_selecting(&self) -> bool {
        let visual = |mode| {
            matches!(
                mode,
                VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
            )
        };
        visual(self.mode)
            || (self.mode == VimMode::Command
                && matches!(self.prompt, '/' | '?')
                && visual(self.resume))
    }

    /// Which line is open — `:`, `/` or `?`. What the prompt draws.
    pub fn prompt(&self) -> char {
        if self.prompt == '\0' {
            ':'
        } else {
            self.prompt
        }
    }

    /// Back into the visual mode a selection was made in, for `gv`.
    pub fn reselect(&mut self, mode: VimMode) {
        if matches!(
            mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        ) {
            self.pending.clear();
            self.mode = mode;
        }
    }

    /// Begin a charwise visual selection — what a mouse drag is.
    ///
    /// A selection already running is left alone, so dragging inside a
    /// linewise or blockwise one extends it in the shape you chose rather than
    /// silently downgrading it to charwise.
    pub fn enter_visual(&mut self) {
        if matches!(
            self.mode,
            VimMode::Normal | VimMode::Insert | VimMode::Replace
        ) {
            self.mode = VimMode::Visual;
        }
    }

    /// Open the command line already holding something.
    ///
    /// For `␣lr`, which is `:rename <the name it has now>` — a command you
    /// edit rather than one you type from nothing. Reusing the `:` line means
    /// a rename needs no prompt of its own, and the name arrives through the
    /// same ex command anyone can type by hand.
    pub fn open_cmdline(&mut self, text: String) -> String {
        self.pending.clear();
        self.leader = None;
        self.resume = self.mode;
        self.mode = VimMode::Command;
        self.prompt = ':';
        self.cmdline = text;
        self.cmdline.clone()
    }

    /// Drop a leader sequence in progress, without the mode change Escape
    /// would also do. Used when something above the workspace claims the
    /// prefix — the sequence is over, but nothing about the mode changed.
    pub fn cancel_leader(&mut self) {
        self.leader = None;
        self.pending.clear();
        self.awaiting_window = false;
    }

    pub fn leader_prefix(&self) -> Option<&str> {
        self.leader.as_deref()
    }

    /// Whether a macro is being recorded, and into which register.
    pub fn recording(&self) -> Option<char> {
        self.macro_register
    }

    /// Feed one keystroke. Returns everything it produced — a list, because
    /// `.` and `@q` expand into several actions from a single key.
    pub fn keys(&mut self, k: &Key) -> Vec<Outcome> {
        // `.` and `@` are the only keys that fan out, and they are handled
        // before the state machine so a replayed step cannot re-trigger them.
        if self.mode == VimMode::Normal && self.leader.is_none() && self.pending.is_empty() {
            if let Some(c) = k.as_char().filter(|_| k.is_bare_char()) {
                if let Some(wait) = self.awaiting_macro {
                    self.awaiting_macro = None;
                    return match wait {
                        MacroWait::Record => {
                            self.macro_register = Some(c);
                            self.macro_steps.clear();
                            vec![Outcome::Pending(format!("recording @{c}"))]
                        }
                        MacroWait::Replay => {
                            let steps = self.macros.get(&c).cloned().unwrap_or_default();
                            self.replay(steps)
                        }
                    };
                }
                if c == 'q' {
                    if let Some(reg) = self.macro_register.take() {
                        let steps = std::mem::take(&mut self.macro_steps);
                        self.macros.insert(reg, steps);
                        return vec![Outcome::Pending(String::new())];
                    }
                    self.awaiting_macro = Some(MacroWait::Record);
                    return vec![Outcome::Pending("q".into())];
                }
                if c == '@' {
                    self.awaiting_macro = Some(MacroWait::Replay);
                    return vec![Outcome::Pending("@".into())];
                }
                if c == '.' {
                    let steps = self.last_change.clone().unwrap_or_default();
                    return self.replay(steps);
                }
            }
        }

        let outcome = self.key(k);
        self.record(&outcome, k);
        vec![outcome]
    }

    /// Replay recorded steps straight into outcomes, without going back
    /// through the keyboard. A step is already resolved, so this cannot
    /// re-enter `.` or `@` and loop forever.
    fn replay(&mut self, steps: Vec<Step>) -> Vec<Outcome> {
        let mut out = Vec::with_capacity(steps.len());
        for step in &steps {
            match step {
                Step::Act(action) => {
                    if action.verb == Verb::Change || action.verb == Verb::Insert {
                        self.mode = VimMode::Insert;
                    }
                    out.push(Outcome::Action(action.clone()));
                }
                Step::Type(c) => out.push(Outcome::Input(*c)),
                Step::Escape => {
                    self.mode = VimMode::Normal;
                    out.push(Outcome::Ignored);
                }
            }
        }
        // A replayed change is itself repeatable.
        if !steps.is_empty() {
            self.last_change = Some(steps);
        }
        out
    }

    /// Fold an outcome into the macro and `.` recorders.
    fn record(&mut self, outcome: &Outcome, k: &Key) {
        let step = match outcome {
            Outcome::Action(a) => Some(Step::Act(a.clone())),
            Outcome::Input(c) => Some(Step::Type(*c)),
            _ if k.key == "Escape" => Some(Step::Escape),
            _ => None,
        };
        let Some(step) = step else { return };

        if self.macro_register.is_some() {
            self.macro_steps.push(step.clone());
        }

        // A change starts at the action that modified the buffer and ends when
        // insert mode does, so `ciwfoo<Esc>` is one repeatable unit.
        match &step {
            Step::Act(a) if a.verb.modifies() => {
                self.recording_change = Some(vec![step.clone()]);
                if self.mode != VimMode::Insert {
                    self.last_change = self.recording_change.take();
                }
            }
            Step::Type(_) => {
                if let Some(change) = &mut self.recording_change {
                    change.push(step.clone());
                }
            }
            Step::Escape => {
                if let Some(mut change) = self.recording_change.take() {
                    // The Escape is part of the change. Without it a replayed
                    // `.` leaves insert mode open and the next keys are typed
                    // into the buffer instead of being commands.
                    change.push(Step::Escape);
                    self.last_change = Some(change);
                }
            }
            _ => {}
        }
    }

    /// Feed one keystroke. Returns what it produced and leaves the engine in
    /// whatever state that implies.
    pub fn key(&mut self, k: &Key) -> Outcome {
        // Escape always unwinds one level, from any mode. This is checked first
        // so no other branch can swallow it.
        if k.key == "Escape" {
            return self.escape();
        }

        if k.ctrl && k.key.eq_ignore_ascii_case("r") && self.mode == VimMode::Normal {
            let count = self.pending.count.unwrap_or(1);
            self.pending.clear();
            return Outcome::Action(Action::new(Verb::Redo, Object::None, count));
        }

        // `⌃w` is vim's window prefix: the next key names what to do with the
        // splits. Handled here rather than in `key_normal` because it is a
        // chord, and held in its own field rather than in `Pending` because it
        // swallows the next key whatever else is half-typed — exactly as
        // vim's does.
        if self.awaiting_window {
            self.awaiting_window = false;
            return Outcome::Leader(format!("w{}", k.key));
        }
        // Not in insert mode, where `⌃w` deletes the word before the cursor —
        // as it does in vim, and in every terminal.
        if k.ctrl
            && k.key.eq_ignore_ascii_case("w")
            && !matches!(self.mode, VimMode::Command | VimMode::Insert | VimMode::Replace)
        {
            self.awaiting_window = true;
            return Outcome::Pending("^w".into());
        }

        // `⌃v` is the one modal command that is a chord rather than a letter,
        // so it is matched here — before the branch that only looks at bare
        // characters and would drop it.
        if k.ctrl
            && k.key.eq_ignore_ascii_case("v")
            && !matches!(self.mode, VimMode::Command | VimMode::Insert | VimMode::Replace)
        {
            self.pending.clear();
            self.mode = if self.mode == VimMode::VisualBlock {
                VimMode::Normal
            } else {
                VimMode::VisualBlock
            };
            return Outcome::Ignored;
        }

        match self.mode {
            VimMode::Insert | VimMode::Replace => self.key_insert(k),
            VimMode::Command => self.key_cmdline(k),
            VimMode::Normal | VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock => {
                self.key_normal(k)
            }
        }
    }

    fn escape(&mut self) -> Outcome {
        // In normal mode with something pending, Escape cancels the pending
        // command rather than doing nothing.
        if self.mode == VimMode::Normal && (!self.pending.is_empty() || self.leader.is_some()) {
            self.pending.clear();
            self.leader = None;
            return Outcome::Pending(String::new());
        }
        let was_cmdline = self.mode == VimMode::Command;
        self.pending.clear();
        self.leader = None;
        self.cmdline.clear();
        // Abandoning a search goes back to the selection you were holding when
        // you started typing it. Escape out of `v/foo` should leave you where
        // Escape out of `v` would not have.
        self.mode = if was_cmdline {
            self.after_prompt()
        } else {
            VimMode::Normal
        };
        if was_cmdline {
            Outcome::Cmdline(None)
        } else {
            Outcome::Ignored
        }
    }

    /// Which mode closing the command line returns to.
    ///
    /// A search is a motion, so a selection survives it: `v/foo` extends to
    /// the match, exactly as `v}` extends to the paragraph. An ex command is
    /// not, and leaves you in normal mode the way vim does.
    fn after_prompt(&self) -> VimMode {
        let visual = matches!(
            self.resume,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        );
        if visual && matches!(self.prompt, '/' | '?') {
            self.resume
        } else {
            VimMode::Normal
        }
    }

    fn key_insert(&mut self, k: &Key) -> Outcome {
        // The arrows and Home/End work in insert mode. Vim's do, and a modal
        // editor that traps you at one position until you press Escape is not
        // being stricter than vim — it is being broken.
        let object = match k.key.as_str() {
            "ArrowLeft" => Some(Object::CharLeft),
            "ArrowRight" => Some(Object::CharRight),
            "ArrowUp" => Some(Object::LineUp),
            "ArrowDown" => Some(Object::LineDown),
            "Home" => Some(Object::LineStart),
            "End" => Some(Object::LineEnd),
            _ => None,
        };
        if let Some(object) = object {
            return Outcome::Action(Action {
                verb: Verb::Move,
                object,
                count: 1,
                register: None,
            });
        }
        if k.key == "Tab" && !k.ctrl && !k.alt && !k.meta && !k.shift {
            return Outcome::Indent;
        }
        // The line-editing keys every terminal has: `⌃w` takes back the word
        // before the cursor, `⌃u` the line before it, `⌃h` one character.
        if k.ctrl && !k.alt && !k.meta {
            let object = match k.key.as_str() {
                "w" => Some(Object::WordBackward),
                "u" => Some(Object::LineStart),
                "h" => Some(Object::CharLeft),
                _ => None,
            };
            if let Some(object) = object {
                return Outcome::Action(Action::new(Verb::Delete, object, 1));
            }
        }

        match k.as_char() {
            Some(c) if k.is_bare_char() => Outcome::Input(c),
            _ => Outcome::Ignored,
        }
    }

    fn key_cmdline(&mut self, k: &Key) -> Outcome {
        match k.key.as_str() {
            "Enter" => {
                let line = std::mem::take(&mut self.cmdline);
                self.mode = self.after_prompt();
                match self.prompt {
                    '/' => Outcome::Search {
                        pattern: line,
                        forward: true,
                    },
                    '?' => Outcome::Search {
                        pattern: line,
                        forward: false,
                    },
                    _ => Outcome::Execute(line),
                }
            }
            "Backspace" => {
                if self.cmdline.pop().is_none() {
                    self.mode = VimMode::Normal;
                    return Outcome::Cmdline(None);
                }
                Outcome::Cmdline(Some(self.cmdline.clone()))
            }
            _ => match k.as_char() {
                Some(c) if k.is_bare_char() => {
                    self.cmdline.push(c);
                    Outcome::Cmdline(Some(self.cmdline.clone()))
                }
                _ => Outcome::Ignored,
            },
        }
    }

    fn key_normal(&mut self, k: &Key) -> Outcome {
        // The arrows, Home and End are `hjkl`, `0` and `$` here as they are in
        // insert mode: the way to grow a selection without leaving them, and
        // what `d↓` means. Not while a key is being taken literally — `f→` is
        // not a search for `l`.
        let literal = self.leader.is_some()
            || self.pending.awaiting_mark.is_some()
            || self.pending.awaiting_find.is_some()
            || self.pending.awaiting_replace
            || self.pending.awaiting_register;
        let arrow = match k.key.as_str() {
            "ArrowLeft" => Some('h'),
            "ArrowRight" => Some('l'),
            "ArrowUp" => Some('k'),
            "ArrowDown" => Some('j'),
            "Home" => Some('0'),
            "End" => Some('$'),
            _ => None,
        }
        .filter(|_| !literal && !k.ctrl && !k.alt && !k.meta);
        let Some(c) = arrow.or_else(|| k.as_char().filter(|_| k.is_bare_char())) else {
            return self.key_page(k);
        };

        // Leader sequence in progress: collect until a mode or plugin claims it.
        if let Some(prefix) = &mut self.leader {
            prefix.push(c);
            return Outcome::Leader(prefix.clone());
        }

        // A mark's name is whatever key comes next, taken literally — before
        // digits, before the leader, before an operator gets a look at it.
        // `m1` names a mark `1`, not a count of one.
        if let Some(use_of) = self.pending.awaiting_mark.take() {
            return self.mark_named(use_of, c);
        }
        // The character after `f`, `t`, `F` or `T` is the one to look for,
        // taken literally like a mark's name.
        if let Some((forward, till)) = self.pending.awaiting_find.take() {
            self.last_find = Some((c, forward, till));
            return self.motion_outcome(Object::FindChar { ch: c, forward, till });
        }
        // And the one after `r` is what to write.
        if self.pending.awaiting_replace {
            let count = self.pending.total_count();
            let register = self.pending.register.clone();
            self.pending.clear();
            self.mode = VimMode::Normal;
            let mut action = Action::new(Verb::Replace, Object::Char { ch: c }, count);
            action.register = register;
            return Outcome::Action(action);
        }

        // `"a` selects a register for the next operator.
        if self.pending.awaiting_register {
            self.pending.awaiting_register = false;
            self.pending.register = Some(c.to_string());
            return Outcome::Pending(self.pending.showcmd());
        }
        // Not after `i` or `a`, where `"` is the quote text object — `di"`.
        if c == '"' && self.pending.text_object.is_none() {
            self.pending.awaiting_register = true;
            return Outcome::Pending(self.pending.showcmd());
        }

        // Digits build a count. A leading `0` is the line-start motion, not a
        // count — that ordering is why this branch tests for an existing count.
        if c.is_ascii_digit() && !(c == '0' && self.count_slot().is_none()) {
            let digit = c.to_digit(10).unwrap();
            let slot = self.count_slot_mut();
            *slot = Some(slot.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            return Outcome::Pending(self.pending.showcmd());
        }

        // Leader is Space, and only in normal mode with nothing pending.
        if c == ' ' && self.mode == VimMode::Normal && self.pending.is_empty() {
            self.leader = Some(String::new());
            return Outcome::Leader(String::new());
        }

        // `m` names a mark to set; `` ` `` and `'` name one to go to. All
        // three claim the next key, in normal mode and operator-pending
        // alike — which is what makes `d'a` mean "delete to that mark's line".
        // `m` is normal-mode only, because `dm` is not a command.
        // Again not after `i` or `a`: `ci'` and ``da` `` are quotes.
        if let Some(use_of) = MarkUse::prefix(c, self.pending.operator.is_some())
            .filter(|_| self.pending.text_object.is_none())
        {
            self.pending.awaiting_mark = Some(use_of);
            return Outcome::Pending(self.pending.showcmd());
        }

        // `f`, `t`, `F` and `T` wait for a character, in normal mode, visual
        // mode and after an operator alike — `dt)` is as common as `t)`. Not
        // inside a text object, where `t` would be a tag.
        let find = match c {
            'f' => Some((true, false)),
            'F' => Some((false, false)),
            't' => Some((true, true)),
            'T' => Some((false, true)),
            _ => None,
        };
        if let Some(find) = find.filter(|_| self.pending.text_object.is_none() && !self.pending.awaiting_g) {
            self.pending.awaiting_find = Some(find);
            return Outcome::Pending(self.pending.showcmd());
        }
        // `;` repeats the last of them, and `,` repeats it the other way.
        if (c == ';' || c == ',') && self.pending.text_object.is_none() && !self.pending.awaiting_g {
            if let Some((ch, forward, till)) = self.last_find {
                let forward = if c == ',' { !forward } else { forward };
                return self.motion_outcome(Object::FindChar { ch, forward, till });
            }
            self.pending.clear();
            return Outcome::Ignored;
        }
        // `r` waits for the character to write — and `r` on a selection
        // writes it over every character of it.
        if c == 'r' && self.pending.operator.is_none() && !self.pending.awaiting_g {
            self.pending.awaiting_replace = true;
            return Outcome::Pending(self.pending.showcmd());
        }

        // An operator already waiting: this key must be a motion, a doubled
        // operator, a text object, or nothing.
        if let Some(op) = self.pending.operator {
            // `iw` / `a(` — `i` and `a` only mean "text object" here. In normal
            // mode they are insert commands, which is why this branch sits
            // inside the operator-pending check rather than beside it.
            if let Some(around) = self.pending.text_object {
                let Some(object) = Self::text_object(c, around) else {
                    self.pending.clear();
                    return Outcome::Pending(String::new());
                };
                return Outcome::Action(self.finish_operator(op, object));
            }
            if c == 'i' || c == 'a' {
                self.pending.text_object = Some(c == 'a');
                return Outcome::Pending(self.pending.showcmd());
            }
            if Self::doubles(op, c) {
                return Outcome::Action(self.finish_operator(op, Object::Line));
            }
            if let Some(object) = self.motion_counted(c) {
                return Outcome::Action(self.finish_operator(op, object));
            }
            // Not a motion — cancel, as vim does.
            self.pending.clear();
            return Outcome::Pending(String::new());
        }

        // Visual mode: an operator applies to the selection immediately.
        let visual = matches!(
            self.mode,
            VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
        );
        if visual {
            // `x` is `d` on a selection. In normal mode it is a character
            // delete, which is why it is folded in here rather than added to
            // the operator table.
            // So are the case keys and `J`, which on a selection need no
            // motion — `U` uppercases it, `J` joins its lines.
            let on_selection = match c {
                'x' => Some(Verb::Delete),
                'u' => Some(Verb::Lowercase),
                'U' => Some(Verb::Uppercase),
                '~' => Some(Verb::ToggleCase),
                'J' => Some(Verb::Join),
                _ => None,
            };
            if let Some(verb) = Self::operator(c).or(on_selection) {
                let count = self.pending.total_count();
                let register = self.pending.register.clone();
                self.pending.clear();
                self.mode = if verb == Verb::Change {
                    VimMode::Insert
                } else {
                    VimMode::Normal
                };
                let mut action = Action::new(verb, Object::Selection, count);
                action.register = register;
                return Outcome::Action(action);
            }
        }

        self.normal_command(c)
    }

    /// The page keys, and vim's chords for them.
    fn key_page(&mut self, k: &Key) -> Outcome {
        let chord = |c: &str| k.ctrl && !k.alt && !k.meta && k.key.eq_ignore_ascii_case(c);
        let page = match k.key.as_str() {
            "PageDown" if !k.ctrl && !k.meta => Some((true, false)),
            "PageUp" if !k.ctrl && !k.meta => Some((false, false)),
            _ if chord("f") => Some((true, false)),
            _ if chord("b") => Some((false, false)),
            _ if chord("d") => Some((true, true)),
            _ if chord("u") => Some((false, true)),
            _ => None,
        };
        match page {
            Some((down, half)) if self.pending.operator.is_none() => {
                let count = self.pending.total_count();
                self.pending.clear();
                Outcome::Page { down, half, count }
            }
            _ => Outcome::Ignored,
        }
    }

    fn normal_command(&mut self, c: char) -> Outcome {
        let count = self.pending.total_count();

        // `g` is a prefix, not a command. `gg`, `gd` and `gr` are bound; the
        // rest of the family (`gU`, `gq`) hangs off the same branch.
        if self.pending.awaiting_g {
            let counted = self.pending.count;
            let visual = matches!(
                self.mode,
                VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
            );
            // `gu`, `gU` and `g~` are operators: on a selection they act at
            // once, and otherwise they wait for a motion, as `d` does.
            let case = match c {
                'u' => Some(Verb::Lowercase),
                'U' => Some(Verb::Uppercase),
                '~' => Some(Verb::ToggleCase),
                _ => None,
            };
            if let Some(verb) = case {
                self.pending.awaiting_g = false;
                if visual {
                    self.pending.clear();
                    self.mode = VimMode::Normal;
                    return Outcome::Action(Action::new(verb, Object::Selection, count));
                }
                self.pending.operator = Some(verb);
                return Outcome::Pending(self.pending.showcmd());
            }
            self.pending.clear();
            return match c {
                // `5gg` is line five, as `5G` is.
                'g' => match counted {
                    Some(line) => self.motion_outcome(Object::LineNumber { line }),
                    None => Outcome::Action(Action::new(Verb::Move, Object::FileStart, count)),
                },
                'd' => Outcome::Action(Action::new(Verb::Goto, Object::Definition, 1)),
                'r' => Outcome::Action(Action::new(Verb::Goto, Object::References, 1)),
                'v' => Outcome::Reselect,
                _ => Outcome::Ignored,
            };
        }
        if c == 'g' {
            self.pending.awaiting_g = true;
            return Outcome::Pending(self.pending.showcmd());
        }

        // In a block, `I` and `A` insert on *every* line of it at once — the
        // reason the mode exists. Everywhere else they are ordinary insert
        // commands, which is why this sits ahead of the mode entries rather
        // than as another arm of them.
        if self.mode == VimMode::VisualBlock && (c == 'I' || c == 'A') {
            self.pending.clear();
            self.mode = VimMode::Insert;
            let edge = if c == 'I' {
                Object::BlockStart
            } else {
                Object::BlockEnd
            };
            return Outcome::Action(Action::new(Verb::Insert, edge, 1));
        }

        // Mode entries.
        let enter = |vim: &mut Vim, mode: VimMode, verb: Verb, object: Object| {
            vim.pending.clear();
            vim.mode = mode;
            Outcome::Action(Action::new(verb, object, 1))
        };

        match c {
            'i' => return enter(self, VimMode::Insert, Verb::Insert, Object::Here),
            'a' => return enter(self, VimMode::Insert, Verb::Insert, Object::After),
            'I' => return enter(self, VimMode::Insert, Verb::Insert, Object::LineStart),
            'A' => return enter(self, VimMode::Insert, Verb::Insert, Object::LineEnd),
            'o' => return enter(self, VimMode::Insert, Verb::Insert, Object::LineBelow),
            'O' => return enter(self, VimMode::Insert, Verb::Insert, Object::LineAbove),
            'R' => {
                self.pending.clear();
                self.mode = VimMode::Replace;
                return Outcome::Ignored;
            }
            'v' => {
                self.pending.clear();
                self.mode = if self.mode == VimMode::Visual {
                    VimMode::Normal
                } else {
                    VimMode::Visual
                };
                return Outcome::Ignored;
            }
            'V' => {
                self.pending.clear();
                self.mode = if self.mode == VimMode::VisualLine {
                    VimMode::Normal
                } else {
                    VimMode::VisualLine
                };
                return Outcome::Ignored;
            }
            ':' | '/' | '?' => {
                self.pending.clear();
                self.resume = self.mode;
                self.mode = VimMode::Command;
                self.cmdline.clear();
                self.prompt = c;
                return Outcome::Cmdline(Some(String::new()));
            }
            // `n` and `N` repeat the last search; the pattern is the
            // document's, not the modal engine's, so the workspace resolves it.
            'n' => return Outcome::Repeat { forward: true },
            'N' => return Outcome::Repeat { forward: false },
            // `*` and `#` search for the word under the cursor.
            '*' => return Outcome::SearchWord { forward: true },
            '#' => return Outcome::SearchWord { forward: false },
            _ => {}
        }

        // Operators begin an operator-pending state.
        if let Some(verb) = Self::operator(c) {
            self.pending.operator = Some(verb);
            return Outcome::Pending(self.pending.showcmd());
        }

        // Standalone verbs.
        let standalone = match c {
            'u' => Some(Action::new(Verb::Undo, Object::None, count)),
            'p' => Some(Action::new(Verb::Paste, Object::After, count)),
            'P' => Some(Action::new(Verb::Paste, Object::Here, count)),
            'x' => Some(Action::new(Verb::Delete, Object::CharRight, count)),
            'X' => Some(Action::new(Verb::Delete, Object::CharLeft, count)),
            'D' => Some(Action::new(Verb::Delete, Object::LineEnd, count)),
            'C' => Some(Action::new(Verb::Change, Object::LineEnd, count)),
            'Y' => Some(Action::new(Verb::Yank, Object::Line, count)),
            // `s` is `cl` and `S` is `cc`, as vim defines them.
            's' => Some(Action::new(Verb::Change, Object::CharRight, count)),
            'S' => Some(Action::new(Verb::Change, Object::Line, count)),
            '~' => Some(Action::new(Verb::ToggleCase, Object::CharRight, count)),
            'J' => Some(Action::new(Verb::Join, Object::Line, count)),
            // `K` is vim's "look this up". Count is meaningless — there is
            // one thing under the cursor — so it is not carried.
            'K' => Some(Action::new(Verb::Explain, Object::Here, 1)),
            _ => None,
        };
        if let Some(mut action) = standalone {
            action.register = self.pending.register.clone();
            self.pending.clear();
            if action.verb == Verb::Change {
                self.mode = VimMode::Insert;
            }
            return Outcome::Action(action);
        }

        // Bare motions.
        if let Some(object) = self.motion_counted(c) {
            self.pending.clear();
            return Outcome::Action(Action::new(Verb::Move, object, count));
        }

        self.pending.clear();
        Outcome::Ignored
    }

    /// Which count a digit belongs to: before an operator it is the outer
    /// count, after it the inner one.
    fn count_slot(&self) -> Option<u32> {
        if self.pending.operator.is_some() {
            self.pending.operator_count
        } else {
            self.pending.count
        }
    }

    fn count_slot_mut(&mut self) -> &mut Option<u32> {
        if self.pending.operator.is_some() {
            &mut self.pending.operator_count
        } else {
            &mut self.pending.count
        }
    }

    /// Complete an operator with its object: take the counts and register,
    /// reset the pending state, and enter insert mode if the verb was `c`.
    fn finish_operator(&mut self, op: Verb, object: Object) -> Action {
        let count = self.pending.total_count();
        let register = self.pending.register.clone();
        self.pending.clear();
        if op == Verb::Change {
            self.mode = VimMode::Insert;
        }
        let mut action = Action::new(op, object, count);
        action.register = register;
        action
    }

    /// The key after `i` or `a` in operator-pending state.
    fn text_object(c: char, around: bool) -> Option<Object> {
        Some(match c {
            'w' if around => Object::AroundWord,
            'w' => Object::InnerWord,
            'W' if around => Object::AroundBigWord,
            'W' => Object::InnerBigWord,
            '(' | ')' | 'b' => Self::pair('(', around),
            '{' | '}' | 'B' => Self::pair('{', around),
            '[' | ']' => Self::pair('[', around),
            '<' | '>' => Self::pair('<', around),
            '"' => Self::pair('"', around),
            '\'' => Self::pair('\'', around),
            '`' => Self::pair('`', around),
            _ => return None,
        })
    }

    fn pair(open: char, around: bool) -> Object {
        if around {
            Object::AroundPair { open }
        } else {
            Object::InnerPair { open }
        }
    }

    fn operator(c: char) -> Option<Verb> {
        match c {
            'd' => Some(Verb::Delete),
            'y' => Some(Verb::Yank),
            'c' => Some(Verb::Change),
            '>' => Some(Verb::Indent),
            '<' => Some(Verb::Outdent),
            _ => None,
        }
    }

    /// `dd`, `yy`, `cc` — an operator doubled means "this whole line".
    fn doubles(op: Verb, c: char) -> bool {
        matches!(
            (op, c),
            (Verb::Delete, 'd')
                | (Verb::Yank, 'y')
                | (Verb::Change, 'c')
                | (Verb::Indent, '>')
                | (Verb::Outdent, '<')
                | (Verb::Lowercase, 'u')
                | (Verb::Uppercase, 'U')
                | (Verb::ToggleCase, '~')
        )
    }

    fn motion(c: char) -> Option<Object> {
        Some(match c {
            'h' => Object::CharLeft,
            'l' => Object::CharRight,
            'j' => Object::LineDown,
            'k' => Object::LineUp,
            'w' => Object::WordForward,
            'b' => Object::WordBackward,
            'e' => Object::WordEnd,
            'W' => Object::BigWordForward,
            'B' => Object::BigWordBackward,
            'E' => Object::BigWordEnd,
            '^' => Object::FirstNonBlank,
            '0' => Object::LineStart,
            '$' => Object::LineEnd,
            'G' => Object::FileEnd,
            '%' => Object::MatchPair,
            '}' => Object::ParagraphForward,
            '{' => Object::ParagraphBackward,
            _ => return None,
        })
    }

    /// A motion, with `G` going to the counted line when there is a count —
    /// `5G` — rather than to the end.
    fn motion_counted(&self, c: char) -> Option<Object> {
        let counted = self.pending.count.or(self.pending.operator_count);
        match (c, counted) {
            ('G', Some(_)) => Some(Object::LineNumber {
                line: self.pending.total_count(),
            }),
            _ => Self::motion(c),
        }
    }

    /// A motion's object, as an operator's target if one is waiting and as a
    /// move otherwise.
    fn motion_outcome(&mut self, object: Object) -> Outcome {
        match self.pending.operator {
            Some(op) => Outcome::Action(self.finish_operator(op, object)),
            None => {
                let count = self.pending.total_count();
                self.pending.clear();
                let count = match object {
                    Object::LineNumber { .. } => 1,
                    _ => count,
                };
                Outcome::Action(Action::new(Verb::Move, object, count))
            }
        }
    }
}

/// What the key after `m`, `` ` `` or `'` is going to be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkUse {
    Set,
    /// `` `a `` — exactly where the mark is, column and all.
    Exact,
    /// `'a` — the first non-blank of the mark's line, and linewise when an
    /// operator is waiting.
    Line,
}

impl MarkUse {
    fn prefix(c: char, operator_pending: bool) -> Option<MarkUse> {
        match c {
            // `dm` is not a command, so `m` only starts a mark when nothing
            // is waiting on a motion.
            'm' if !operator_pending => Some(MarkUse::Set),
            '`' => Some(MarkUse::Exact),
            '\'' => Some(MarkUse::Line),
            _ => None,
        }
    }
}

impl Vim {
    /// The mark's name arrived. Set it, go to it, or operate to it.
    fn mark_named(&mut self, use_of: MarkUse, name: char) -> Outcome {
        // Vim's marks are letters and a handful of punctuation. Anything else
        // is a typo, and cancelling is friendlier than storing a mark under a
        // name nobody can type again.
        if !name.is_ascii_alphanumeric() {
            self.pending.clear();
            return Outcome::Pending(String::new());
        }

        if use_of == MarkUse::Set {
            self.pending.clear();
            return Outcome::Action(Action::new(Verb::SetMark, Object::Mark { name }, 1));
        }

        let object = match use_of {
            MarkUse::Exact => Object::MarkExact { name },
            _ => Object::MarkLine { name },
        };
        match self.pending.operator {
            Some(op) => Outcome::Action(self.finish_operator(op, object)),
            None => {
                self.pending.clear();
                // No count: `3'a` is not three of anything.
                Outcome::Action(Action::new(Verb::Move, object, 1))
            }
        }
    }
}
