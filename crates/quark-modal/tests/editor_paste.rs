//! ⌘V in the editor: the clipboard's text put where vim's `"+` register
//! would put it — as copied in insert mode, before the cursor in normal mode,
//! over the selection in visual mode — and taken back by one undo.

mod common;
use common::*;

/// A buffer holding `text`, the cursor at its start, in normal mode.
fn holding(text: &str) -> Workspace {
    let mut ws = Workspace::new();
    ws.apply(Command::Key { key: Key::char('i') });
    ws.paste_text(text);
    ws.apply(Command::Key { key: Key::named("Escape") });
    for c in "gg0".chars() {
        ws.apply(Command::Key { key: Key::char(c) });
    }
    ws
}

fn keys(ws: &mut Workspace, s: &str) {
    for c in s.chars() {
        let mut k = Key::char(c);
        k.shift = c.is_uppercase();
        ws.apply(Command::Key { key: k });
    }
}

fn paste(ws: &mut Workspace, text: &str) {
    ws.apply(Command::EditorPaste { text: text.into() });
}

fn at(ws: &Workspace) -> (usize, usize) {
    let buf = ws.editor().buffer();
    let p = buf.position(buf.cursor.offset);
    (p.line, p.column)
}

#[test]
fn in_insert_mode_it_goes_in_as_copied_and_typing_carries_on_after_it() {
    let mut ws = holding("int main() {}\n");
    keys(&mut ws, "f{a");
    // Typed, these lines would be indented again and the brace closed twice.
    paste(&mut ws, "\n    if (x) {\n        y();\n    }\n");
    assert_eq!(ws.editor().buffer().contents(), "int main() {\n    if (x) {\n        y();\n    }\n}\n");
    assert_eq!(ws.vim_mode(), quark_modal::vim::VimMode::Insert, "a paste left insert mode");
    keys(&mut ws, "z");
    assert_eq!(ws.editor().buffer().contents(), "int main() {\n    if (x) {\n        y();\n    }\nz}\n", "typing did not go on after the paste");
}

#[test]
fn in_normal_mode_it_goes_before_the_cursor_and_whole_lines_go_above() {
    let mut ws = holding("one two\nthree\n");
    keys(&mut ws, "w");
    paste(&mut ws, "and ");
    assert_eq!(ws.editor().buffer().contents(), "one and two\nthree\n");
    assert_eq!(at(&ws), (0, 7), "the cursor is not on the last character put");
    keys(&mut ws, "j");
    paste(&mut ws, "two and a half\n");
    assert_eq!(ws.editor().buffer().contents(), "one and two\ntwo and a half\nthree\n");
    assert_eq!(at(&ws), (1, 0));
}

#[test]
fn in_visual_mode_it_takes_the_selections_place_and_ends_the_selection() {
    let mut ws = holding("keep this word here\n");
    keys(&mut ws, "wwve");
    paste(&mut ws, "line");
    assert_eq!(ws.editor().buffer().contents(), "keep this line here\n");
    assert_eq!(ws.vim_mode(), quark_modal::vim::VimMode::Normal);
}

#[test]
fn one_undo_takes_a_paste_back_whole() {
    let mut ws = holding("a\n");
    keys(&mut ws, "A");
    keys(&mut ws, "bc");
    paste(&mut ws, "xyz\nmore\n");
    ws.apply(Command::Key { key: Key::named("Escape") });
    keys(&mut ws, "u");
    assert_eq!(ws.editor().buffer().contents(), "abc\n", "undo took back more or less than the paste");
    // Windows' line ends come in as the file's.
    let mut ws = holding("\n");
    paste(&mut ws, "a\r\nb\r\n");
    assert_eq!(ws.editor().buffer().contents(), "a\nb\n\n");
}

/// What a key sends for the system clipboard, if anything.
fn copied(ws: &mut Workspace, s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in s.chars() {
        let mut k = Key::char(c);
        k.shift = c.is_uppercase();
        for e in ws.apply(Command::Key { key: k }) {
            if let Event::Clipboard { text } = e {
                out.push(text);
            }
        }
    }
    out
}

#[test]
fn a_yank_or_a_delete_goes_on_the_system_clipboard_and_a_named_register_does_not() {
    let mut ws = holding("one two\nthree\n");
    assert_eq!(copied(&mut ws, "yy"), ["one two\n"], "yy did not reach the clipboard");
    assert_eq!(copied(&mut ws, "jdd"), ["three\n"], "dd did not reach the clipboard");
    assert_eq!(copied(&mut ws, "k\"ayy"), Vec::<String>::new(), "a named register reached the clipboard");
    assert_eq!(copied(&mut ws, "0yw"), ["one "]);
    // And `p` still puts what was yanked.
    keys(&mut ws, "P");
    assert_eq!(ws.editor().buffer().contents(), "one one two\n");
}
