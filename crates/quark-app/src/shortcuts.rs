//! Keyboard accelerators.
//!
//! The bindings come from the same [`command_table`] the menus and the command
//! palette are built from, so a shortcut written next to a menu item is the
//! shortcut that actually fires. Keeping a second hand-maintained key map is
//! how those two drift apart.

use egui::{Key, Modifiers};
use quark_core::command::{Command, CommandInfo, command_table, parse_shortcut};

/// A resolved binding: modifiers plus a physical key.
#[derive(Debug, Clone)]
pub struct Binding {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
    pub command: Command,
}

/// Maps a key name from the command table onto an egui key.
pub fn key_from_name(name: &str) -> Option<Key> {
    let k = match name.to_ascii_lowercase().as_str() {
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        "0" => Key::Num0,
        "1" => Key::Num1,
        "2" => Key::Num2,
        "3" => Key::Num3,
        "4" => Key::Num4,
        "5" => Key::Num5,
        "6" => Key::Num6,
        "7" => Key::Num7,
        "8" => Key::Num8,
        "9" => Key::Num9,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f8" => Key::F8,
        "f11" => Key::F11,
        "=" | "plus" => Key::Plus,
        "-" | "minus" => Key::Minus,
        "delete" => Key::Delete,
        "backspace" => Key::Backspace,
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "home" => Key::Home,
        "end" => Key::End,
        "page up" | "pageup" => Key::PageUp,
        "page down" | "pagedown" => Key::PageDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        _ => return None,
    };
    Some(k)
}

/// Builds every binding the command table defines.
pub fn bindings() -> Vec<Binding> {
    let mut out = Vec::new();
    for CommandInfo {
        command, shortcut, ..
    } in command_table()
    {
        let Some((ctrl, shift, alt, key_name)) = parse_shortcut(shortcut) else {
            continue;
        };
        let Some(key) = key_from_name(&key_name) else {
            // A shortcut naming a key egui has no constant for would silently
            // never fire; surface it in the log rather than hiding it.
            tracing::debug!("no key mapping for shortcut {shortcut:?}");
            continue;
        };
        out.push(Binding {
            ctrl,
            shift,
            alt,
            key,
            command,
        });
    }
    out
}

/// Returns the command whose binding the current input matches.
///
/// Bindings are checked most-specific first, so `Ctrl+Shift+S` is not shadowed
/// by `Ctrl+S`. Without that ordering the more specific accelerator can never
/// fire, because its modifiers are a superset of the simpler one's.
pub fn matched(input: &egui::InputState, bindings: &[Binding]) -> Option<Command> {
    let m: Modifiers = input.modifiers;

    let mut candidates: Vec<&Binding> = bindings
        .iter()
        .filter(|b| {
            // `command` covers Cmd on macOS and Ctrl elsewhere.
            let ctrl = m.command || m.ctrl;
            b.ctrl == ctrl && b.shift == m.shift && b.alt == m.alt
        })
        .collect();
    candidates.sort_by_key(|b| {
        std::cmp::Reverse(usize::from(b.ctrl) + usize::from(b.shift) + usize::from(b.alt))
    });

    for b in candidates {
        if input.key_pressed(b.key) {
            return Some(b.command.clone());
        }
    }
    None
}

/// Single-letter tool shortcuts, which only apply when no text field has focus.
pub fn tool_shortcut(input: &egui::InputState) -> Option<quark_core::tools::Tool> {
    use quark_core::tools::Tool;
    if input.modifiers.ctrl || input.modifiers.command || input.modifiers.alt {
        return None;
    }
    if input.key_pressed(Key::V) {
        return Some(Tool::Select);
    }
    if input.key_pressed(Key::H) {
        return Some(Tool::Pan);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_core::layout::ZoomMode;

    #[test]
    fn every_shortcut_in_the_table_maps_to_a_real_key() {
        // A shortcut that cannot be resolved would appear next to a menu item
        // and then never work.
        let table = command_table();
        let with_shortcuts = table.iter().filter(|c| !c.shortcut.is_empty()).count();
        let resolved = bindings().len();
        assert_eq!(
            resolved, with_shortcuts,
            "{} shortcut(s) in the table do not resolve to a key",
            with_shortcuts - resolved
        );
    }

    #[test]
    fn common_keys_resolve() {
        assert_eq!(key_from_name("S"), Some(Key::S));
        assert_eq!(key_from_name("F3"), Some(Key::F3));
        assert_eq!(key_from_name("Page Down"), Some(Key::PageDown));
        assert_eq!(key_from_name("Left"), Some(Key::ArrowLeft));
        assert_eq!(key_from_name("Delete"), Some(Key::Delete));
        assert_eq!(key_from_name("nonsense"), None);
    }

    #[test]
    fn key_names_are_case_insensitive() {
        assert_eq!(key_from_name("s"), key_from_name("S"));
        assert_eq!(key_from_name("f11"), key_from_name("F11"));
    }

    #[test]
    fn bindings_carry_their_modifiers() {
        let b = bindings();
        let save_as = b
            .iter()
            .find(|b| b.command == Command::SaveAs)
            .expect("Save As should be bound");
        assert!(save_as.ctrl && save_as.shift);
        assert_eq!(save_as.key, Key::S);

        let save = b
            .iter()
            .find(|b| b.command == Command::Save)
            .expect("Save should be bound");
        assert!(save.ctrl && !save.shift);
    }

    #[test]
    fn zoom_presets_are_bound_to_the_number_row() {
        let b = bindings();
        let fit_page = b
            .iter()
            .find(|x| x.command == Command::SetZoom(ZoomMode::FitPage))
            .expect("Fit Page should be bound");
        assert_eq!(fit_page.key, Key::Num0);
        assert!(fit_page.ctrl);
    }

    #[test]
    fn more_specific_bindings_are_tried_first() {
        // Ctrl+Shift+S must not be shadowed by Ctrl+S, whose modifier set it
        // contains.
        let b = bindings();
        let mut candidates: Vec<&Binding> = b
            .iter()
            .filter(|x| x.key == Key::S && x.ctrl)
            .collect();
        candidates.sort_by_key(|x| {
            std::cmp::Reverse(usize::from(x.ctrl) + usize::from(x.shift) + usize::from(x.alt))
        });
        assert_eq!(
            candidates.first().map(|c| c.command.clone()),
            Some(Command::SaveAs),
            "the more specific binding should sort first"
        );
    }

    #[test]
    fn no_two_bindings_share_the_same_chord() {
        let b = bindings();
        for (i, x) in b.iter().enumerate() {
            for y in &b[i + 1..] {
                let same = x.ctrl == y.ctrl
                    && x.shift == y.shift
                    && x.alt == y.alt
                    && x.key == y.key;
                assert!(
                    !same,
                    "{:?} and {:?} share a chord",
                    x.command, y.command
                );
            }
        }
    }
}
