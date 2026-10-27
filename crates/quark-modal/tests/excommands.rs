//! The `:` line — `:w` and `:e` and the family around them.
//!
//! Typed rather than dispatched: every test presses `:`, types the command and
//! presses Enter, because the interesting failures are in the seam between the
//! modal engine and the command, not in either alone.

mod common;
use common::*;

use std::path::PathBuf;


struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!(
            "proton-ex-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch { root }
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap()
    }

    fn workspace(&self) -> Workspace {
        Workspace::rooted(&self.root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Type a `:` line and press Enter, exactly as a user does.
fn run(ws: &mut Workspace, line: &str) -> Vec<Event> {
    ws.apply(Command::Key {
        key: Key::char(':'),
    });
    for c in line.chars() {
        ws.apply(Command::Key { key: Key::char(c) });
    }
    ws.apply(Command::Key {
        key: Key::named("Enter"),
    })
}

fn keys(ws: &mut Workspace, text: &str) {
    for c in text.chars() {
        ws.apply(Command::Key { key: Key::char(c) });
    }
}

fn notices(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn said(events: &[Event], fragment: &str) -> bool {
    notices(events).iter().any(|n| n.contains(fragment))
}

fn open_names(ws: &Workspace) -> Vec<String> {
    ws.editor().buffers().iter().map(|b| b.name()).collect()
}

/// What the front end was left to deal with, if anything.
fn passed_through(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::CmdlineRun { text } => Some(text.clone()),
        _ => None,
    })
}

/* ── :w ───────────────────────────────────────────────────────────── */

#[test]
fn write_puts_the_buffer_on_disk() {
    let scratch = Scratch::new("write");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });

    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    assert!(ws.is_modified(), "typing made it dirty");

    let events = run(&mut ws, "w");
    assert!(said(&events, "wrote a.txt"), "{events:?}");
    assert_eq!(scratch.read("a.txt"), "xxone\n");
    assert!(!ws.is_modified(), "and clean again");
}

#[test]
fn write_abbreviates_the_way_vim_does() {
    let scratch = Scratch::new("abbrev");
    scratch.file("a.txt", "one\n");

    for spelling in ["w", "wr", "wri", "writ", "write"] {
        let mut ws = scratch.workspace();
        ws.apply(Command::OpenBuffer {
            path: "a.txt".into(),
        });
        let events = run(&mut ws, spelling);
        assert!(said(&events, "wrote"), "{spelling}: {events:?}");
    }
}

#[test]
fn write_with_a_name_is_a_copy_and_keeps_the_buffer_where_it_was() {
    // Vim's rule, and the surprising half of it: `:w other` writes a copy and
    // leaves you editing the original, still modified. `:saveas` is the one
    // that moves house.
    let scratch = Scratch::new("copy");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    let events = run(&mut ws, "w b.txt");
    assert!(said(&events, "copy"), "and says so: {events:?}");
    assert_eq!(scratch.read("b.txt"), "xxone\n");
    assert_eq!(scratch.read("a.txt"), "one\n", "the original is untouched");
    assert!(ws.editor().buffer().name().ends_with("a.txt"));
    assert!(ws.is_modified(), "still unsaved, because it is");
}

#[test]
fn write_with_a_name_adopts_it_when_the_buffer_had_none() {
    // The other half of vim's rule. Writing a nameless buffer to a file and
    // then having no way to keep editing that file would be absurd.
    let scratch = Scratch::new("adopt");
    let mut ws = scratch.workspace();
    keys(&mut ws, "ihello");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    assert_eq!(ws.editor().buffer().name(), "[No Name]");

    let events = run(&mut ws, "w notes.txt");
    assert!(said(&events, "wrote notes.txt"), "{events:?}");
    assert_eq!(scratch.read("notes.txt"), "hello");
    assert_eq!(ws.editor().buffer().name(), "notes.txt");
    assert!(!ws.is_modified());
}

#[test]
fn writing_a_nameless_buffer_with_no_name_to_give_it_says_so() {
    let scratch = Scratch::new("noname");
    let mut ws = scratch.workspace();
    keys(&mut ws, "ihello");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    let events = run(&mut ws, "w");
    assert!(said(&events, "no path"), "{events:?}");
}

