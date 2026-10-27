//! `/`, `?`, `n`, `N`, `*` — driven by typing them.


mod common;
use common::*;

/// The match ranges on each visible line.
fn hits(events: &[Event]) -> Option<Vec<Vec<(u32, u32)>>> {
    events.iter().rev().find_map(|e| match e {
        Event::Viewport { matches, .. } => Some(
            matches
                .iter()
                .map(|line| line.iter().map(|h| (h.start, h.end)).collect())
                .collect(),
        ),
        _ => None,
    })
}


fn workspace(text: &str) -> (Workspace, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "proton-search-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), text).unwrap();

    let mut ws = Workspace::rooted(&root);
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
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

/// Type a search and submit it.
fn search(ws: &mut Workspace, line: &str) -> Vec<Event> {
    keys(ws, line);
    ws.apply(Command::Key {
        key: Key::named("Enter"),
    })
}

fn at(ws: &Workspace) -> (usize, usize) {
    let buffer = ws.editor().buffer();
    let p = buffer.position(buffer.cursor.offset);
    (p.line, p.column)
}

fn said(events: &[Event], fragment: &str) -> bool {
    events.iter().any(|e| match e {
        Event::Notice { text, .. } => text.contains(fragment),
        _ => false,
    })
}

const SAMPLE: &str = "the first alpha\nsecond beta\nthird alpha gamma\nfourth Alpha\n";

#[test]
fn a_search_jumps_to_the_match() {
    let (mut ws, root) = workspace(SAMPLE);
    search(&mut ws, "/beta");
    assert_eq!(at(&ws), (1, 7));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn n_and_shift_n_walk_the_matches() {
    let (mut ws, root) = workspace(SAMPLE);
    search(&mut ws, "/alpha");
    assert_eq!(at(&ws), (0, 10), "the first match after the cursor");

    keys(&mut ws, "n");
    assert_eq!(at(&ws), (2, 6));

    keys(&mut ws, "n");
    assert_eq!(
        at(&ws),
        (3, 7),
        "smartcase: a lowercase pattern finds Alpha"
    );

    keys(&mut ws, "N");
    assert_eq!(at(&ws), (2, 6), "and back");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn searching_wraps_and_says_so() {
    // Silently wrapping looks exactly like the search having gone the wrong
    // way, which is how you end up editing the wrong function.
    let (mut ws, root) = workspace(SAMPLE);
    search(&mut ws, "/gamma");
    assert_eq!(at(&ws), (2, 12));

    let events = keys(&mut ws, "n");
    assert_eq!(at(&ws), (2, 12), "the only match, found again");
    assert!(said(&events, "continued from the top"), "{events:?}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn backwards_search_goes_backwards_and_n_keeps_going_that_way() {
    // `N` is the opposite of the direction the search was going, not of
    // "forwards" — after `?foo`, `n` still goes backwards.
    let (mut ws, root) = workspace(SAMPLE);
    keys(&mut ws, "G"); // last line
    search(&mut ws, "?alpha");
    assert_eq!(at(&ws), (3, 7), "the nearest match above");

    keys(&mut ws, "n");
    assert_eq!(at(&ws), (2, 6), "still going up");

    keys(&mut ws, "N");
    assert_eq!(at(&ws), (3, 7), "and N turns round");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn smartcase_is_vims() {
    // A lowercase pattern ignores case; a pattern with a capital in it does
    // not. That is what makes `/err` find `Error` and `/Error` not find `err`.
    let (mut ws, root) = workspace(SAMPLE);
    search(&mut ws, "/Alpha");
    assert_eq!(at(&ws), (3, 7), "only the capitalised one");

    let events = keys(&mut ws, "n");
    assert!(
        said(&events, "continued"),
        "it is the only match: {events:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn star_searches_the_word_under_the_cursor() {
    let (mut ws, root) = workspace(SAMPLE);
    // Land on `alpha` on the first line.
    keys(&mut ws, "0wwv");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    ws.apply(Command::PlaceCursor {
        line: 0,
        column: 10,
    });

    keys(&mut ws, "*");
    assert_eq!(at(&ws), (2, 6), "the next `alpha`");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_pattern_that_is_nowhere_says_so_and_stays_put() {
    let (mut ws, root) = workspace(SAMPLE);
    let before = at(&ws);
    let events = search(&mut ws, "/nowhere");
    assert!(said(&events, "not found"), "{events:?}");
    assert_eq!(at(&ws), before, "and did not move");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn n_before_any_search_says_so_rather_than_doing_nothing() {
    let (mut ws, root) = workspace(SAMPLE);
    let events = keys(&mut ws, "n");
    assert!(said(&events, "no previous search"), "{events:?}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_empty_search_repeats_the_last_one() {
    // Vim's rule, and what anybody who pressed `/` and Enter by accident
    // wants — rather than "matched nothing", which is not a useful answer.
    let (mut ws, root) = workspace(SAMPLE);
    search(&mut ws, "/beta");
    ws.apply(Command::PlaceCursor { line: 0, column: 0 });

    search(&mut ws, "/");
    assert_eq!(at(&ws), (1, 7), "found beta again");
    std::fs::remove_dir_all(root).ok();
}


#[test]
fn a_search_in_visual_mode_extends_the_selection() {
    // `v/foo` selects up to the match, the way an operator-pending search
    // does — the head moves and the anchor stays.
    let (mut ws, root) = workspace(SAMPLE);
    keys(&mut ws, "v");
    search(&mut ws, "/gamma");

    let cursor = ws.editor().buffer().cursor;
    assert!(cursor.has_selection(), "the drag left a selection");
    assert_eq!(cursor.anchor, 0, "anchored where visual mode started");
    std::fs::remove_dir_all(root).ok();
}

/* ── hlsearch and incremental search ──────────────────────────────── */


#[test]
fn every_match_on_screen_is_painted() {
    let (mut ws, root) = workspace(SAMPLE);
    let events = search(&mut ws, "/alpha");

    // Line 0 and line 2 have `alpha`; line 3 has `Alpha`, which a lowercase
    // pattern also matches.
    assert_eq!(
        hits(&events).unwrap(),
        vec![vec![(10, 15)], vec![], vec![(6, 11)], vec![(7, 12)], vec![],]
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn nothing_is_painted_before_a_search() {
    let (mut ws, root) = workspace(SAMPLE);
    let events = ws.apply(Command::Sync);
    assert!(hits(&events).unwrap().iter().all(Vec::is_empty));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn typing_paints_before_you_press_enter() {
    // The whole point of an incremental search: you see where you would land,
    // and everywhere else you could have landed, before committing.
    let (mut ws, root) = workspace(SAMPLE);
    ws.apply(Command::Key {
        key: Key::char('/'),
    });
    let events = keys(&mut ws, "alph");

    let painted: usize = hits(&events).unwrap().iter().map(Vec::len).sum();
    assert_eq!(painted, 3, "all three, mid-pattern");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn typing_moves_the_cursor_to_the_match() {
    let (mut ws, root) = workspace(SAMPLE);
    ws.apply(Command::Key {
        key: Key::char('/'),
    });
    keys(&mut ws, "gam");
    assert_eq!(at(&ws), (2, 12), "already there, before Enter");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn each_keystroke_searches_from_where_the_prompt_opened() {
    // Not from where the last preview landed — otherwise every character
    // jumps to the match after the one before, and the cursor walks away down
    // the file as you type.
    let (mut ws, root) = workspace("alpha\nalpha\nalpha\nalpha\n");
    ws.apply(Command::Key {
        key: Key::char('/'),
    });
    keys(&mut ws, "a");
    assert_eq!(at(&ws), (0, 4), "the next `a` after the cursor");
    keys(&mut ws, "l");
    assert_eq!(at(&ws), (1, 0), "the next `al` after the cursor");
    keys(&mut ws, "pha");
    assert_eq!(
        at(&ws),
        (1, 0),
        "still the first `alpha` after the cursor — searching from the \
         preview instead would have walked on to line 2"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn abandoning_a_search_puts_the_cursor_back() {
    let (mut ws, root) = workspace(SAMPLE);
    ws.apply(Command::PlaceCursor { line: 1, column: 2 });
    let before = at(&ws);

    ws.apply(Command::Key {
        key: Key::char('/'),
    });
    keys(&mut ws, "gamma");
    assert_ne!(at(&ws), before, "the preview moved it");

    let events = ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    assert_eq!(at(&ws), before, "and Escape put it back");
    assert!(
        hits(&events).unwrap().iter().all(Vec::is_empty),
        "and stopped painting"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn committing_lands_on_the_match_the_preview_showed() {
    // The preview has already moved the cursor, so searching from *there*
    // would land on the second match rather than the first.
    let (mut ws, root) = workspace("alpha\nalpha\nalpha\n");
    ws.apply(Command::Key {
        key: Key::char('/'),
    });
    keys(&mut ws, "alpha");
    let previewed = at(&ws);

    ws.apply(Command::Key {
        key: Key::named("Enter"),
    });
    assert_eq!(at(&ws), previewed, "where the preview said");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn noh_stops_painting_without_forgetting_the_pattern() {
    // The distinction people use `:noh` for: the highlight has served its
    // purpose, and `n` still has to work.
    let (mut ws, root) = workspace(SAMPLE);
    search(&mut ws, "/alpha");

    let events = run_ex(&mut ws, "noh");
    assert!(
        hits(&events).unwrap().iter().all(Vec::is_empty),
        "stopped painting: {events:?}"
    );

    let events = keys(&mut ws, "n");
    assert!(!said(&events, "no previous search"), "{events:?}");
    assert!(
        hits(&events).unwrap().iter().any(|line| !line.is_empty()),
        "and searching again paints again"
    );
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
