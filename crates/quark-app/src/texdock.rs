//! The LaTeX workspace's dock: where TeX stands. Quark installs its own —
//! TinyTeX, a TeX Live made small, with latexmk, and texlab for completion —
//! in `~/.quark/tex`, with the person's say-so twice: once to plan it
//! (GitHub is asked what its newest releases are and what they cost, exact
//! sizes said as such), and once to install exactly that. The engine is
//! `quark_lab::tex`; this draws where it stands and asks for the next step
//! only when a button says so.

use egui::{CornerRadius, RichText, Ui, vec2};
use quark_lab::tex::{Bundle, TexPlan, TexState};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

/// An install's stages, as the engine names them.
const STAGES: [(&str, &str); 5] = [
    ("tinytex", "TinyTeX"),
    ("latexmk", "latexmk"),
    ("texlab", "texlab, for completion"),
    ("checking", "A first page"),
    ("finishing", "Tidying up"),
];

/// What a button asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvAction {
    /// Ask GitHub what installing this bundle would be.
    Plan(Bundle),
    /// Install exactly what was planned.
    Create,
    /// Stop, or put a plan or a failure away.
    Cancel,
    Remove,
}

/// What the dock keeps between frames: the bundle chosen, and whether a
/// removal is being confirmed.
#[derive(Debug, Default)]
pub(crate) struct Dock {
    pub bundle: Bundle,
    pub confirming: bool,
}

/// A size, as a person reads one.
pub(crate) fn bytes(n: u64) -> String {
    crate::home::human_size(n)
}

/// A size said as exact, or as about.
pub(crate) fn size(n: u64, exact: bool) -> String {
    if exact { bytes(n) } else { format!("about {}", bytes(n)) }
}

/// A path with the home folder as `~`.
fn home(path: &str) -> String {
    quark_pdf::library::tilde(std::path::Path::new(path))
}

/// The stage reached, as a share of the stages: done before it, then it.
pub(crate) fn stage_index(stage: &str) -> Option<usize> {
    STAGES.iter().position(|(id, _)| *id == stage)
}

/// What failed, in words.
pub(crate) fn failed_step(step: &str) -> &'static str {
    match step {
        "plan" => "Planning",
        "tinytex" => "Installing TinyTeX",
        "latexmk" => "Installing latexmk",
        "texlab" => "Installing texlab",
        "checking" => "The first page",
        "finishing" => "Finishing",
        _ => "It",
    }
}

/// The plan, as a sentence: what, how much, where.
pub(crate) fn plan_said(plan: &TexPlan) -> String {
    let estimated = plan.downloads.iter().any(|d| !d.exact);
    let name = if plan.bundle == Bundle::Small { "TinyTeX-1" } else { "TinyTeX" };
    format!(
        "{name} {}, with latexmk, and texlab {}: {} to download, taking up to {} while it is installed and keeping about {}, in {}.",
        plan.tinytex,
        plan.texlab,
        size(plan.download, !estimated),
        bytes(plan.disk),
        bytes(plan.kept),
        home(&plan.home),
    )
}