#[test]
fn a_write_refuses_when_the_file_changed_underneath_it() {
    // Formatters, language servers and `git checkout` rewrite files under an
    // open editor constantly. Silently discarding that is the one data-loss
    // bug an editor must not have.
    let scratch = Scratch::new("clobber");
    scratch.file("a.txt", "mine\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    // Something else gets there first.
    touch_later(&scratch.root.join("a.txt"), "theirs\n");

    let events = run(&mut ws, "w");
    assert!(said(&events, "changed on disk"), "{events:?}");
    assert_eq!(scratch.read("a.txt"), "theirs\n", "not overwritten");

    // And the bang is what it is for.
    let events = run(&mut ws, "w!");
    assert!(said(&events, "wrote"), "{events:?}");
    assert_eq!(scratch.read("a.txt"), "xxmine\n");
}

#[test]
fn the_save_command_goes_through_the_same_guard() {
    // Whichever of the two the front end sends, the answer has to be the same
    // — a guard with a way around it is not a guard.
    let scratch = Scratch::new("guard");
    scratch.file("a.txt", "mine\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    touch_later(&scratch.root.join("a.txt"), "theirs\n");

    let events = ws.apply(Command::Save);
    assert!(said(&events, "changed on disk"), "{events:?}");
    assert_eq!(scratch.read("a.txt"), "theirs\n");
}

/* ── :e ───────────────────────────────────────────────────────────── */

#[test]
fn edit_with_a_name_opens_that_file() {
    let scratch = Scratch::new("edit");
    scratch.file("a.txt", "one\n");
    scratch.file("deep/b.txt", "two\n");
    let mut ws = scratch.workspace();

    run(&mut ws, "e a.txt");
    assert_eq!(ws.editor().buffer().name(), "a.txt");

    // Relative to the project root, not to the process's working directory.
    run(&mut ws, "e deep/b.txt");
    assert_eq!(ws.editor().buffer().name(), "b.txt");
    assert_eq!(ws.editor().buffer().contents(), "two\n");
}

#[test]
fn edit_re_reads_the_file_when_given_no_name() {
    let scratch = Scratch::new("reread");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });

    std::fs::write(scratch.root.join("a.txt"), "rewritten\n").unwrap();
    let events = run(&mut ws, "e");
    assert!(said(&events, "re-read a.txt"), "{events:?}");
    assert_eq!(ws.editor().buffer().contents(), "rewritten\n");
}

#[test]
fn re_reading_over_unsaved_work_needs_the_bang() {
    let scratch = Scratch::new("discard");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    std::fs::write(scratch.root.join("a.txt"), "rewritten\n").unwrap();

    let events = run(&mut ws, "e");
    assert!(said(&events, "unsaved changes"), "{events:?}");
    assert_eq!(ws.editor().buffer().contents(), "xxone\n", "kept");

    let events = run(&mut ws, "e!");
    assert!(said(&events, "re-read"), "{events:?}");
    assert_eq!(ws.editor().buffer().contents(), "rewritten\n");
}

#[test]
fn a_re_read_is_undoable() {
    // It goes through the ordinary edit path rather than rebuilding the
    // buffer, so `u` gets your work back after an `:e!` you regret. A rebuilt
    // buffer could not offer that.
    let scratch = Scratch::new("undo");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    std::fs::write(scratch.root.join("a.txt"), "rewritten\n").unwrap();

    run(&mut ws, "e!");
    assert_eq!(ws.editor().buffer().contents(), "rewritten\n");

    keys(&mut ws, "u");
    assert_eq!(
        ws.editor().buffer().contents(),
        "xxone\n",
        "the work that :e! discarded is still in the undo tree"
    );
}

#[test]
fn editing_a_folder_points_at_the_thing_that_opens_folders() {
    // Vim would open a directory listing. Quark opens folders as projects,
    // which is a different verb living somewhere else.
    let scratch = Scratch::new("folder");
    scratch.file("deep/b.txt", "two\n");
    let mut ws = scratch.workspace();

    let events = run(&mut ws, "e deep");
    assert!(said(&events, "is a folder"), "{events:?}");
    assert!(
        said(&events, "Open LaTeX Project"),
        "and says what does open one: {events:?}"
    );
}

