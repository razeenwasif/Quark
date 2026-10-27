//! M1's exit criterion, as tests: `2dd`, `ciw`, `.`, `u` and a recorded macro
//! all behave, driven through the same seam the shell uses.


mod common;
use common::*;

/// Drive the workspace with a literal key sequence. `<Esc>` and `<CR>` stand
/// for the named keys so a test reads like something you would actually type.
fn type_keys(ws: &mut Workspace, keys: &str) {
    let mut rest = keys;
    while !rest.is_empty() {
        let key = if let Some(tail) = rest.strip_prefix("<Esc>") {
            rest = tail;
            Key::named("Escape")
        } else if let Some(tail) = rest.strip_prefix("<CR>") {
            rest = tail;
            Key::named("Enter")
        } else if let Some(tail) = rest.strip_prefix("<BS>") {
            rest = tail;
            Key::named("Backspace")
        } else if let Some(tail) = rest.strip_prefix("<Tab>") {
            rest = tail;
            Key::named("Tab")
        } else {
            let c = rest.chars().next().unwrap();
            rest = &rest[c.len_utf8()..];
            Key::char(c)
        };
        ws.apply(Command::Key { key });
    }
}

fn open(text: &str) -> Workspace {
    let mut ws = Workspace::new();
    // Fill the empty scratch buffer as a paste would — typed, its lines would
    // be indented and its brackets closed — then undo history is irrelevant
    // because we reset the cursor afterwards.
    ws.apply(Command::Key {
        key: Key::char('i'),
    });
    ws.paste_text(text);
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    type_keys(&mut ws, "gg");
    ws
}

fn text(ws: &Workspace) -> String {
    ws.editor().buffer().contents()
}

fn cursor_line(ws: &Workspace) -> usize {
    let buf = ws.editor().buffer();
    buf.position(buf.cursor.offset).line
}

#[test]
fn typing_inserts_text() {
    let ws = open("hello");
    assert_eq!(text(&ws), "hello");
}

#[test]
fn dd_and_2dd_delete_lines() {
    let mut ws = open("one\ntwo\nthree\nfour");
    type_keys(&mut ws, "jdd");
    assert_eq!(text(&ws), "one\nthree\nfour");

    let mut ws = open("one\ntwo\nthree\nfour");
    type_keys(&mut ws, "j2dd");
    assert_eq!(text(&ws), "one\nfour");
}

#[test]
fn ciw_replaces_the_word_under_the_cursor() {
    let mut ws = open("alpha beta gamma");
    type_keys(&mut ws, "wciwBETA<Esc>");
    assert_eq!(text(&ws), "alpha BETA gamma");
}

#[test]
fn daw_takes_the_trailing_space_too() {
    let mut ws = open("alpha beta gamma");
    type_keys(&mut ws, "wdaw");
    assert_eq!(text(&ws), "alpha gamma");
}

#[test]
fn di_paren_empties_the_call() {
    let mut ws = open("call(a, b) end");
    type_keys(&mut ws, "wdi(");
    assert_eq!(text(&ws), "call() end");
}

#[test]
fn dot_repeats_the_whole_change_including_the_typing() {
    // The reason `.` stores a change rather than an action: repeating `ciw`
    // has to retype what was inserted.
    let mut ws = open("one two three");
    type_keys(&mut ws, "ciwX<Esc>");
    assert_eq!(text(&ws), "X two three");

    type_keys(&mut ws, "w.");
    assert_eq!(text(&ws), "X X three");

    type_keys(&mut ws, "w.");
    assert_eq!(text(&ws), "X X X");
}

#[test]
fn undo_walks_back_through_every_change() {
    let mut ws = open("one two");
    type_keys(&mut ws, "ciwX<Esc>");
    assert_eq!(text(&ws), "X two");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "one two");
}

#[test]
fn a_recorded_macro_replays() {
    // qa … q records; @a replays. The macro stores resolved steps, so it is
    // the change that repeats, not the keystrokes.
    let mut ws = open("a\nb\nc\nd");
    type_keys(&mut ws, "qaI- <Esc>jq");
    assert_eq!(text(&ws), "- a\nb\nc\nd");

    type_keys(&mut ws, "@a");
    assert_eq!(text(&ws), "- a\n- b\nc\nd");

    type_keys(&mut ws, "@a");
    assert_eq!(text(&ws), "- a\n- b\n- c\nd");
}

