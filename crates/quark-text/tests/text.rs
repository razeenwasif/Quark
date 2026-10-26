use quark_text::edit::Range;
use quark_text::{Buffer, Cursor, Edit, Position};

fn buf(text: &str, at: usize) -> Buffer {
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor::at(at);
    b
}

/* ── editing ─────────────────────────────────────────────────────── */

#[test]
fn an_edit_round_trips_through_undo() {
    let mut b = Buffer::from_text("hello world");
    b.apply(vec![Edit::delete(Range::new(0, 6))], Cursor::at(0));
    assert_eq!(b.contents(), "world");

    assert!(b.undo());
    assert_eq!(b.contents(), "hello world");
    assert!(b.redo());
    assert_eq!(b.contents(), "world");
}

#[test]
fn several_edits_in_one_change_undo_together() {
    // Applying back to front is what keeps the earlier offsets valid.
    let mut b = Buffer::from_text("aaa bbb ccc");
    b.apply(
        vec![
            Edit::replace(Range::new(0, 3), "xx"),
            Edit::replace(Range::new(8, 11), "yy"),
        ],
        Cursor::at(0),
    );
    assert_eq!(b.contents(), "xx bbb yy");
    assert!(b.undo());
    assert_eq!(b.contents(), "aaa bbb ccc");
}

#[test]
fn undo_is_a_tree_not_a_stack() {
    // The failure a stack has: undo, type something else, and the thing you
    // undid is gone forever. Here the old branch is still reachable.
    let mut b = Buffer::from_text("start");
    b.apply(vec![Edit::insert(5, " one")], Cursor::at(9));
    assert_eq!(b.contents(), "start one");

    b.undo();
    assert_eq!(b.contents(), "start");

    b.apply(vec![Edit::insert(5, " two")], Cursor::at(9));
    assert_eq!(b.contents(), "start two");
    assert_eq!(b.undo_branches(), 0, "the new edit is a leaf");

    b.undo();
    assert_eq!(b.contents(), "start");
    assert_eq!(
        b.undo_branches(),
        2,
        "both branches still hang off this state"
    );

    // Redo follows the branch taken most recently, which is why the tree feels
    // exactly like a stack until you need it not to.
    b.redo();
    assert_eq!(b.contents(), "start two");
}

#[test]
fn undo_at_the_beginning_is_a_no_op() {
    let mut b = Buffer::from_text("x");
    assert!(!b.undo());
    assert_eq!(b.contents(), "x");
}

#[test]
fn modified_tracks_the_saved_revision() {
    let mut b = Buffer::from_text("a");
    assert!(!b.is_modified());
    b.apply(vec![Edit::insert(1, "b")], Cursor::at(2));
    assert!(b.is_modified());
    b.undo();
    assert!(
        !b.is_modified(),
        "undoing back to the saved state is not modified"
    );
}

/* ── positions ───────────────────────────────────────────────────── */

#[test]
fn positions_and_offsets_agree() {
    let b = Buffer::from_text("one\ntwo\nthree");
    assert_eq!(b.position(0).line, 0);
    assert_eq!(b.position(4).line, 1);
    assert_eq!(b.position(5).column, 1);
    assert_eq!(b.offset_of(b.position(9)), 9);
}

#[test]
fn utf16_offsets_are_available_for_lsp() {
    // An emoji is one char to us and two UTF-16 code units to a language
    // server. Getting this wrong is a whole class of off-by-one bugs.
    let b = Buffer::from_text("a🚀b");
    assert_eq!(b.len_chars(), 3);
    assert_eq!(b.utf16_of(2), 3);
}

#[test]
fn the_cursor_cannot_sit_past_the_end_of_a_line() {
    let b = buf("ab\ncd", 0);
    let clamped = b.clamp(Cursor::at(2));
    assert_eq!(clamped.offset, 2, "the newline itself is the limit");
    assert_eq!(b.clamp(Cursor::at(99)).offset, 5);
}

/* ── motions ─────────────────────────────────────────────────────── */

#[test]
fn word_motions_respect_character_classes() {
    //          0123456789
    let b = buf("foo.bar baz", 0);
    assert_eq!(b.word_forward(1, false), 3, "punctuation is its own word");
    assert_eq!(b.word_forward(2, false), 4);
    assert_eq!(b.word_end(1, false), 2);

    let b = buf("foo.bar baz", 0);
    assert_eq!(b.word_forward(1, true), 8, "W skips punctuation");

    let b = buf("foo.bar baz", 8);
    assert_eq!(b.word_backward(1, false), 4);
}

#[test]
fn line_motions_stop_before_the_newline() {
    let b = buf("  hello\nnext", 4);
    assert_eq!(b.line_start(), 0);
    assert_eq!(b.line_end(), 7, "$ lands on text, not on the line break");
    assert_eq!(b.first_non_blank(), 2);
}

#[test]
fn the_goal_column_survives_a_short_line() {
    // Down through a short line and back must return to column 8 — the thing
    // that separates a vim motion from a plain caret.
    let mut b = buf("0123456789\nab\n0123456789", 8);
    let down = b.line_down(1);
    b.cursor = Cursor {
        offset: down,
        anchor: down,
        goal_column: Some(8),
    };
    assert_eq!(b.column(), 2, "clamped to the short line");
    assert_eq!(
        b.position(b.line_down(1)).column,
        8,
        "and restored below it"
    );
}