#[test]
fn re_reading_a_buffer_that_was_never_a_file_says_so() {
    let scratch = Scratch::new("nofile");
    let mut ws = scratch.workspace();
    let events = run(&mut ws, "e");
    assert!(said(&events, "no file to re-read"), "{events:?}");
}

/* ── :saveas ──────────────────────────────────────────────────────── */

#[test]
fn saveas_writes_it_there_and_keeps_editing_it_there() {
    let scratch = Scratch::new("saveas");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    run(&mut ws, "saveas b.txt");
    assert_eq!(scratch.read("b.txt"), "xxone\n");
    assert_eq!(scratch.read("a.txt"), "one\n", "the original stays behind");
    assert_eq!(
        ws.editor().buffer().name(),
        "b.txt",
        "you are in the new one"
    );
    assert!(!ws.is_modified());
}

#[test]
fn saveas_will_not_answer_to_s_because_that_is_substitute() {
    let scratch = Scratch::new("sav");
    let mut ws = scratch.workspace();
    // `:s` and `:sa` are not saveas; they go out to whoever else might know.
    assert_eq!(passed_through(&run(&mut ws, "s")).as_deref(), Some("s"));
    assert_eq!(passed_through(&run(&mut ws, "sa")).as_deref(), Some("sa"));
}

/* ── :q ───────────────────────────────────────────────────────────── */

#[test]
fn quit_closes_the_buffer() {
    let scratch = Scratch::new("quit");
    scratch.file("a.txt", "one\n");
    scratch.file("b.txt", "two\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    ws.apply(Command::OpenBuffer {
        path: "b.txt".into(),
    });
    assert_eq!(open_names(&ws), vec!["[No Name]", "a.txt", "b.txt"]);

    let events = run(&mut ws, "q");
    assert!(said(&events, "closed b.txt"), "{events:?}");
    assert_eq!(open_names(&ws), vec!["[No Name]", "a.txt"]);
    assert_eq!(
        ws.editor().buffer().name(),
        "a.txt",
        "you land on the neighbour to the left"
    );
}

#[test]
fn quitting_unsaved_work_needs_the_bang() {
    let scratch = Scratch::new("quitdirty");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    let events = run(&mut ws, "q");
    assert!(said(&events, "unsaved changes"), "{events:?}");
    assert!(said(&events, ":wq"), "and says how to keep it: {events:?}");
    assert_eq!(open_names(&ws), vec!["[No Name]", "a.txt"], "still open");

    let events = run(&mut ws, "q!");
    assert!(said(&events, "closed a.txt"), "{events:?}");
    assert_eq!(open_names(&ws), vec!["[No Name]"]);
    assert_eq!(scratch.read("a.txt"), "one\n", "and the file is untouched");
}

#[test]
fn closing_the_last_buffer_leaves_an_empty_one() {
    // `buffer()` indexes the list and every caller assumes it succeeds, so
    // there is always one. An editor showing no editor is not a state anybody
    // asked for either.
    let scratch = Scratch::new("lastbuffer");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });

    run(&mut ws, "q");
    run(&mut ws, "q");
    assert_eq!(open_names(&ws), vec!["[No Name]"]);
    assert_eq!(ws.editor().buffer().contents(), "");
}

#[test]
fn bd_is_the_same_thing_under_the_name_vim_gives_it() {
    let scratch = Scratch::new("bdelete");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });

    let events = run(&mut ws, "bd");
    assert!(said(&events, "closed a.txt"), "{events:?}");

    // `:b` alone is not it — that is `:buffer`, which switches rather than
    // closes, and guessing between the two would be unforgivable.
    let mut ws = scratch.workspace();
    assert_eq!(passed_through(&run(&mut ws, "b")).as_deref(), Some("b"));
}