#[test]
fn motions_move_the_cursor_without_touching_the_text() {
    let mut ws = open("one\ntwo\nthree");
    let before = text(&ws);
    type_keys(&mut ws, "jj");
    assert_eq!(cursor_line(&ws), 2);
    assert_eq!(text(&ws), before);
}




#[test]
fn yank_and_put_round_trip_a_line() {
    let mut ws = open("one\ntwo");
    type_keys(&mut ws, "yyp");
    assert_eq!(text(&ws), "one\none\ntwo");
}

#[test]
fn opening_a_missing_file_is_a_notice_not_a_panic() {
    let mut ws = Workspace::new();
    let events = ws.apply(Command::OpenBuffer {
        path: "does/not/exist.rs".into(),
    });
    assert!(
        events.iter().any(|e| matches!(e, Event::Notice { .. })),
        "{events:?}"
    );
}

#[test]
fn proton_can_open_its_own_source() {
    // The literal M1 exit criterion: the editor opens a real file from this
    // repository and reports its lines.
    let mut ws = Workspace::rooted("../..");
    let events = ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/src/session.rs".into(),
    });
    assert!(
        !events.iter().any(|e| matches!(e, Event::Notice { .. })),
        "could not open its own source: {events:?}"
    );

    let buf = ws.editor().buffer();
    assert!(
        buf.len_lines() > 100,
        "expected a real file, got {} lines",
        buf.len_lines()
    );
    assert!(buf.contents().contains("pub struct Session"));
    assert_eq!(buf.name(), "session.rs");
}

/* ── moving without leaving insert mode ───────────────────────────────
A modal editor that traps you at one position until you press Escape is
not being stricter than vim — it is being broken. */

#[test]
fn the_arrows_move_in_insert_mode() {
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });
    type_keys(&mut ws, "i");
    assert_eq!(ws.vim_mode(), quark_modal::VimMode::Insert);

    let start = ws.editor().buffer().cursor.offset;
    ws.apply(Command::Key {
        key: Key::named("ArrowDown"),
    });
    let moved = ws.editor().buffer().cursor.offset;
    assert!(moved > start, "ArrowDown went nowhere: {start} -> {moved}");
    assert_eq!(
        ws.vim_mode(),
        quark_modal::VimMode::Insert,
        "and it is still insert mode"
    );

    ws.apply(Command::Key {
        key: Key::named("ArrowUp"),
    });
    assert_eq!(ws.editor().buffer().cursor.offset, start, "and back");

    // Typing still types, which is the thing that must not have broken.
    ws.apply(Command::Key {
        key: Key::char('x'),
    });
    assert!(ws.editor().buffer().line(0).starts_with('x'));
}

#[test]
fn home_and_end_work_in_insert_mode_too() {
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });
    type_keys(&mut ws, "i");

    ws.apply(Command::Key {
        key: Key::named("End"),
    });
    let at_end = ws
        .editor()
        .buffer()
        .position(ws.editor().buffer().cursor.offset);
    assert!(at_end.column > 0, "End went nowhere");

    ws.apply(Command::Key {
        key: Key::named("Home"),
    });
    let at_start = ws
        .editor()
        .buffer()
        .position(ws.editor().buffer().cursor.offset);
    assert_eq!(at_start.column, 0);
}

/* ── clicking ─────────────────────────────────────────────────────────
A motion cannot express this: `Move` is always relative to where the
cursor already is, and a mouse knows exactly where it wants to be. */

#[test]
fn a_click_puts_the_cursor_where_it_landed() {
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });

    // Line 0 is `  indented line`; line 2 is the trailing empty one.
    ws.apply(Command::PlaceCursor { line: 0, column: 3 });
    let at = ws
        .editor()
        .buffer()
        .position(ws.editor().buffer().cursor.offset);
    assert_eq!((at.line, at.column), (0, 3));
}

#[test]
fn a_click_past_the_end_of_a_short_line_lands_on_the_line() {
    // The ordinary case, not an error: the front end does not hold the
    // document and cannot know how long the line is before it clicks.
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });

    ws.apply(Command::PlaceCursor {
        line: 1,
        column: 9999,
    });
    let at = ws
        .editor()
        .buffer()
        .position(ws.editor().buffer().cursor.offset);
    assert_eq!(at.line, 1, "still on the line that was clicked");

    // And past the end of the file lands on the last line rather than panicking.
    ws.apply(Command::PlaceCursor {
        line: 9999,
        column: 0,
    });
    let at = ws
        .editor()
        .buffer()
        .position(ws.editor().buffer().cursor.offset);
    assert!(at.line < ws.editor().buffer().len_lines());
}


