//! Splits, through the seam the shell uses.
//!
//! The tests that matter here are the ones that tell a *view* from a *copy*:
//! two panes on one file scroll apart and keep their own cursors, and an edit
//! in either shows in both.


mod common;
use common::*;

/// The layout: which buffer each split shows, and which has the focus.
fn layout(events: &[Event]) -> Option<(Vec<String>, usize, bool)> {
    events.iter().rev().find_map(|e| match e {
        Event::Splits {
            views,
            focused,
            vertical,
        } => Some((
            views.iter().map(|v| v.name.clone()).collect(),
            *focused,
            *vertical,
        )),
        _ => None,
    })
}

/// The lines one split is showing.
fn lines_of(events: &[Event], want: usize) -> Option<Vec<String>> {
    events.iter().rev().find_map(|e| match e {
        Event::Viewport { view, lines, .. } if *view == want => Some(lines.clone()),
        _ => None,
    })
}

fn cursor_of(events: &[Event], want: usize) -> Option<(usize, usize)> {
    events.iter().rev().find_map(|e| match e {
        Event::Cursor {
            view, line, column, ..
        } if *view == want => Some((*line, *column)),
        _ => None,
    })
}


fn workspace() -> (Workspace, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "proton-splits-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let long: String = (0..200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("long.txt"), long).unwrap();
    std::fs::write(root.join("other.txt"), "other one\nother two\n").unwrap();

    let mut ws = Workspace::rooted(&root);
    ws.apply(Command::Viewport {
        view: 0,
        first_line: 0,
        lines: 10,
    });
    ws.apply(Command::OpenBuffer {
        path: "long.txt".into(),
    });
    (ws, root)
}

fn keys(ws: &mut Workspace, text: &str) -> Vec<Event> {
    let mut last = Vec::new();
    for c in text.chars() {
        last = ws.apply(Command::Key { key: Key::char(c) });
    }
    last
}




fn said(events: &[Event], fragment: &str) -> bool {
    events.iter().any(|e| match e {
        Event::Notice { text, .. } => text.contains(fragment),
        _ => false,
    })
}

/* ── what a split is ──────────────────────────────────────────────── */

#[test]
fn splitting_gives_you_two_of_what_you_were_looking_at() {
    let (mut ws, root) = workspace();
    let events = ws.apply(Command::Split { vertical: true });

    let (names, focused, vertical) = layout(&events).expect("a layout");
    assert_eq!(names, vec!["long.txt", "long.txt"]);
    assert_eq!(focused, 1, "the new one takes the focus");
    assert!(vertical);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn two_splits_on_one_file_scroll_apart() {
    // The whole reason a split is a view and not a copy.
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });

    ws.apply(Command::Viewport {
        view: 1,
        first_line: 100,
        lines: 10,
    });
    let events = ws.apply(Command::Sync);

    assert_eq!(lines_of(&events, 0).unwrap()[0], "line 0");
    assert_eq!(lines_of(&events, 1).unwrap()[0], "line 100");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn two_splits_on_one_file_keep_their_own_cursors() {
    let (mut ws, root) = workspace();
    keys(&mut ws, "jjj"); // down three in the first split
    let before = ws.editor().buffer().cursor.offset;

    ws.apply(Command::Split { vertical: true });
    keys(&mut ws, "jjjjj"); // and five more in the second

    let moved = ws.editor().buffer().cursor.offset;
    assert!(moved > before, "the new split's cursor moved");

    // Back to the first: its cursor is where it was left.
    let events = ws.apply(Command::FocusSplit { index: 0 });
    assert_eq!(
        ws.editor().buffer().cursor.offset,
        before,
        "the first split's cursor stayed where it was"
    );
    assert_eq!(
        cursor_of(&events, 1).unwrap().0,
        8,
        "and the other's did too"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_edit_in_one_split_shows_in_the_other() {
    // Two views of one buffer disagreeing about what is in it, even for a
    // frame, is the bug that makes a split feel like a copy.
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });

    let events = keys(&mut ws, "Ixx");
    assert!(
        lines_of(&events, 0).unwrap()[0].starts_with("xx"),
        "the other split was redrawn: {:?}",
        lines_of(&events, 0)
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn splits_can_show_different_files() {
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::OpenBuffer {
        path: "other.txt".into(),
    });

    let events = ws.apply(Command::Sync);
    let (names, _, _) = layout(&events).unwrap();
    assert_eq!(names, vec!["long.txt", "other.txt"]);
    assert_eq!(lines_of(&events, 0).unwrap()[0], "line 0");
    assert_eq!(lines_of(&events, 1).unwrap()[0], "other one");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn typing_goes_to_the_focused_split_only() {
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::OpenBuffer {
        path: "other.txt".into(),
    });

    keys(&mut ws, "Izz");
    let events = ws.apply(Command::Sync);
    assert!(lines_of(&events, 1).unwrap()[0].starts_with("zz"));
    assert_eq!(
        lines_of(&events, 0).unwrap()[0],
        "line 0",
        "the unfocused split's file is untouched"
    );
    std::fs::remove_dir_all(root).ok();
}

/* ── moving between them ──────────────────────────────────────────── */

#[test]
fn the_window_keys_move_the_focus() {
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::Split { vertical: true });
    assert_eq!(layout(&ws.apply(Command::Sync)).unwrap().1, 2);

    // `⌃w h` — vim's window prefix.
    ws.apply(Command::Key {
        key: Key::ctrl('w'),
    });
    let events = ws.apply(Command::Key {
        key: Key::char('h'),
    });
    assert_eq!(layout(&events).unwrap().1, 1, "moved left: {events:?}");

    // And under the leader, as the rail has always shown it.
    let events = keys(&mut ws, " wh");
    assert_eq!(layout(&events).unwrap().1, 0);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn walking_into_the_edge_is_not_an_error() {
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::FocusSplit { index: 0 });

    let events = ws.apply(Command::StepSplit {
        direction: Direction::Left,
    });
    assert!(events.is_empty(), "nothing happened, and nothing was said");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_row_has_no_up_and_down() {
    // Vim does the same in the same layout: half the directions are along the
    // row and half are across it, and across it there is nothing.
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });

    assert!(ws
        .apply(Command::StepSplit {
            direction: Direction::Up
        })
        .is_empty());

    // Stacked, and now up works and left does not.
    ws.apply(Command::Split { vertical: false });
    assert!(!ws
        .apply(Command::StepSplit {
            direction: Direction::Up
        })
        .is_empty());
    std::fs::remove_dir_all(root).ok();
}

