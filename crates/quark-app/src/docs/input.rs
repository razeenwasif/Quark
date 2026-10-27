//! The keyboard, while the page has focus.
//!
//! Text and the clipboard arrive as egui's own events; everything else is a
//! key, read here the way a word processor reads it. The shortcuts are Google
//! Docs' where it has one, so hands that know it already know these.

use std::time::Instant;

use egui::{Event, ImeEvent, Key, Modifiers};
use quark_doc::{BlockKind, Marks, Pos, Selection};

use super::DocTab;

/// What a key asked for that the page cannot do by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ask {
    /// ⌘K: edit the link at the selection.
    Link,
    /// ⌘⌥M: comment on the selection.
    Comment,
    /// ⌘F and ⌘H.
    Find { replace: bool },
    /// ⌘G and ⇧⌘G.
    FindNext { back: bool },
    /// ⌘J: ask Claude about the selection.
    Claude,
    /// ⌘↩ and Esc on a suggestion under the caret.
    Accept(u32),
    Reject(u32),
}

/// Reads this frame's input into the document. Returns what it could not
/// do itself.
pub(crate) fn read(tab: &mut DocTab, ctx: &egui::Context) -> Vec<Ask> {
    let events = ctx.input(|i| i.events.clone());
    let mut asks = Vec::new();
    for event in events {
        let before = tab.editor.selection();
        let state = tab.editor.state();
        let mut vertical = false;
        match event {
            Event::Text(text) => {
                if !text.is_empty() {
                    tab.editor.type_text(&text);
                }
            }
            Event::Ime(ImeEvent::Preedit { text, .. }) => tab.preedit = text,
            Event::Ime(ImeEvent::Commit(text)) => {
                tab.preedit.clear();
                if !text.is_empty() {
                    tab.editor.type_text(&text);
                }
            }
            Event::Copy => copy(tab, ctx),
            Event::Cut => {
                copy(tab, ctx);
                tab.editor.delete_selection();
            }
            Event::Paste(text) => tab.editor.paste_markdown(&text),
            Event::Key { key, pressed: true, modifiers, .. } => {
                vertical = matches!(key, Key::ArrowUp | Key::ArrowDown) && !modifiers.command && !modifiers.alt;
                if let Some(ask) = key_press(tab, key, modifiers) {
                    asks.push(ask);
                }
            }
            _ => {}
        }
        if tab.editor.selection() != before || tab.editor.state() != state {
            tab.reveal = true;
            tab.blink = Instant::now();
            if !vertical {
                tab.goal_x = None;
            }
        }
    }
    asks
}

/// The selection to the clipboard: as plain text when it is plain words in
/// one block, otherwise as Markdown, which pastes back with its formatting.
fn copy(tab: &mut DocTab, ctx: &egui::Context) {
    let sel = tab.editor.selection();
    if sel.is_caret() {
        return;
    }
    let (a, b) = sel.ordered();
    let plain = a.block == b.block
        && tab.editor.doc.block(a.block).kind == BlockKind::Paragraph
        && tab.editor.doc.styles_between(a, b).all(|s| s.marks.is_empty() && s.link.is_none());
    let text = if plain { tab.editor.selected_text() } else { tab.editor.selected_markdown().trim_end().to_string() };
    ctx.copy_text(text);
}

/// The suggestion the caret is in or beside, if any.
fn suggestion_here(tab: &DocTab) -> Option<u32> {
    let at = tab.editor.selection().head;
    let block = tab.editor.doc.block(at.block);
    let styles = block.styles();
    let near = [at.offset.checked_sub(1), Some(at.offset)];
    near.into_iter().flatten().filter_map(|i| styles.get(i)).flat_map(|s| s.anchors.iter()).copied().find(|id| tab.editor.doc.suggestion(*id).is_some())
}

fn move_to(tab: &mut DocTab, to: Pos, extend: bool) {
    if extend {
        tab.editor.extend_to(to);
    } else {
        tab.editor.caret_to(to);
    }
}