#[test]
fn a_drag_leaves_a_visual_selection() {
    // Not a selection the modal engine cannot see: a drag has to leave you
    // somewhere `y` and `d` mean what you expect.
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });

    ws.apply(Command::PlaceCursor { line: 0, column: 2 });
    let anchor = ws.editor().buffer().cursor.offset;
    ws.apply(Command::SelectTo { line: 0, column: 8 });

    let cursor = ws.editor().buffer().cursor;
    assert_eq!(ws.vim_mode(), quark_modal::VimMode::Visual);
    assert!(cursor.has_selection());
    assert_eq!(cursor.anchor, anchor, "the anchor stayed where it started");
    assert!(cursor.offset > anchor, "and the head moved");
}

#[test]
fn a_drag_that_goes_nowhere_is_a_click_and_keeps_insert_mode() {
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });
    type_keys(&mut ws, "i");
    ws.apply(Command::PlaceCursor { line: 1, column: 2 });
    // The pointer twitched, and stayed on the character pressed on.
    ws.apply(Command::SelectTo { line: 1, column: 2 });
    assert_eq!(ws.vim_mode(), quark_modal::VimMode::Insert, "a click left insert mode");
    assert!(!ws.editor().buffer().cursor.has_selection());
    // A real drag still selects.
    ws.apply(Command::SelectTo { line: 1, column: 6 });
    assert_eq!(ws.vim_mode(), quark_modal::VimMode::Visual);
}

#[test]
fn dragging_further_moves_the_head_and_not_the_anchor() {
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });

    ws.apply(Command::PlaceCursor { line: 0, column: 2 });
    let anchor = ws.editor().buffer().cursor.offset;
    for column in [4, 8, 12] {
        ws.apply(Command::SelectTo { line: 0, column });
        assert_eq!(
            ws.editor().buffer().cursor.anchor,
            anchor,
            "the anchor moved at column {column}"
        );
    }
}

#[test]
fn a_drag_inside_a_line_selection_keeps_it_linewise() {
    // Dragging inside a `V` selection must not silently downgrade it to
    // charwise — the shape was chosen deliberately.
    let mut ws = Workspace::rooted("../..");
    ws.apply(Command::OpenBuffer {
        path: "crates/quark-modal/tests/fixtures/indented.txt".into(),
    });

    type_keys(&mut ws, "V");
    ws.apply(Command::SelectTo { line: 1, column: 2 });
    assert_eq!(ws.vim_mode(), quark_modal::VimMode::VisualLine);
}

/* Undo boundaries follow user edits, not individual IPC messages. */
fn named(ws: &mut Workspace, name: &str) {
    ws.apply(Command::Key { key: Key::named(name) });
}

fn redo(ws: &mut Workspace) {
    ws.apply(Command::Perform {
        action: quark_modal::action::Action::new(
            quark_modal::action::Verb::Redo,
            quark_modal::action::Object::Here,
            1,
        ),
    });
}

#[test]
fn continuous_typing_with_newlines_and_backspace_is_one_undo() {
    let mut ws = Workspace::new();
    type_keys(&mut ws, "ihello 🚀<CR>world<BS>!<Esc>");
    let after = text(&ws);
    assert_eq!(after, "hello 🚀\nworl!");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "");
    redo(&mut ws);
    assert_eq!(text(&ws), after);
}

#[test]
fn replace_mode_typing_undoes_as_a_single_run() {
    let mut ws = open("original");
    type_keys(&mut ws, "Rseveral characters<Esc>");
    let after = text(&ws);
    assert_ne!(after, "original");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "original");
    redo(&mut ws);
    assert_eq!(text(&ws), after);
}

#[test]
fn separate_insert_sessions_are_separate_undo_steps() {
    let mut ws = Workspace::new();
    type_keys(&mut ws, "ifirst<Esc>A second<Esc>u");
    assert_eq!(text(&ws), "first");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "");
}

#[test]
fn cursor_keys_start_a_new_typing_run() {
    let mut ws = Workspace::new();
    type_keys(&mut ws, "iabc");
    named(&mut ws, "ArrowLeft");
    type_keys(&mut ws, "XY<Esc>u");
    assert_eq!(text(&ws), "abc");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "");
    redo(&mut ws);
    redo(&mut ws);
    assert_eq!(text(&ws), "abXYc");
}