#[test]
fn a_tab_can_be_closed_without_being_the_one_you_are_in() {
    // A click on a `×` names the tab it is on, which need not be the active
    // one — and must not leave you sitting in some third buffer afterwards.
    let scratch = Scratch::new("closeother");
    scratch.file("a.txt", "one\n");
    scratch.file("b.txt", "two\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    ws.apply(Command::OpenBuffer {
        path: "b.txt".into(),
    });

    ws.apply(Command::CloseBuffer {
        index: 1,
        force: false,
    });
    assert_eq!(open_names(&ws), vec!["[No Name]", "b.txt"]);
    assert_eq!(
        ws.editor().buffer().name(),
        "b.txt",
        "still in the one you were in"
    );
}

#[test]
fn a_click_that_would_lose_work_is_refused_and_puts_you_back() {
    // Never forced from a click: losing unsaved work must cost a deliberate
    // `:q!`, not a mis-aimed pointer. And the refusal must not strand you in
    // the buffer the click only pointed at.
    let scratch = Scratch::new("clickrefuse");
    scratch.file("a.txt", "one\n");
    scratch.file("b.txt", "two\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    ws.apply(Command::OpenBuffer {
        path: "b.txt".into(),
    });
    assert_eq!(ws.editor().buffer().name(), "b.txt");

    let events = ws.apply(Command::CloseBuffer {
        index: 1,
        force: false,
    });
    assert!(said(&events, "unsaved changes"), "{events:?}");
    assert_eq!(open_names(&ws), vec!["[No Name]", "a.txt", "b.txt"]);
    assert_eq!(
        ws.editor().buffer().name(),
        "b.txt",
        "put back where you were"
    );
}

/* ── :bn and :bp ──────────────────────────────────────────────────── */

#[test]
fn bn_and_bp_walk_the_buffers_and_wrap() {
    let scratch = Scratch::new("bnext");
    scratch.file("a.txt", "one\n");
    scratch.file("b.txt", "two\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    ws.apply(Command::OpenBuffer {
        path: "b.txt".into(),
    });
    // [No Name], a.txt, b.txt — sitting on the last one.
    assert_eq!(ws.editor().buffer().name(), "b.txt");

    run(&mut ws, "bn");
    assert_eq!(
        ws.editor().buffer().name(),
        "[No Name]",
        "off the end and round"
    );
    run(&mut ws, "bn");
    assert_eq!(ws.editor().buffer().name(), "a.txt");
    run(&mut ws, "bp");
    assert_eq!(ws.editor().buffer().name(), "[No Name]");
    run(&mut ws, "bp");
    assert_eq!(
        ws.editor().buffer().name(),
        "b.txt",
        "and back the other way"
    );
}

#[test]
fn bn_abbreviates_and_does_not_collide_with_bdelete() {
    // `:bn` and `:bd` share a first letter and nothing else. Getting the
    // precedence wrong would make "next buffer" close one.
    let scratch = Scratch::new("bncollide");
    scratch.file("a.txt", "one\n");

    for spelling in ["bn", "bne", "bnex", "bnext"] {
        let mut ws = scratch.workspace();
        ws.apply(Command::OpenBuffer {
            path: "a.txt".into(),
        });
        run(&mut ws, spelling);
        assert_eq!(
            open_names(&ws).len(),
            2,
            "{spelling} must not have closed anything"
        );
        assert_eq!(ws.editor().buffer().name(), "[No Name]", "{spelling}");
    }

    for spelling in ["bp", "bpr", "bprev", "bprevious", "bN"] {
        let mut ws = scratch.workspace();
        ws.apply(Command::OpenBuffer {
            path: "a.txt".into(),
        });
        run(&mut ws, spelling);
        assert_eq!(open_names(&ws).len(), 2, "{spelling}");
        assert_eq!(ws.editor().buffer().name(), "[No Name]", "{spelling}");
    }
}

#[test]
fn stepping_with_one_buffer_says_so_rather_than_doing_nothing() {
    // Silently doing nothing is the failure users cannot debug.
    let scratch = Scratch::new("bnone");
    let mut ws = scratch.workspace();
    let events = run(&mut ws, "bn");
    assert!(said(&events, "only one buffer"), "{events:?}");
}

/* ── compounds, and what the front end still owns ─────────────────── */