#[test]
fn dd_takes_the_whole_line_including_its_newline() {
    let b = buf("one\ntwo\nthree", 4);
    assert_eq!(b.line_range(1), Range::new(4, 8));
    assert_eq!(
        b.line_range(2),
        Range::new(4, 13),
        "2dd reaches the last line"
    );
}

/* ── text objects ────────────────────────────────────────────────── */

#[test]
fn inner_and_around_word_differ_by_the_trailing_space() {
    //          0123456789
    let b = buf("foo bar baz", 4);
    assert_eq!(b.inner_word(false), Range::new(4, 7));
    assert_eq!(
        b.a_word(false),
        Range::new(4, 8),
        "aw takes the space after"
    );
}

#[test]
fn around_word_falls_back_to_the_leading_space_at_end_of_line() {
    // vim's asymmetry, and the reason `daw` leaves a sentence spaced right.
    let b = buf("foo bar", 4);
    assert_eq!(b.a_word(false), Range::new(3, 7));
}

#[test]
fn inner_word_on_whitespace_selects_the_whitespace() {
    let b = buf("foo   bar", 4);
    assert_eq!(b.inner_word(false), Range::new(3, 6));
}

#[test]
fn pairs_nest() {
    //          0123456789012345
    let b = buf("f(a, g(b), c) x", 11);
    assert_eq!(b.inside_pair('(', ')', false), Some(Range::new(2, 12)));
    assert_eq!(b.inside_pair('(', ')', true), Some(Range::new(1, 13)));

    let inner = buf("f(a, g(b), c) x", 7);
    assert_eq!(inner.inside_pair('(', ')', false), Some(Range::new(7, 8)));
}

#[test]
fn quotes_do_not_nest() {
    let b = buf(r#"say "hi there" now"#, 8);
    assert_eq!(b.inside_pair('"', '"', false), Some(Range::new(5, 13)));
    assert_eq!(b.inside_pair('"', '"', true), Some(Range::new(4, 14)));
}

#[test]
fn an_unclosed_pair_is_none_rather_than_a_panic() {
    let b = buf("f(a, b", 3);
    assert_eq!(b.inside_pair('(', ')', false), None);
    let b = buf("no parens here", 3);
    assert_eq!(b.inside_pair('(', ')', false), None);
}

/* ── the UTF-16 seam ──────────────────────────────────────────────── */

#[test]
fn lsp_columns_are_utf16_code_units_not_characters() {
    // One emoji is one character to us and two code units to a language
    // server. Get this wrong and every diagnostic on a line containing one is
    // reported in the wrong column — and only for that user.
    let buffer = Buffer::from_text("let a = 1;\nlet 🦀 = \"héllo\";\n");
    let line_two = buffer.offset_of(Position { line: 1, column: 0 });

    // `let ` is 4 characters and 4 code units.
    assert_eq!(buffer.utf16_column(line_two + 4), 4);
    // Just past the crab: 5 characters in, 6 code units in.
    assert_eq!(buffer.utf16_column(line_two + 5), 6);
    // `é` is one code unit, so from here the two counts stay two apart.
    assert_eq!(buffer.utf16_column(line_two + 12), 13);
}

#[test]
fn a_position_from_a_server_comes_back_to_the_right_character() {
    let buffer = Buffer::from_text("let 🦀 = \"héllo\";\n");
    for column in 0..buffer.line(0).chars().count() {
        let offset = buffer.offset_of(Position { line: 0, column });
        let round_trip = buffer.offset_at_utf16(0, buffer.utf16_column(offset));
        assert_eq!(round_trip, offset, "column {column} did not survive");
    }
}

#[test]
fn a_stale_position_is_clamped_rather_than_fatal() {
    // Diagnostics arrive after the edit that invalidated them. Landing on the
    // end of the line is fine; panicking is not.
    let buffer = Buffer::from_text("short\n");
    assert_eq!(buffer.offset_at_utf16(0, 900), buffer.line_end());
    assert_eq!(buffer.offset_at_utf16(900, 0), buffer.len_chars());
}

#[test]
fn undo_ends_an_open_group_before_a_new_history_branch() {
    let mut b = Buffer::from_text("");
    b.begin_group(false);
    b.apply(vec![Edit::insert(0, "first")], Cursor::at(5));
    b.end_group();
    b.begin_group(false);
    b.apply(vec![Edit::insert(5, " old")], Cursor::at(9));
    assert!(b.undo());
    // No end_group call from the consumer: undo itself is the boundary.
    b.ensure_group();
    b.apply(vec![Edit::insert(5, " new")], Cursor::at(9));
    b.undo();
    assert_eq!(b.contents(), "first");
    assert_eq!(b.undo_branches(), 2);
    b.undo();
    assert_eq!(b.contents(), "");
    b.redo();
    b.redo();
    assert_eq!(b.contents(), "first new");
}

#[test]
fn saving_an_open_group_preserves_the_saved_revision() {
    let path =
        std::env::temp_dir().join(format!("quark-text-save-group-{}.txt", std::process::id()));
    std::fs::write(&path, "base").unwrap();
    let mut b = Buffer::open(&path).unwrap();
    b.begin_group(false);
    b.apply(vec![Edit::insert(4, " saved")], Cursor::at(10));
    b.save().unwrap();
    b.ensure_group();
    b.apply(vec![Edit::insert(10, " more")], Cursor::at(15));
    assert!(b.is_modified());
    b.undo();
    assert_eq!(b.contents(), "base saved");
    assert!(!b.is_modified());
    b.redo();
    assert_eq!(b.contents(), "base saved more");
    assert!(b.is_modified());
    std::fs::remove_file(path).unwrap();
}