#[test]
fn clicking_elsewhere_starts_a_new_typing_run() {
    let mut ws = Workspace::new();
    type_keys(&mut ws, "iabc");
    ws.apply(Command::PlaceCursor { line: 0, column: 0 });
    type_keys(&mut ws, "XY<Esc>u");
    assert_eq!(text(&ws), "abc");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "");
}



#[test]
fn dot_repeat_is_one_undo_step_including_its_deletion() {
    let mut ws = open("one two three");
    type_keys(&mut ws, "ciwlong<Esc>w");
    let before = text(&ws);
    type_keys(&mut ws, ".");
    let after = text(&ws);
    assert_eq!(after, "long long three");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), before);
    redo(&mut ws);
    assert_eq!(text(&ws), after);
}

#[test]
fn a_macro_invocation_is_one_undo_step() {
    let mut ws = open("one\ntwo\nthree");
    type_keys(&mut ws, "qaI- <Esc>jq");
    let before = text(&ws);
    type_keys(&mut ws, "@a");
    let after = text(&ws);
    assert_eq!(after, "- one\n- two\nthree");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), before);
    redo(&mut ws);
    assert_eq!(text(&ws), after);
}

#[test]
fn entering_insert_without_typing_does_not_add_an_undo_step() {
    let mut ws = open("one two");
    type_keys(&mut ws, "ciwchanged<Esc>i<Esc>u");
    assert_eq!(text(&ws), "one two");
}

#[test]
fn save_in_insert_mode_keeps_later_typing_dirty_and_separately_undoable() {
    let root = std::env::temp_dir().join(format!("proton-undo-save-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("note.txt");
    std::fs::write(&path, "base").unwrap();
    let mut ws = Workspace::rooted(&root);
    ws.apply(Command::OpenBuffer { path: "note.txt".into() });
    type_keys(&mut ws, "A saved");
    ws.apply(Command::Save);
    assert!(!ws.editor().buffer().is_modified());
    type_keys(&mut ws, " unsaved<Esc>");
    assert!(ws.editor().buffer().is_modified());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "base saved");
    type_keys(&mut ws, "u");
    assert_eq!(text(&ws), "base saved");
    assert!(!ws.editor().buffer().is_modified());
    redo(&mut ws);
    assert_eq!(text(&ws), "base saved unsaved");
    assert!(ws.editor().buffer().is_modified());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn control_r_redoes_a_grouped_change() {
    let mut ws = Workspace::new();
    type_keys(&mut ws, "ityped together<Esc>u");
    let mut key = Key::char('r');
    key.ctrl = true;
    ws.apply(Command::Key { key });
    assert_eq!(text(&ws), "typed together");
}

#[test]
fn tab_in_insert_mode_indents_to_the_next_stop() {
    // Spaces, four to a stop: a Tab at the start of a line is a whole indent,
    // and one after a character goes only as far as the next stop, so what
    // follows lines up with the lines around it.
    let mut ws = open("ab");
    type_keys(&mut ws, "I<Tab><Esc>");
    assert_eq!(text(&ws), "    ab");
    type_keys(&mut ws, "Ax<Tab>y<Esc>");
    assert_eq!(text(&ws), "    abx y");
}

#[test]
fn tab_in_a_file_indented_with_tabs_is_a_tab() {
    let mut ws = open("\tone\n\ttwo\nthree");
    type_keys(&mut ws, "GI<Tab><Esc>");
    assert_eq!(text(&ws), "\tone\n\ttwo\n\tthree");
}


#[test]
fn the_page_keys_move_by_the_view_and_grow_a_selection() {
    // Twenty lines on screen: half of that for ⌃d and ⌃u, all of it less two
    // lines of overlap for the page keys — and in visual-line mode they
    // extend the selection, so ⌃d after `V` selects eleven lines.
    let mut ws = open(&"x\n".repeat(60));
    ws.apply(Command::Viewport {
        view: 0,
        first_line: 0,
        lines: 20,
    });
    ws.apply(Command::Key { key: Key::ctrl('d') });
    assert_eq!(cursor_line(&ws), 10);
    ws.apply(Command::Key {
        key: Key::named("PageDown"),
    });
    assert_eq!(cursor_line(&ws), 28);
    ws.apply(Command::Key { key: Key::ctrl('u') });
    assert_eq!(cursor_line(&ws), 18);

    type_keys(&mut ws, "gg");
    ws.apply(Command::Key {
        key: Key {
            shift: true,
            ..Key::char('V')
        },
    });
    ws.apply(Command::Key { key: Key::ctrl('d') });
    type_keys(&mut ws, "d");
    assert_eq!(text(&ws), "x\n".repeat(49));
}
