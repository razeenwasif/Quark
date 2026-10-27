//! A keystroke, as it arrives from the shell.
//!
//! This mirrors the browser's `KeyboardEvent` closely enough that the shell can
//! forward one without interpretation — deciding what a key *means* is the
//! core's job, not the front end's.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    /// The printable value, or a named key: `Escape`, `Enter`, `Tab`,
    /// `Backspace`, `ArrowLeft`…
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub meta: bool,
}

impl Key {
    pub fn char(c: char) -> Self {
        Self {
            key: c.to_string(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        }
    }

    pub fn named(name: &str) -> Self {
        Self {
            key: name.to_string(),
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        }
    }

    pub fn ctrl(c: char) -> Self {
        Self {
            ctrl: true,
            ..Self::char(c)
        }
    }

    /// The single character this key produces, if it produces one. Named keys
    /// like `Escape` are multi-character and yield `None`, which is what keeps
    /// `Escape` from ever being mistaken for `E`.
    pub fn as_char(&self) -> Option<char> {
        let mut chars = self.key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    }

    /// A bare character with no modifier that changes its meaning. Shift is
    /// allowed through because it is already reflected in the character itself
    /// (`shift`+`a` arrives as `A`).
    pub fn is_bare_char(&self) -> bool {
        !self.ctrl && !self.alt && !self.meta && self.as_char().is_some()
    }

    /// Vim notation, for the statusline's pending-command display and for
    /// recorded macros.
    pub fn notation(&self) -> String {
        let base = match self.key.as_str() {
            " " => "Space".to_string(),
            other => other.to_string(),
        };
        if self.ctrl {
            format!("<C-{base}>")
        } else if self.alt {
            format!("<M-{base}>")
        } else if base.chars().count() > 1 {
            format!("<{base}>")
        } else {
            base
        }
    }
}