/// The dock, for `state`; what was clicked.
pub(crate) fn show(ui: &mut Ui, p: &Palette, state: &TexState, dock: &mut Dock) -> Option<EnvAction> {
    let mut action = None;
    // Everything wraps to the dock: nothing may widen it.
    ui.set_max_width(ui.available_width());
    ui.label(RichText::new("TeX").font(theme::strong(14.0)).color(p.text));
    ui.add_space(8.0);
    let faint = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).font(theme::body(12.0)).color(p.text_faint));
    };
    let body = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).font(theme::body(12.5)).color(p.text_muted));
    };
    let log = |ui: &mut Ui, lines: &[String]| {
        if lines.is_empty() {
            return;
        }
        let tail: Vec<&str> = lines.iter().rev().take(8).rev().map(String::as_str).collect();
        egui::Frame::new().fill(p.recess).corner_radius(CornerRadius::same(8)).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(tail.join("\n")).font(egui::FontId::monospace(11.0)).color(p.text_muted));
        });
    };
    match state {
        TexState::Unknown => faint(ui, "Looking for TeX…"),
        TexState::Missing { needs, free, home: at } => {
            body(
                ui,
                &format!(
                    "To typeset, Quark needs TeX. It installs its own — TinyTeX, a TeX Live made small, with pdfLaTeX, XeLaTeX, LuaLaTeX, BibTeX, biber and latexmk, and texlab for completion — in {}, apart from any TeX of yours. Documents are typeset in the sandbox: no network, and nothing written outside their build folder.",
                    home(at)
                ),
            );
            ui.add_space(8.0);
            if !needs.is_empty() {
                body(ui, "It cannot be installed here yet. It needs:");
                for n in needs {
                    faint(ui, &format!("•  {n}"));
                }
                return None;
            }
            for (value, name, note) in [
                (Bundle::Full, "TinyTeX — about 210 MB (recommended)", "Most articles, theses and Beamer decks compile without asking for more."),
                (Bundle::Small, "TinyTeX-1 — about 67 MB", "The common core: more documents stop to ask for a package the first time."),
            ] {
                ui.radio_value(&mut dock.bundle, value, RichText::new(name).font(theme::strong(12.5)));
                egui::Frame::new().inner_margin(egui::Margin { left: 24, ..Default::default() }).show(ui, |ui| faint(ui, note));
                ui.add_space(4.0);
            }
            ui.add_space(6.0);
            body(
                ui,
                &format!(
                    "First, a plan. Quark asks GitHub for TinyTeX's and texlab's newest releases and what they cost — a few kilobytes. Nothing is downloaded yet. {} is free here.",
                    bytes(*free)
                ),
            );
            ui.add_space(8.0);
            if widgets::primary_button(ui, p, "Plan TeX", true).clicked() {
                action = Some(EnvAction::Plan(dock.bundle));
            }
        }
        TexState::Planning => {
            ui.add(egui::Spinner::new().size(13.0));
            body(ui, "Asking GitHub for TinyTeX's and texlab's newest releases…");
            ui.add_space(8.0);
            if widgets::ghost_button(ui, p, "Stop").clicked() {
                action = Some(EnvAction::Cancel);
            }
        }
        TexState::Planned { plan } => {
            body(ui, &format!("Then, if you agree. {}", plan_said(plan)));
            if plan.fits {
                faint(ui, &format!("{} is free.", bytes(plan.free)));
            } else {
                ui.label(
                    RichText::new(format!("There is not room for it: {} is free.", bytes(plan.free)))
                        .font(theme::body(12.0))
                        .color(p.warning),
                );
            }
            ui.add_space(8.0);
            // What it downloads: each a row, its size and whether it is
            // checked on the right.
            for d in &plan.downloads {
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), egui::Sense::hover());
                let painter = ui.painter();
                painter.text(row.left_top() + vec2(0.0, 2.0), egui::Align2::LEFT_TOP, &d.what, theme::body(12.5), p.text);
                painter.text(row.left_top() + vec2(0.0, 19.0), egui::Align2::LEFT_TOP, &d.host, theme::body(11.0), p.text_faint);
                if d.verified {
                    let tick = egui::Rect::from_center_size(row.right_center() - vec2(8.0, 0.0), vec2(14.0, 14.0));
                    quark_ui::icons::draw(painter, tick, quark_ui::icons::Icon::Check, p.accent_text);
                }
                painter.text(
                    row.right_center() - vec2(24.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    size(d.bytes, d.exact),
                    theme::body(12.0),
                    p.text_muted,
                );
            }
            ui.add_space(4.0);
            let estimated = plan.downloads.iter().any(|d| !d.exact);
            faint(
                ui,
                &format!(
                    "Those ticked are checked against the SHA-256 GitHub publishes.{}",
                    if estimated { " latexmk's size is an estimate: TeX's package manager fetches it." } else { "" }
                ),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, p, "Install", plan.fits).clicked() {
                    action = Some(EnvAction::Create);
                }
                if widgets::ghost_button(ui, p, "Not now").clicked() {
                    action = Some(EnvAction::Cancel);
                }
            });
        }
        TexState::Making { stage, done, total, log: lines } => {
            let at = stage_index(stage);
            let name = at.map_or(stage.as_str(), |i| STAGES[i].1);
            let amount = if *total > 0 { format!(" — {} of {}", bytes(*done), bytes(*total)) } else { String::new() };
            ui.add(egui::Spinner::new().size(13.0));
            body(ui, &format!("Installing TeX: {name}{amount}."));
            if *total > 0 {
                let share = (*done as f32 / *total as f32).clamp(0.0, 1.0);
                ui.add(egui::ProgressBar::new(share).desired_height(5.0));
            }
            ui.add_space(6.0);
            for (i, (_, label)) in STAGES.iter().enumerate() {
                let (done, colour) = match at {
                    Some(n) if i < n => (true, p.text_muted),
                    Some(n) if i == n => (false, p.text),
                    _ => (false, p.text_faint),
                };
                ui.horizontal(|ui| {
                    let (tick, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                    if done {
                        quark_ui::icons::draw(ui.painter(), tick, quark_ui::icons::Icon::Check, p.accent_text);
                    } else if Some(i) == at {
                        ui.painter().circle_filled(tick.center(), 3.0, p.accent);
                    }
                    ui.label(RichText::new(*label).font(theme::body(12.0)).color(colour));
                });
            }
            ui.add_space(6.0);
            log(ui, lines);
            ui.add_space(6.0);
            if widgets::ghost_button(ui, p, "Stop").on_hover_text("Nothing it installed is kept").clicked() {
                action = Some(EnvAction::Cancel);
            }
        }
        TexState::Ready { tinytex, texlab, bundle, size: kept, home: at } => {
            let name = if *bundle == Bundle::Small { "TinyTeX-1" } else { "TinyTeX" };
            ui.horizontal(|ui| {
                let (tick, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                quark_ui::icons::draw(ui.painter(), tick, quark_ui::icons::Icon::Check, p.accent_text);
                ui.label(RichText::new(format!("{name} {tinytex} · texlab {texlab}")).font(theme::body(12.5)).color(p.text_muted));
            });
            faint(ui, &format!("{} in {}", bytes(*kept), home(at)));
            ui.add_space(8.0);
            if dock.confirming {
                body(ui, "Remove TeX, and every project's builds?");
                ui.horizontal(|ui| {
                    if widgets::danger_button(ui, p, "Remove").clicked() {
                        dock.confirming = false;
                        action = Some(EnvAction::Remove);
                    }
                    if widgets::ghost_button(ui, p, "Keep it").clicked() {
                        dock.confirming = false;
                    }
                });
            } else if ui.small_button("Remove…").clicked() {
                dock.confirming = true;
            }
        }
        TexState::Removing => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(13.0));
                body(ui, "Removing TeX…");
            });
        }
        TexState::Failed { step, why, log: lines } => {
            ui.label(
                RichText::new(format!("{} failed: {why}", failed_step(step)))
                    .font(theme::body(12.5))
                    .color(p.warning),
            );
            ui.add_space(6.0);
            log(ui, lines);
            faint(ui, "Nothing it installed is kept.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, p, "Try again", true).clicked() {
                    action = Some(EnvAction::Plan(dock.bundle));
                }
                if widgets::ghost_button(ui, p, "Put it away").clicked() {
                    action = Some(EnvAction::Cancel);
                }
            });
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_lab::tex::TexDownload;

    fn plan(exact: bool) -> TexPlan {
        TexPlan {
            bundle: Bundle::Full,
            tinytex: "v2026.10".into(),
            texlab: "v5.26.0".into(),
            downloads: vec![
                TexDownload { what: "TinyTeX".into(), host: "github.com".into(), bytes: 120 << 20, exact: true, verified: true },
                TexDownload { what: "latexmk".into(), host: "mirror.ctan.org".into(), bytes: 1 << 20, exact, verified: false },
            ],
            download: 121 << 20,
            disk: 600 << 20,
            kept: 210 << 20,
            free: 90 << 30,
            fits: true,
            home: "/nowhere/.quark/tex".into(),
            tinytex_url: String::new(),
            tinytex_sha256: None,
            texlab_url: String::new(),
            texlab_sha256: None,
        }
    }

    #[test]
    fn a_plan_says_what_it_costs_and_whether_that_is_exact() {
        let exact = plan_said(&plan(true));
        assert!(exact.contains("TinyTeX v2026.10") && exact.contains("texlab v5.26.0"), "{exact}");
        assert!(exact.contains(": 121.0 MB to download"), "{exact}");
        assert!(exact.contains("up to 600.0 MB") && exact.contains("about 210.0 MB"), "{exact}");
        assert!(plan_said(&plan(false)).contains("about 121.0 MB to download"), "an estimate is said as one");
        let small = TexPlan { bundle: Bundle::Small, ..plan(true) };
        assert!(plan_said(&small).starts_with("TinyTeX-1 "));
    }

    #[test]
    fn stages_and_failures_are_said_in_words() {
        assert_eq!(stage_index("tinytex"), Some(0));
        assert_eq!(stage_index("checking"), Some(3));
        assert_eq!(stage_index("elsewhere"), None);
        assert_eq!(failed_step("texlab"), "Installing texlab");
        assert_eq!(failed_step("mystery"), "It");
        assert_eq!(size(1536, false), "about 2 KB");
    }

    #[test]
    fn each_state_draws_and_only_a_button_asks_for_the_next_step() {
        // Drawn headless, with nothing clicked: no state asks for anything
        // of its own accord — a download is only ever asked for.
        let states = [
            TexState::Unknown,
            TexState::Missing { needs: vec![], free: 90 << 30, home: "/h/.quark/tex".into() },
            TexState::Missing { needs: vec!["perl".into()], free: 0, home: "/h/.quark/tex".into() },
            TexState::Planning,
            TexState::Planned { plan: plan(true) },
            TexState::Making { stage: "texlab".into(), done: 3, total: 10, log: vec!["unpacking".into()] },
            TexState::Ready { tinytex: "v1".into(), texlab: "v2".into(), bundle: Bundle::Full, size: 210 << 20, home: "/h/.quark/tex".into() },
            TexState::Removing,
            TexState::Failed { step: "tinytex".into(), why: "no network".into(), log: vec![] },
        ];
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let p = Palette::for_mode(theme::ThemeMode::Dark);
        for state in &states {
            let mut dock = Dock::default();
            let mut asked = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| asked = show(ui, &p, state, &mut dock));
            out.textures_delta.clear();
            assert_eq!(asked, None, "{state:?}");
        }
    }
}
