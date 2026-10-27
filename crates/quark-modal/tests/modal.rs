//! The M1 exit criterion in test form, as far as M0 reaches: counts multiply,
//! operators compose, doubled operators mean the line, and Escape always
//! unwinds exactly one level.


mod common;
use common::*;

/// Type a string of bare characters, returning the last outcome.
fn type_keys(vim: &mut Vim, keys: &str) -> Outcome {
    let mut last = Outcome::Ignored;
    for c in keys.chars() {
        last = vim.key(&Key::char(c));
    }
    last
}

fn action_of(outcome: Outcome) -> Action {
    match outcome {
        Outcome::Action(a) => a,
        other => panic!("expected an action, got {other:?}"),
    }
}

#[test]
fn dd_deletes_a_line() {
    let mut vim = Vim::new();
    let a = action_of(type_keys(&mut vim, "dd"));
    assert_eq!(a.verb, Verb::Delete);
    assert_eq!(a.object, Object::Line);
    assert_eq!(a.count, 1);
}

#[test]
fn counts_prefix_and_multiply() {
    let mut vim = Vim::new();
    assert_eq!(action_of(type_keys(&mut vim, "2dd")).count, 2);

    // vim multiplies the two counts: 2d3w is six words.
    let mut vim = Vim::new();
    let a = action_of(type_keys(&mut vim, "2d3w"));
    assert_eq!(a.count, 6);
    assert_eq!(a.object, Object::WordForward);
}

#[test]
fn zero_is_a_motion_until_a_count_is_started() {
    // Bare `0` is line-start.
    let mut vim = Vim::new();
    let a = action_of(type_keys(&mut vim, "0"));
    assert_eq!(a.verb, Verb::Move);
    assert_eq!(a.object, Object::LineStart);

    // After a digit, `0` is part of the count.
    let mut vim = Vim::new();
    assert_eq!(action_of(type_keys(&mut vim, "10dd")).count, 10);
}

#[test]
fn change_enters_insert_and_delete_does_not() {
    let mut vim = Vim::new();
    type_keys(&mut vim, "cw");
    assert_eq!(vim.mode(), VimMode::Insert);

    let mut vim = Vim::new();
    type_keys(&mut vim, "dw");
    assert_eq!(vim.mode(), VimMode::Normal);
}

#[test]
fn registers_attach_to_the_operator() {
    let mut vim = Vim::new();
    let a = action_of(type_keys(&mut vim, "\"ayy"));
    assert_eq!(a.verb, Verb::Yank);
    assert_eq!(a.object, Object::Line);
    assert_eq!(a.register.as_deref(), Some("a"));
}

#[test]
fn escape_unwinds_one_level_at_a_time() {
    let mut vim = Vim::new();

    // Pending operator → cancelled, still normal.
    type_keys(&mut vim, "2d");
    vim.key(&Key::named("Escape"));
    assert_eq!(vim.mode(), VimMode::Normal);
    // The cancel really cleared: a following `d` starts fresh, so `dd` is
    // count 1 rather than inheriting the 2.
    assert_eq!(action_of(type_keys(&mut vim, "dd")).count, 1);

    // Insert → normal.
    type_keys(&mut vim, "i");
    assert_eq!(vim.mode(), VimMode::Insert);
    vim.key(&Key::named("Escape"));
    assert_eq!(vim.mode(), VimMode::Normal);
}

#[test]
fn escape_is_never_read_as_a_character() {
    let mut vim = Vim::new();
    type_keys(&mut vim, "i");
    // `Escape` is multi-character, so `as_char` must refuse it — otherwise it
    // would be typed as an `E`.
    assert!(matches!(vim.key(&Key::named("Escape")), Outcome::Ignored));
    assert_eq!(vim.mode(), VimMode::Normal);
}

#[test]
fn insert_mode_yields_characters_not_commands() {
    let mut vim = Vim::new();
    type_keys(&mut vim, "i");
    assert!(matches!(vim.key(&Key::char('d')), Outcome::Input('d')));
    assert_eq!(vim.mode(), VimMode::Insert);
}

#[test]
fn leader_opens_and_collects() {
    let mut vim = Vim::new();
    assert!(matches!(vim.key(&Key::char(' ')), Outcome::Leader(p) if p.is_empty()));
    assert!(matches!(vim.key(&Key::char('f')), Outcome::Leader(p) if p == "f"));
    assert!(matches!(vim.key(&Key::char('f')), Outcome::Leader(p) if p == "ff"));
}

#[test]
fn cmdline_collects_and_executes() {
    let mut vim = Vim::new();
    vim.key(&Key::char(':'));
    assert_eq!(vim.mode(), VimMode::Command);
    type_keys(&mut vim, "wq");
    match vim.key(&Key::named("Enter")) {
        Outcome::Execute(line) => assert_eq!(line, "wq"),
        other => panic!("expected the command line to execute, got {other:?}"),
    }
    assert_eq!(vim.mode(), VimMode::Normal);
}

#[test]
fn visual_operator_applies_to_the_selection() {
    let mut vim = Vim::new();
    vim.key(&Key::char('v'));
    assert_eq!(vim.mode(), VimMode::Visual);
    let a = action_of(vim.key(&Key::char('d')));
    assert_eq!(a.object, Object::Selection);
    assert_eq!(vim.mode(), VimMode::Normal);
}

// ── the seam ────────────────────────────────────────────────────────────

#[test]
fn a_keystroke_round_trips_through_the_workspace() {
    // This is the M0 exit criterion, minus the pixels.
    let mut ws = Workspace::new();
    let events = ws.apply(Command::Key {
        key: Key::char('i'),
    });

    assert!(events.iter().any(|e| matches!(
        e,
        Event::VimMode {
            mode: VimMode::Insert,
            ..
        }
    )));
    assert_eq!(ws.vim_mode(), VimMode::Insert);
}




#[test]
fn typing_a_command_is_not_running_it() {
    // The whole reason `Execute` is a separate outcome: a consumer that cannot
    // tell these apart runs `:`, `:s`, `:so` and `:sor` on the way to `:sort`.
    //
    // `sort` because the core does not own it — one it did own would be run
    // here rather than passed out, and this is about the passing out.
    let mut ws = Workspace::new();
    ws.apply(Command::Key {
        key: Key::char(':'),
    });

    for c in "sort".chars() {
        let events = ws.apply(Command::Key { key: Key::char(c) });
        assert!(
            !events.iter().any(|e| matches!(e, Event::CmdlineRun { .. })),
            "typing {c:?} must not execute anything"
        );
    }

    let events = ws.apply(Command::Key {
        key: Key::named("Enter"),
    });
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::CmdlineRun { text } if text == "sort")),
        "Enter should run the collected line: {events:?}"
    );
}
