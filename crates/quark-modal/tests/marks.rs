//! Marks: `ma` to name a place, `` `a `` and `'a` to come back.
//!
//! The easy half is setting and jumping. The half worth testing is that a
//! mark still points at the same *text* after an hour of editing above it —
//! a mark that confidently takes you somewhere wrong is worse than no mark.


mod common;
use common::*;

/// A workspace with a fixture open. Real files rather than an injected
/// string, so these go through exactly the path the editor does — nothing
/// here is reaching past the seam to set up.
///
/// `fixtures/marks.txt` is five lines: alpha, beta, gamma, delta, epsilon.
fn over(fixture: &str) -> Workspace {
    let mut ws = Workspace::rooted("../..");
    let events = ws.apply(Command::OpenBuffer {
        path: format!("crates/quark-modal/tests/fixtures/{fixture}"),
    });
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::Notice {
                level: NoticeLevel::Error,
                ..
            }
        )),
        "the fixture should have opened: {events:?}"
    );
    ws
}

fn keys(ws: &mut Workspace, s: &str) -> Vec<Event> {
    let mut out = Vec::new();
    for c in s.chars() {
        out = ws.apply(Command::Key { key: Key::char(c) });
    }
    out
}

/// Where the cursor is, as (line, column).
fn at(ws: &Workspace) -> (usize, usize) {
    let buffer = ws.editor().buffer();
    let position = buffer.position(buffer.cursor.offset);
    (position.line, position.column)
}

#[test]
fn a_mark_remembers_a_place_and_takes_you_back_to_it() {
    let mut ws = over("marks.txt");
    keys(&mut ws, "jjll"); // line 2, column 2 — the `m` of `gamma`
    assert_eq!(at(&ws), (2, 2));

    keys(&mut ws, "ma");
    keys(&mut ws, "gg");
    assert_eq!(at(&ws), (0, 0));

    keys(&mut ws, "`a");
    assert_eq!(at(&ws), (2, 2), "backtick returns to the exact column");
}

#[test]
fn a_quote_goes_to_the_line_and_a_backtick_to_the_column() {
    // The distinction is the entire reason both keys exist.
    let mut ws = over("indented.txt");
    keys(&mut ws, "lllll"); // partway into the indented line
    keys(&mut ws, "ma");
    keys(&mut ws, "j");

    keys(&mut ws, "'a");
    assert_eq!(at(&ws), (0, 2), "quote lands on the first non-blank");

    keys(&mut ws, "j");
    keys(&mut ws, "`a");
    assert_eq!(at(&ws), (0, 5), "backtick lands on the column");
}

#[test]
fn a_mark_moves_with_the_text_it_marked() {
    // The whole difficulty. A mark set at an offset and then left alone
    // through edits *above* it is not at that offset any more.
    let mut ws = over("marks.txt");
    keys(&mut ws, "jjj"); // `delta`
    keys(&mut ws, "ma");
    assert_eq!(at(&ws), (3, 0));

    // Insert two lines above it.
    keys(&mut ws, "gg");
    keys(&mut ws, "Ofirst");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    keys(&mut ws, "Osecond");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    keys(&mut ws, "`a");
    let buffer = ws.editor().buffer();
    let line = buffer.position(buffer.cursor.offset).line;
    assert_eq!(
        buffer.line(line),
        "delta",
        "the mark should still be on the text it was set on, not on line 3"
    );
}

#[test]
fn a_mark_inside_deleted_text_collapses_rather_than_dangling() {
    // There is no honest answer for "where is the mark that was in the text
    // you just deleted". Landing where that text began is defensible;
    // pointing past the end of the buffer is not.
    let mut ws = over("marks.txt");
    keys(&mut ws, "jj"); // `gamma`
    keys(&mut ws, "ma");
    keys(&mut ws, "gg");
    keys(&mut ws, "4dd"); // take the first four lines, mark and all

    keys(&mut ws, "`a");
    let buffer = ws.editor().buffer();
    assert!(
        buffer.cursor.offset <= buffer.len_chars(),
        "the mark must not point past the end of the buffer"
    );
    assert_eq!(at(&ws), (0, 0));
}

#[test]
fn an_operator_can_take_a_mark_as_its_motion() {
    // `d'a` is the pay-off from marks being an *object* rather than a special
    // case: the operator machinery needed no changes at all.
    let mut ws = over("marks.txt");
    keys(&mut ws, "jjj"); // `delta`
    keys(&mut ws, "ma");
    keys(&mut ws, "gg");
    keys(&mut ws, "d'a");

    assert_eq!(
        ws.editor().buffer().contents(),
        "epsilon\n",
        "`d'a` is linewise — it takes whole lines, not a rectangle"
    );
}

#[test]
fn jumping_to_a_mark_that_was_never_set_does_nothing() {
    let mut ws = over("marks.txt");
    keys(&mut ws, "jj");
    let before = at(&ws);
    keys(&mut ws, "`z");
    assert_eq!(at(&ws), before, "an unset mark is not a jump to the top");
}

#[test]
fn a_mark_name_is_taken_literally_whatever_it_looks_like() {
    // After `m` the next key is a name, not a count and not a command. `m1`
    // sets a mark called `1`; without that rule it would set a count of one
    // and then wonder what `m` meant.
    let mut ws = over("marks.txt");
    keys(&mut ws, "jj");
    keys(&mut ws, "m1");
    keys(&mut ws, "gg");
    keys(&mut ws, "`1");
    assert_eq!(at(&ws), (2, 0));
}

#[test]
fn each_buffer_keeps_its_own_marks() {
    // Marks live on the buffer, not on the editor, so `a` in one file has
    // nothing to do with `a` in another.
    let mut ws = over("marks.txt");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/src/vim.rs".into(),
    });
    keys(&mut ws, "jjjj");
    keys(&mut ws, "ma");
    let marked = at(&ws);

    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/src/editor.rs".into(),
    });
    keys(&mut ws, "`a");
    assert_eq!(at(&ws), (0, 0), "the other file's mark must not apply here");

    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/src/vim.rs".into(),
    });
    keys(&mut ws, "gg");
    keys(&mut ws, "`a");
    assert_eq!(at(&ws), marked, "and this file's mark survived the trip");
}
