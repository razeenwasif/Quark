//! Visual block — the selection that is a rectangle rather than a run.
//!
//! Everything else an editor selects is a range of offsets. A block is not,
//! and the text it covers is not contiguous, so this is the one feature that
//! could not reuse the operator machinery. These tests are mostly about the
//! ways a rectangle stops being one: a short line in the middle, a line the
//! block reaches past, an undo that takes back half of it.


mod common;
use common::*;

/// `fixtures/block.txt`:
///
/// ```text
/// one   alpha
/// two   beta
/// three gamma
/// x
/// five  epsilon
/// ```
///
/// The short line is there on purpose — it is where every naive
/// implementation of this falls over.
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

fn keys(ws: &mut Workspace, s: &str) {
    for c in s.chars() {
        ws.apply(Command::Key { key: Key::char(c) });
    }
}

fn ctrl(ws: &mut Workspace, key: &str) {
    ws.apply(Command::Key {
        key: Key {
            key: key.into(),
            ctrl: true,
            alt: false,
            shift: false,
            meta: false,
        },
    });
}

fn escape(ws: &mut Workspace) {
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
}

fn text(ws: &Workspace) -> String {
    ws.editor().buffer().contents()
}

#[test]
fn control_v_enters_and_leaves_block_mode() {
    let mut ws = over("block.txt");
    ctrl(&mut ws, "v");
    assert_eq!(ws.vim_mode(), VimMode::VisualBlock);
    assert_eq!(ws.vim_mode().label(), "V-BLOCK");

    ctrl(&mut ws, "v");
    assert_eq!(ws.vim_mode(), VimMode::Normal, "it toggles, like v and V");

    ctrl(&mut ws, "v");
    escape(&mut ws);
    assert_eq!(ws.vim_mode(), VimMode::Normal);
}

#[test]
fn deleting_a_block_takes_a_rectangle_not_a_run() {
    // The difference that makes the mode worth having: the text between the
    // columns on different lines is not contiguous, and a range-based delete
    // would take the line endings with it.
    let mut ws = over("block.txt");
    ctrl(&mut ws, "v");
    keys(&mut ws, "jjll"); // three lines tall, three columns wide

    keys(&mut ws, "d");
    assert_eq!(
        text(&ws),
        "   alpha\n   beta\nee gamma\nx\nfive  epsilon\n",
        "each line should lose its own three characters and keep its newline"
    );
    assert_eq!(ws.vim_mode(), VimMode::Normal);
}

#[test]
fn a_line_too_short_to_reach_the_block_is_left_alone() {
    // The case that breaks naive implementations: clamping a short line into
    // the block's columns either panics or eats the wrong characters.
    let mut ws = over("block.txt");
    keys(&mut ws, "jj"); // `three gamma`
    keys(&mut ws, "llllll"); // column 6, the `g`
    ctrl(&mut ws, "v");
    keys(&mut ws, "jjl"); // down across `x`, which has no column 6

    keys(&mut ws, "d");
    assert_eq!(
        text(&ws),
        "one   alpha\ntwo   beta\nthree mma\nx\nfive  silon\n",
        "the short line should survive untouched"
    );
}

#[test]
fn capital_i_inserts_on_every_line_of_the_block() {
    // The reason anyone uses this mode.
    let mut ws = over("block.txt");
    ctrl(&mut ws, "v");
    keys(&mut ws, "jj");
    keys(&mut ws, "I// ");
    escape(&mut ws);

    assert_eq!(
        text(&ws),
        "// one   alpha\n// two   beta\n// three gamma\nx\nfive  epsilon\n"
    );
}

#[test]
fn a_block_insert_is_one_undo_not_one_per_line() {
    // Three lines changed, but the user did one thing.
    let mut ws = over("block.txt");
    let before = text(&ws);

    ctrl(&mut ws, "v");
    keys(&mut ws, "jj");
    keys(&mut ws, "I// ");
    escape(&mut ws);
    assert_ne!(text(&ws), before);

    keys(&mut ws, "u");
    assert_eq!(text(&ws), before, "one undo should take back the rectangle");
}

#[test]
fn capital_a_appends_and_pads_lines_that_are_too_short() {
    // `A` reaches past the end of a short line by padding it out, so the
    // result is a column. `I` deliberately does not — see the test below.
    let mut ws = over("block.txt");
    keys(&mut ws, "jj"); // `three gamma`
    ctrl(&mut ws, "v");
    keys(&mut ws, "j"); // down onto `x`
    keys(&mut ws, "A!");
    escape(&mut ws);

    let after = text(&ws);
    let lines: Vec<&str> = after.lines().collect();
    assert_eq!(lines[2], "!three gamma");
    assert_eq!(lines[3], "!x");
}

#[test]
fn yanking_a_block_and_putting_it_back_gives_a_block() {
    let mut ws = over("block.txt");
    keys(&mut ws, "llllll"); // column 6 — the start of the second word
    ctrl(&mut ws, "v");
    keys(&mut ws, "jjll"); // a 3x3 rectangle: alp / bet / gam
    keys(&mut ws, "y");

    // Put it at the start of the last line. `jjjj` rather than `G`, which
    // lands on the empty line after the trailing newline.
    keys(&mut ws, "jjjj0");
    keys(&mut ws, "P");

    let after = text(&ws);
    let lines: Vec<&str> = after.lines().collect();
    assert_eq!(
        lines[4], "alpfive  epsilon",
        "the first row lands where the cursor is"
    );
    assert_eq!(lines[5], "bet", "the rest go on the lines below it");
    assert_eq!(lines[6], "gam", "in the same column, making a column again");
}

#[test]
fn moving_in_block_mode_grows_the_block_rather_than_collapsing_it() {
    // A motion in visual mode extends the selection. Getting this wrong makes
    // the mode look like it works and then delete a single character.
    let mut ws = over("block.txt");
    ctrl(&mut ws, "v");
    keys(&mut ws, "jjl");
    assert!(
        ws.editor().buffer().cursor.has_selection(),
        "the anchor should still be up at the top-left"
    );

    keys(&mut ws, "d");
    let lines: Vec<String> = text(&ws).lines().map(str::to_string).collect();
    assert_eq!(lines[0], "e   alpha", "two columns off three lines");
    assert_eq!(lines[1], "o   beta");
    assert_eq!(lines[2], "ree gamma");
}

#[test]
fn a_block_dragged_up_is_the_same_rectangle_as_one_dragged_down() {
    // Both corners are normalised, so which one is the anchor cannot matter.
    // Asserted as an equivalence rather than against a hand-written string,
    // because the interesting claim is that the two agree — not what either
    // one happens to produce.
    let mut downward = over("block.txt");
    ctrl(&mut downward, "v");
    keys(&mut downward, "jjll");
    keys(&mut downward, "d");

    let mut upward = over("block.txt");
    keys(&mut upward, "jjll"); // start at the far corner
    ctrl(&mut upward, "v");
    keys(&mut upward, "kkhh"); // and drag back to where the other one began
    keys(&mut upward, "d");

    assert_eq!(text(&upward), text(&downward));
    assert_eq!(
        text(&downward),
        "   alpha\n   beta\nee gamma\nx\nfive  epsilon\n",
        "and both should be the rectangle, not a run"
    );
}