#[test]
fn wq_writes_and_then_closes() {
    let scratch = Scratch::new("wq");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    let events = run(&mut ws, "wq");
    assert!(said(&events, "wrote"), "{events:?}");
    assert!(said(&events, "closed"), "{events:?}");
    assert_eq!(scratch.read("a.txt"), "xxone\n");
    assert_eq!(open_names(&ws), vec!["[No Name]"]);
}

#[test]
fn wq_does_not_close_when_the_write_was_refused() {
    // Taking the buffer with it on the way to losing the work is the worst
    // possible reading of "write and quit".
    let scratch = Scratch::new("wqrefuse");
    scratch.file("a.txt", "mine\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });
    touch_later(&scratch.root.join("a.txt"), "theirs\n");

    let events = run(&mut ws, "wq");
    assert!(said(&events, "changed on disk"), "{events:?}");
    assert!(!said(&events, "closed"), "it did not close: {events:?}");
    assert_eq!(open_names(&ws), vec!["[No Name]", "a.txt"]);
}

#[test]
fn x_writes_only_when_there_is_something_to_write() {
    // The whole difference from `:wq`: an unmodified `:x` leaves the mtime
    // alone, and whatever is watching the file stays quiet.
    let scratch = Scratch::new("xit");
    let path = scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });

    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let events = run(&mut ws, "x");
    assert!(!said(&events, "wrote"), "nothing to write: {events:?}");
    assert!(
        said(&events, "closed a.txt"),
        "but it still closes: {events:?}"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before,
        "the file was not touched"
    );
}

#[test]
fn a_command_the_core_does_not_know_still_reaches_the_front_end() {
    // Whatever is left after the core has taken what it owns. Swallowing an
    // unknown command silently would leave the user with no way to tell a
    // typo from a missing feature.
    let scratch = Scratch::new("passthrough");
    let mut ws = scratch.workspace();

    for line in ["1,5d", "%s/a/b/", "sort", "make"] {
        assert_eq!(
            passed_through(&run(&mut ws, line)).as_deref(),
            Some(line),
            "{line} should have been passed through"
        );
    }
}

#[test]
fn escape_abandons_the_line_without_running_it() {
    let scratch = Scratch::new("escape");
    scratch.file("a.txt", "one\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.txt".into(),
    });
    keys(&mut ws, "Ixx");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    ws.apply(Command::Key {
        key: Key::char(':'),
    });
    keys(&mut ws, "w");
    ws.apply(Command::Key {
        key: Key::named("Escape"),
    });

    assert_eq!(scratch.read("a.txt"), "one\n", "nothing was written");
    assert!(ws.is_modified());
}

#[test]
fn a_save_with_no_formatter_writes_and_says_nothing_about_it() {
    // Formatting on save is on, but most languages have no formatter behind
    // their server and these tests start no servers at all. A save that
    // complained about that every time would be worse than a quiet one — and
    // the file still has to reach the disk, which is the whole job.
    let scratch = Scratch::new("fmt");
    scratch.file("a.rs", "fn  a(){}\n");
    let mut ws = scratch.workspace();
    ws.apply(Command::OpenBuffer {
        path: "a.rs".into(),
    });

    let events = run(&mut ws, "w");
    assert!(
        notices(&events).iter().any(|n| n.contains("wrote")),
        "`:w` should write: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::Notice {
                level: NoticeLevel::Warn,
                ..
            }
        )),
        "and should not mention a formatter nobody asked for: {events:?}"
    );
    assert_eq!(scratch.read("a.rs"), "fn  a(){}\n", "written as it was");
    assert!(!ws.is_modified());
}

/// Rewrite a file and make sure its mtime actually differs.
///
/// Windows timestamps are coarse enough that writing twice in the same
/// millisecond can leave them equal, and the test would then be asserting on
/// the clock rather than on the code.
fn touch_later(path: &std::path::Path, text: &str) {
    let before = std::fs::metadata(path).unwrap().modified().unwrap();
    for _ in 0..200 {
        std::fs::write(path, text).unwrap();
        if std::fs::metadata(path).unwrap().modified().unwrap() != before {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the filesystem never reported a new mtime for {path:?}");
}
