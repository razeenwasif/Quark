//! Pictures of the docs editor, drawn off screen: run on purpose, with
//! `QUARK_SHOTS=<folder> cargo test -p quark-app docs::shots -- --ignored`.

use quark_doc::{Author, Comment, Note, Pos, Selection, markdown};
use quark_ui::theme::{self, Palette, ThemeMode};

use super::claude::{ChatTurn, Status};
use super::view::{self, Around};
use super::{DockTab, DocTab};
use crate::shot::{folder, shoot};

const PLAN: &str = "# Colour confinement — seminar plan\n\n*Thursday 5 November · Physics reading group*\n\n## Goals\n\nGet the room from **asymptotic freedom** to *why nobody has ever seen a free quark* in under fifty minutes, with one figure that carries the argument. Reuse the running-coupling plot from the [lecture notes](https://example.org/notes).\n\n## Running order\n\n1. The puzzle: three colours, and no coloured particles (5 min)\n2. Running coupling and deep inelastic scattering (15 min)\n3. Flux tubes and the linear potential (15 min)\n4. Questions (10 min)\n\n## Before Thursday\n\n- [x] Book the seminar room\n- [x] Export Figure 1.3 at 300 dpi\n- [ ] Ask Mira for the lattice snapshots\n\n### Figures\n\n> Only colour-neutral combinations — hadrons — ever reach a detector.\n\n## Open questions\n\nDoes the linear-potential argument need a value for the string tension, or is the scaling enough? Rough estimate: `σ ≈ 0.18 GeV²`\n";

fn plan() -> DocTab {
    let dir = std::env::temp_dir().join(format!("quark-docs-shot-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("Seminar plan.md");
    std::fs::write(&path, PLAN).unwrap();
    let mut tab = DocTab::open(&path).unwrap();
    let doc = &mut tab.editor.doc;
    // A comment, answered by Claude, on "one figure that carries the argument".
    let goals = doc.blocks.iter().position(|b| b.text().starts_with("Get the room")).unwrap();
    let text = doc.blocks[goals].text().to_string();
    let at = text[..text.find("one figure").unwrap()].chars().count();
    doc.restyle(Pos::new(goals, at), Pos::new(goals, at + 36), |s| s.add_anchor(1));
    doc.comments.push(Comment {
        id: 1,
        quote: "one figure that carries the argument".into(),
        notes: vec![
            Note::now(Author::You, "Which figure? The αs plot or the flux-tube sketch?"),
            Note::now(Author::Claude, "The αs plot. It shows asymptotic freedom directly, and the rise at low Q sets up confinement, so one picture covers items 2 and 3."),
        ],
        resolved: false,
    });
    let mira = doc.blocks.iter().position(|b| b.text().starts_with("Ask Mira")).unwrap();
    let len = doc.blocks[mira].len();
    doc.restyle(Pos::new(mira, 0), Pos::new(mira, len), |s| s.add_anchor(2));
    doc.comments.push(Comment { id: 2, quote: "Ask Mira for the lattice snapshots".into(), notes: vec![Note::now(Author::You, "Check the licence before they go in the slides.")], resolved: false });
    tab.active_comment = Some(1);
    tab.editor.caret_to(Pos::new(mira, len));
    tab.exists = true;
    tab
}

fn draw(name: &str, dark: bool, mut tab: DocTab, around: Around) {
    let Some(dir) = folder() else { return };
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
    theme::apply(&ctx, mode);
    let p = Palette::for_mode(mode);
    shoot(&ctx, egui::vec2(1440.0, 860.0), 4, &dir.join(name), |ui| {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.ground).inner_margin(10)).show(ui, |ui| {
            let _ = view::surface(ui, &p, &mut tab, &around);
        });
    })
    .unwrap();
}

#[test]
#[ignore = "draws pictures; run with QUARK_SHOTS set"]
fn shot_the_docs_editor() {
    let around = || Around { dock_open: true, modal: false, claude: Status::Ready(quark_ai::Backend::ClaudeCode) };
    draw("docs-editor-dark.png", true, plan(), around());
    draw("docs-editor-light.png", false, plan(), around());

    // Claude: a question answered with a suggestion, and the selection box.
    let mut tab = plan();
    tab.active_comment = None;
    tab.dock = DockTab::Claude;
    let open = tab.editor.doc.blocks.iter().position(|b| b.text().starts_with("Does the linear")).unwrap();
    let text = tab.editor.doc.blocks[open].text().to_string();
    let from = text[..text.find("or is the scaling").unwrap()].chars().count();
    let id = tab.editor.suggest(Pos::new(open, from), Pos::new(open, from + 24), "or is showing that the energy grows linearly with separation enough on its own?".into(), String::new()).unwrap();
    tab.chat.push(ChatTurn { mine: true, text: "The open question at the end is vague. Tighten it.".into(), ..Default::default() });
    tab.chat.push(ChatTurn {
        mine: false,
        text: "I made the question concrete: it now asks whether showing the energy grows linearly is enough without a number for the string tension.".into(),
        activity: vec!["Read Seminar plan.md".into()],
        edits: vec![id],
        ..Default::default()
    });
    let goals = tab.editor.doc.blocks.iter().position(|b| b.text().starts_with("Get the room")).unwrap();
    let len = tab.editor.doc.blocks[goals].len();
    tab.editor.set_selection(Selection::new(Pos::new(goals, 0), Pos::new(goals, len)));
    tab.ask_box = Some("Make this sound less like a to-do list".into());
    draw("docs-claude-dark.png", true, tab, around());
}