/* ── closing ──────────────────────────────────────────────────────── */

#[test]
fn quit_closes_the_split_while_there_are_several() {
    // Vim's `:q` closes a window. With one split left it closes the buffer
    // instead — an editor whose `:q` sometimes shuts the whole window is one
    // you lose work in.
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });

    let events = run_ex(&mut ws, "q");
    assert_eq!(layout(&events).unwrap().0.len(), 1, "the split closed");
    assert!(!said(&events, "closed long.txt"), "the buffer did not");

    let events = run_ex(&mut ws, "q");
    assert!(said(&events, "closed long.txt"), "and now it does");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn bd_always_means_the_buffer_even_with_splits_open() {
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });

    let events = run_ex(&mut ws, "bd");
    assert!(said(&events, "closed long.txt"), "{events:?}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn only_keeps_the_one_you_are_in() {
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::OpenBuffer {
        path: "other.txt".into(),
    });
    ws.apply(Command::Split { vertical: true });

    let events = run_ex(&mut ws, "only");
    let (names, focused, _) = layout(&events).unwrap();
    assert_eq!(names, vec!["other.txt"]);
    assert_eq!(focused, 0);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn closing_a_buffer_does_not_leave_a_split_pointing_past_the_end() {
    // Every index above the closed one shifts down, and a split still holding
    // the old number would draw the wrong file — or panic reaching for one
    // that is no longer there.
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::OpenBuffer {
        path: "other.txt".into(),
    });

    // Close `other.txt`, which is the highest index.
    run_ex(&mut ws, "bd");
    let events = ws.apply(Command::Sync);
    let (names, _, _) = layout(&events).unwrap();
    assert!(
        names.iter().all(|n| !n.is_empty()),
        "every split is showing something real: {names:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn there_is_a_limit_and_it_says_so() {
    let (mut ws, root) = workspace();
    for _ in 1..4 {
        ws.apply(Command::Split { vertical: true });
    }
    let events = ws.apply(Command::Split { vertical: true });
    assert!(said(&events, "four splits"), "{events:?}");
    assert_eq!(layout(&ws.apply(Command::Sync)).unwrap().0.len(), 4);
    std::fs::remove_dir_all(root).ok();
}

/// Type an ex command and submit it.
fn run_ex(ws: &mut Workspace, line: &str) -> Vec<Event> {
    ws.apply(Command::Key {
        key: Key::char(':'),
    });
    keys(ws, line);
    ws.apply(Command::Key {
        key: Key::named("Enter"),
    })
}

#[test]
fn a_file_opens_in_the_pane_you_are_in() {
    // Three panes, the middle one focused. Opening a file has to land there
    // and not in whichever pane happens to be first.
    let (mut ws, root) = workspace();
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::Split { vertical: true });
    ws.apply(Command::FocusSplit { index: 1 });
    assert_eq!(layout(&ws.apply(Command::Sync)).unwrap().1, 1);

    ws.apply(Command::OpenBuffer {
        path: "other.txt".into(),
    });

    let events = ws.apply(Command::Sync);
    let (names, focused, _) = layout(&events).unwrap();
    assert_eq!(focused, 1, "the focus should not have moved");
    assert_eq!(
        names,
        vec!["long.txt", "other.txt", "long.txt"],
        "the middle pane is the one that changed"
    );
    std::fs::remove_dir_all(root).ok();
}