fn key_press(tab: &mut DocTab, key: Key, m: Modifiers) -> Option<Ask> {
    let ed = &mut tab.editor;
    let extend = m.shift;
    // ⌘ on a Mac, Ctrl elsewhere; on a Mac, Ctrl is left to the system.
    if m.command {
        match key {
            Key::B => ed.toggle_mark(Marks::BOLD),
            Key::I => ed.toggle_mark(Marks::ITALIC),
            Key::U => ed.toggle_mark(Marks::UNDERLINE),
            Key::X if m.shift => ed.toggle_mark(Marks::STRIKE),
            Key::E => ed.toggle_mark(Marks::CODE),
            Key::K => return Some(Ask::Link),
            Key::Z if m.shift => {
                ed.redo();
            }
            Key::Z => {
                ed.undo();
            }
            Key::Y => {
                ed.redo();
            }
            Key::A => ed.select_all(),
            Key::Backslash => ed.clear_formatting(),
            Key::Num0 if m.alt => ed.set_kind(BlockKind::Paragraph),
            Key::Num1 if m.alt => ed.set_kind(BlockKind::Heading(1)),
            Key::Num2 if m.alt => ed.set_kind(BlockKind::Heading(2)),
            Key::Num3 if m.alt => ed.set_kind(BlockKind::Heading(3)),
            Key::Num7 if m.shift => ed.toggle_kind(BlockKind::Numbered { depth: 0 }),
            Key::Num8 if m.shift => ed.toggle_kind(BlockKind::Bullet { depth: 0 }),
            Key::Num9 if m.shift => ed.toggle_kind(BlockKind::Task { checked: false, depth: 0 }),
            Key::M if m.alt => return Some(Ask::Comment),
            Key::F => return Some(Ask::Find { replace: false }),
            Key::H => return Some(Ask::Find { replace: true }),
            Key::G => return Some(Ask::FindNext { back: m.shift }),
            Key::J => return Some(Ask::Claude),
            Key::Enter => return suggestion_here(tab).map(Ask::Accept),
            Key::Equals | Key::Plus => tab.zoom = (tab.zoom + 0.1).min(2.5),
            Key::Minus => tab.zoom = (tab.zoom - 0.1).max(0.5),
            Key::Num0 => tab.zoom = 1.0,
            Key::ArrowUp => move_to(tab, tab.editor.doc.start(), extend),
            Key::ArrowDown => move_to(tab, tab.editor.doc.end(), extend),
            Key::ArrowLeft => {
                let to = tab.sheet.line_end(tab.editor.selection().head, false);
                move_to(tab, to, extend);
            }
            Key::ArrowRight => {
                let to = tab.sheet.line_end(tab.editor.selection().head, true);
                move_to(tab, to, extend);
            }
            Key::Backspace => {
                // To the start of the line.
                let head = tab.editor.selection().head;
                let start = tab.sheet.line_end(head, false);
                if tab.editor.selection().is_caret() && start != head {
                    tab.editor.set_selection(Selection::new(start, head));
                }
                tab.editor.backspace(false);
            }
            _ => {}
        }
        return None;
    }
    match key {
        Key::Enter if m.shift => ed.line_break(),
        Key::Enter => ed.enter(),
        Key::Backspace => ed.backspace(m.alt),
        Key::Delete => ed.delete_forward(m.alt),
        Key::Tab => {
            if !ed.indent(!m.shift) && !m.shift {
                ed.type_text("\t");
            }
        }
        Key::Escape => {
            if let Some(id) = suggestion_here(tab) {
                return Some(Ask::Reject(id));
            }
            let head = tab.editor.selection().head;
            tab.editor.caret_to(head);
        }
        Key::ArrowLeft | Key::ArrowRight => ed.move_horizontal(key == Key::ArrowRight, m.alt, extend),
        Key::ArrowUp | Key::ArrowDown => {
            let down = key == Key::ArrowDown;
            let head = tab.editor.selection().head;
            if !extend && !tab.editor.selection().is_caret() {
                let (a, b) = tab.editor.selection().ordered();
                tab.editor.caret_to(if down { b } else { a });
            }
            if m.alt {
                // ⌥↑ and ⌥↓: to the start or end of the paragraph.
                let len = tab.editor.doc.block(head.block).len();
                let to = match (down, head.offset) {
                    (true, o) if o < len => Pos::new(head.block, len),
                    (true, _) => Pos::new((head.block + 1).min(tab.editor.doc.len() - 1), tab.editor.doc.block((head.block + 1).min(tab.editor.doc.len() - 1)).len()),
                    (false, 0) => Pos::new(head.block.saturating_sub(1), 0),
                    (false, _) => Pos::new(head.block, 0),
                };
                move_to(tab, to, extend);
                return None;
            }
            let x = *tab.goal_x.get_or_insert_with(|| tab.sheet.caret(head).center().x);
            let to = tab.sheet.vertical(head, down, x).unwrap_or(if down { tab.editor.doc.end() } else { tab.editor.doc.start() });
            move_to(tab, to, extend);
        }
        Key::Home => {
            let to = tab.sheet.line_end(tab.editor.selection().head, false);
            move_to(tab, to, extend);
        }
        Key::End => {
            let to = tab.sheet.line_end(tab.editor.selection().head, true);
            move_to(tab, to, extend);
        }
        _ => {}
    }
    None
}