#[test]
#[ignore = "draws pictures; run with QUARK_SHOTS set"]
fn shot_the_documents_home() {
    let Some(dir) = folder() else { return };
    let docs = std::env::temp_dir().join(format!("quark-docs-home-shot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&docs);
    std::fs::create_dir_all(&docs).unwrap();
    for (name, body) in [
        ("Seminar plan", PLAN),
        ("Reading group notes", "# Reading group notes\n\nWe read chapter four this week and argued about the second half of it for an hour.\n\nNext time: chapter five.\n"),
        ("Grant renewal — aims", "# Grant renewal\n\n## Aims\n\n- Measure the thing\n- Measure it again, better\n- Write it up\n\n## Budget\n\nModest.\n"),
        ("Lattice QCD primer", "# Lattice QCD primer\n\nSpace and time made a grid.\n\n```\nS = β Σ (1 − Re Tr U_p / 3)\n```\n\nThen the plaquettes.\n"),
        ("Thesis — chapter 2 outline", "# Chapter 2\n\n- [ ] Introduction\n- [x] Background\n- [ ] Method\n\nNotes to come.\n"),
    ] {
        std::fs::write(docs.join(format!("{name}.md")), markdown::write(&markdown::read(body))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::apply(&ctx, ThemeMode::Dark);
    let p = Palette::for_mode(ThemeMode::Dark);
    let mut home = crate::home::HomeState::default();
    home.section = crate::home::Section::Documents;
    let prefs = quark_core::prefs::Prefs { docs_folder: docs.display().to_string(), ..Default::default() };
    shoot(&ctx, egui::vec2(1440.0, 860.0), 4, &dir.join("docs-home-dark.png"), |ui| {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.ground).inner_margin(10)).show(ui, |ui| {
            let _ = crate::home::show(ui, &p, &mut home, &prefs);
        });
    })
    .unwrap();
}

/// The page, driven the way egui-winit drives it: typed text, keys, a
/// paste, and a click, through `surface` as the window calls it.
#[test]
fn input_reaches_the_page() {
    use egui::{Event, Key, Modifiers};
    let dir = std::env::temp_dir().join(format!("quark-docs-input-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let p = Palette::for_mode(ThemeMode::Dark);
    let mut tab = DocTab::blank(&dir);
    let around = Around { dock_open: true, modal: false, claude: Status::Missing };
    let frame = |tab: &mut DocTab, events: Vec<Event>| {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 860.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = view::surface(ui, &p, tab, &around);
            });
        });
        out.textures_delta.clear();
    };
    let key = |key: Key, modifiers: Modifiers| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let text = |t: &str| t.chars().map(|c| Event::Text(c.to_string())).collect::<Vec<_>>();

    frame(&mut tab, vec![]);
    frame(&mut tab, vec![]);
    let mut events = text("## Plan");
    events.push(key(Key::Enter, Modifiers::NONE));
    events.extend(text("Some "));
    events.push(key(Key::B, Modifiers::COMMAND));
    events.extend(text("bold"));
    events.push(key(Key::B, Modifiers::COMMAND));
    events.extend(text(" words."));
    events.push(key(Key::Enter, Modifiers::NONE));
    events.push(Event::Paste("- one\n- [ ] two".into()));
    frame(&mut tab, events);
    assert_eq!(markdown::write(&tab.editor.doc), "## Plan\n\nSome **bold** words.\n\n- one\n- [ ] two\n");

    // ⌘Z takes back the paste; the arrows and Backspace work on what is left.
    frame(&mut tab, vec![key(Key::Z, Modifiers::COMMAND)]);
    assert_eq!(markdown::write(&tab.editor.doc), "## Plan\n\nSome **bold** words.\n");
    frame(&mut tab, vec![key(Key::ArrowUp, Modifiers::NONE), key(Key::ArrowUp, Modifiers::NONE), key(Key::End, Modifiers::NONE), key(Key::Backspace, Modifiers::ALT)]);
    assert_eq!(tab.editor.doc.block(0).text(), "");

    // A click puts the caret where it lands.
    frame(&mut tab, vec![]);
    let at = tab.sheet.caret(Pos::new(1, 5)).translate(tab.sheet_origin.to_vec2()).center();
    frame(&mut tab, vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    frame(&mut tab, vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    assert_eq!(tab.editor.selection(), Selection::caret(Pos::new(1, 5)));
}
