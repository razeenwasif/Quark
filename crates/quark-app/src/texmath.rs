//! Equations typeset by TeX for the visual editor: each displayed equation
//! of the file put on a page of its own in one document, built by latexmk
//! with Quark's TinyTeX in a folder of its own (as the project is), and each
//! page drawn by PDFium, cut to its ink and kept as a picture of the ink
//! alone — drawn in the theme's colour of words, so it reads in dark and in
//! light.
//!
//! The document gets the macros the file's preamble defines, and the
//! packages any equation would want; an equation TeX says is wrong says so
//! instead. When the pages do not come out one for each equation — a
//! `\newpage` lost to an error — each is typeset again on its own.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use quark_lab::tex::build::{Build, BuildSpec, Engine};

use crate::texbuild::Tools;

/// Pixels per TeX point the pages are drawn at.
const SCALE: f32 = 4.0;
/// Points on screen per TeX point: a 12 pt equation beside 15 pt words.
const SHOWN: f32 = 1.4;
/// The most equations in one document.
const BATCH: usize = 48;
/// How long a document of equations may take.
const TIMEOUT: Duration = Duration::from_secs(60);
/// Ink lighter than this is paper.
const INK: u8 = 10;

/// An equation as the page has it.
pub(crate) enum Shown {
    /// No TeX to typeset it, or not yet typeset.
    Waiting,
    Ready(egui::TextureHandle, egui::Vec2),
    Failed(String),
}

enum State {
    Queued,
    Running,
    Drawn(egui::ColorImage),
    Shown(egui::TextureHandle, egui::Vec2),
    Failed(String),
}

struct Entry {
    tex: String,
    state: State,
    /// Typeset on its own, after a document of many went wrong.
    alone: bool,
}

/// A document of equations being built: which, and the lines each is on.
struct Running {
    build: Build,
    spec: BuildSpec,
    keys: Vec<u64>,
    lines: Vec<(u32, u32)>,
}

enum Work {
    Building(Running),
    /// Its pages being drawn, on a thread.
    Drawing(mpsc::Receiver<Vec<(u64, Result<egui::ColorImage, String>)>>),
}

/// The equations of the files drawn, typeset or waiting to be.
#[derive(Default)]
pub(crate) struct MathImages {
    entries: HashMap<u64, Entry>,
    work: Option<Work>,
    /// What each document begins with: the preamble's macros.
    macros: String,
}

impl MathImages {
    /// The macros the file's preamble defines, for every equation after.
    pub(crate) fn set_macros(&mut self, macros: &[(String, String)]) {
        let defined = definitions(macros);
        if defined != self.macros {
            self.macros = defined;
            // What was typeset with others is typeset again.
            self.entries
                .retain(|_, e| matches!(e.state, State::Running));
        }
    }

    fn key(&self, tex: &str) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.macros.hash(&mut h);
        tex.hash(&mut h);
        h.finish()
    }

    /// An equation, ready for TeX ([`crate::texinline::display_math`]): its
    /// picture, once there is one; asked for, the first time.
    pub(crate) fn shown(&mut self, ctx: &egui::Context, tex: &str) -> Shown {
        let key = self.key(tex);
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            tex: tex.to_string(),
            state: State::Queued,
            alone: false,
        });
        if let State::Drawn(image) = &entry.state {
            let size = egui::vec2(image.size[0] as f32, image.size[1] as f32) * (SHOWN / SCALE);
            let texture = ctx.load_texture(
                format!("tex-math-{key:x}"),
                image.clone(),
                egui::TextureOptions::LINEAR,
            );
            entry.state = State::Shown(texture, size);
        }
        match &entry.state {
            State::Shown(texture, size) => Shown::Ready(texture.clone(), *size),
            State::Failed(why) => Shown::Failed(why.clone()),
            _ => Shown::Waiting,
        }
    }

    /// Whether there is typesetting under way or waiting.
    pub(crate) fn busy(&self) -> bool {
        self.work.is_some()
            || self
                .entries
                .values()
                .any(|e| matches!(e.state, State::Queued))
    }

    /// A document begun for what waits, once TeX is there; one under way
    /// watched; its pages drawn. Whether anything changed.
    pub(crate) fn tend(&mut self, tools: Option<&Tools>) -> bool {
        match self.work.take() {
            None => {
                if let Some(tools) = tools {
                    self.begin(tools);
                }
                false
            }
            Some(Work::Building(mut running)) => {
                if !running.build.poll() {
                    self.work = Some(Work::Building(running));
                    return false;
                }
                self.built(running);
                true
            }
            Some(Work::Drawing(rx)) => match rx.try_recv() {
                Ok(drawn) => {
                    for (key, image) in drawn {
                        if let Some(entry) = self.entries.get_mut(&key) {
                            entry.state = match image {
                                Ok(image) => State::Drawn(image),
                                Err(why) => State::Failed(why),
                            };
                        }
                    }
                    true
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.work = Some(Work::Drawing(rx));
                    false
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.fail_running("the pages could not be drawn");
                    true
                }
            },
        }
    }

    fn fail_running(&mut self, why: &str) {
        for entry in self
            .entries
            .values_mut()
            .filter(|e| matches!(e.state, State::Running))
        {
            entry.state = State::Failed(why.to_string());
        }
    }

    /// A document of what waits — those to be typeset alone one at a time.
    fn begin(&mut self, tools: &Tools) {
        let mut keys: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.state, State::Queued) && !e.alone)
            .map(|(k, _)| *k)
            .collect();
        if keys.is_empty() {
            keys = self
                .entries
                .iter()
                .filter(|(_, e)| matches!(e.state, State::Queued))
                .map(|(k, _)| *k)
                .take(1)
                .collect();
        }
        if keys.is_empty() {
            return;
        }
        keys.sort_unstable();
        keys.truncate(BATCH);
        let texs: Vec<&str> = keys.iter().map(|k| self.entries[k].tex.as_str()).collect();
        let (document, lines) = document(&self.macros, &texs);
        let base = tools.tex_home.join("builds").join("visual-math");
        let root = base.join("src");
        let main = root.join("math.tex");
        let written = std::fs::create_dir_all(&root).and_then(|_| std::fs::write(&main, document));
        let spec = BuildSpec {
            root: root.clone(),
            main,
            dir: base.join("build"),
            bin: tools.marker.bin.clone(),
            tex_home: tools.tex_home.clone(),
            home: tools.person.clone(),
            engine: Engine::Pdflatex,
            timeout: TIMEOUT,
            sandbox: tools.sandbox,
            shell_escape: false,
            reads: Vec::new(),
        };
        let started = written
            .map_err(|e| e.to_string())
            .and_then(|_| Build::start(&spec));
        for key in &keys {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.state = State::Running;
            }
        }
        match started {
            Ok(build) => {
                self.work = Some(Work::Building(Running {
                    build,
                    spec,
                    keys,
                    lines,
                }))
            }
            Err(why) => self.fail_running(&format!("TeX could not be started: {why}")),
        }
    }

    /// A document built: the equations TeX said were wrong, failed; the
    /// rest drawn from their pages — or, when the pages and the equations
    /// do not pair up, each typeset again alone.
    fn built(&mut self, running: Running) {
        let Running {
            build,
            spec,
            keys,
            lines,
        } = running;
        let root = quark_lab::sandbox::real(&spec.root).display().to_string();
        let log = quark_tex::log::read(
            &std::fs::read_to_string(spec.log()).unwrap_or_default(),
            &root,
            &root,
        );
        let mut wrong: HashMap<u64, String> = HashMap::new();
        for entry in log
            .entries
            .iter()
            .filter(|e| e.kind == quark_tex::log::Kind::Error)
        {
            let Some(line) = entry.line else { continue };
            if let Some(i) = lines.iter().position(|(a, b)| *a <= line && line <= *b) {
                wrong
                    .entry(keys[i])
                    .or_insert_with(|| entry.message.clone());
            }
        }
        let pdf = spec.pdf();
        let pages = pdf.is_file().then(|| page_count(&pdf)).flatten();
        if pages != Some(keys.len()) {
            if keys.len() > 1 {
                for key in &keys {
                    if let Some(entry) = self.entries.get_mut(key) {
                        (entry.state, entry.alone) = match wrong.remove(key) {
                            Some(why) => (State::Failed(why), true),
                            None => (State::Queued, true),
                        };
                    }
                }
                return;
            }
            let why = wrong
                .into_values()
                .next()
                .or_else(|| {
                    log.entries
                        .iter()
                        .find(|e| e.kind == quark_tex::log::Kind::Error)
                        .map(|e| e.message.clone())
                })
                .unwrap_or_else(|| {
                    let said = build.said();
                    if said.is_empty() {
                        "TeX made nothing of it".into()
                    } else {
                        said
                    }
                });
            self.fail_running(&why);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let wanted: Vec<(usize, u64)> = keys
            .iter()
            .enumerate()
            .filter(|(_, k)| !wrong.contains_key(k))
            .map(|(i, k)| (i, *k))
            .collect();
        let mut out: Vec<(u64, Result<egui::ColorImage, String>)> =
            wrong.into_iter().map(|(k, why)| (k, Err(why))).collect();
        std::thread::spawn(move || {
            out.extend(draw(&pdf, &wanted));
            let _ = tx.send(out);
        });
        self.work = Some(Work::Drawing(rx));
    }
}

/// The macros, defined for any equation: provided, then defined again, so
/// one LaTeX already has is the preamble's.
fn definitions(macros: &[(String, String)]) -> String {
    let mut out = String::new();
    for (name, body) in macros {
        let arguments = (1..=9)
            .rev()
            .find(|n| body.contains(&format!("#{n}")))
            .unwrap_or(0);
        let count = if arguments > 0 {
            format!("[{arguments}]")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "\\providecommand{{{name}}}{{}}\\renewcommand{{{name}}}{count}{{{body}}}\n"
        ));
    }
    out
}

/// The document of `texs`, a page each, and the lines (one-based) each is on.
fn document(macros: &str, texs: &[&str]) -> (String, Vec<(u32, u32)>) {
    let mut out = String::from(
        "\\documentclass[12pt]{article}\n\\usepackage[paperwidth=6.5in,paperheight=6in,margin=0.25in]{geometry}\n\\usepackage{amsmath,amssymb}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0pt}\n",
    );
    out.push_str(macros);
    out.push_str("\\begin{document}\n");
    let mut lines = Vec::with_capacity(texs.len());
    for tex in texs {
        let first = out.lines().count() as u32 + 1;
        out.push_str(tex.trim());
        out.push('\n');
        lines.push((first, out.lines().count() as u32));
        out.push_str("\\newpage\n");
    }
    out.push_str("\\end{document}\n");
    (out, lines)
}

fn page_count(pdf: &Path) -> Option<usize> {
    let bytes = std::fs::read(pdf).ok()?;
    quark_pdf::doc::Document::from_bytes(bytes, None)
        .ok()
        .map(|d| d.page_count())
}

/// Each wanted page drawn and cut to its ink.
fn draw(pdf: &PathBuf, wanted: &[(usize, u64)]) -> Vec<(u64, Result<egui::ColorImage, String>)> {
    let doc = match std::fs::read(pdf)
        .map_err(|e| e.to_string())
        .and_then(|b| quark_pdf::doc::Document::from_bytes(b, None).map_err(|e| e.to_string()))
    {
        Ok(doc) => doc,
        Err(why) => return wanted.iter().map(|(_, k)| (*k, Err(why.clone()))).collect(),
    };
    wanted
        .iter()
        .map(|&(page, key)| {
            let drawn = quark_pdf::render::render_page(
                &doc,
                &quark_pdf::render::RenderRequest {
                    page,
                    scale: SCALE,
                    annotations: false,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())
            .and_then(|r| {
                ink(r.width as usize, r.height as usize, &r.rgba)
                    .ok_or_else(|| "TeX drew nothing".to_string())
            });
            (key, drawn)
        })
        .collect()
}

/// A page's ink alone, cut to it with a little paper round it: white with
/// the ink as its opacity, to be drawn in any colour.
pub(crate) fn ink(width: usize, height: usize, rgba: &[u8]) -> Option<egui::ColorImage> {
    let alpha = |x: usize, y: usize| {
        let i = (y * width + x) * 4;
        let dark = 255 - rgba[i].min(rgba[i + 1]).min(rgba[i + 2]);
        // Paper with no ink is no ink, whatever it is drawn as.
        if rgba[i + 3] == 0 { 0 } else { dark }
    };
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if alpha(x, y) > INK {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
        }
    }
    if left > right {
        return None;
    }
    let pad = 4;
    let (left, top) = (left.saturating_sub(pad), top.saturating_sub(pad));
    let (right, bottom) = ((right + pad).min(width - 1), (bottom + pad).min(height - 1));
    let (w, h) = (right - left + 1, bottom - top + 1);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in top..=bottom {
        for x in left..=right {
            pixels.extend_from_slice(&[255, 255, 255, alpha(x, y)]);
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// latexmk, standing in: a PDF of a page for each `\newpage` (from
    /// `@PDFS@/<n>.pdf`), and an error on any line that says `\bad`.
    const LATEXMK: &str = r#"#!/bin/sh
out=""; main=""
for a in "$@"; do case "$a" in -outdir=*) out="${a#-outdir=}" ;; -*) ;; *) main="$a" ;; esac; done
dir=$(cd "$(dirname "$main")" && pwd -P)
: > "$out/math.log"
grep -n 'bad' "$main" | while IFS=: read -r line rest; do
  printf '%s\n' "($dir/math.tex" "$dir/math.tex:$line: Undefined control sequence." "l.$line \\bad" "" >> "$out/math.log"
done
n=$(grep -c 'newpage' "$main")
if grep -q 'losepage' "$main"; then n=$((n - 1)); fi
if [ "$n" -gt 0 ]; then cp "@PDFS@/$n.pdf" "$out/math.pdf"; fi
"#;

    fn bench(name: &str) -> Tools {
        let (_, mut tools) = crate::texbuild::tests::bench(name);
        let pdfs = tools.tex_home.join("pdfs");
        std::fs::create_dir_all(&pdfs).unwrap();
        for n in 1..=4 {
            std::fs::copy(
                quark_pdf::testutil::many_page_pdf(n),
                pdfs.join(format!("{n}.pdf")),
            )
            .unwrap();
        }
        let latexmk = tools.marker.bin.join("latexmk");
        std::fs::write(
            &latexmk,
            LATEXMK.replace("@PDFS@", &pdfs.display().to_string()),
        )
        .unwrap();
        std::fs::set_permissions(&latexmk, std::fs::Permissions::from_mode(0o755)).unwrap();
        tools.sandbox = quark_lab::sandbox::Kind::None;
        tools
    }

    fn settle(math: &mut MathImages, tools: &Tools) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while math.busy() {
            math.tend(Some(tools));
            assert!(std::time::Instant::now() < deadline, "never settled");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn states(math: &mut MathImages, ctx: &egui::Context, texs: &[&str]) -> Vec<String> {
        texs.iter()
            .map(|t| match math.shown(ctx, t) {
                Shown::Waiting => "waiting".to_string(),
                Shown::Ready(_, size) => format!("ready {}", size.x > 0.0),
                Shown::Failed(why) => format!("failed: {why}"),
            })
            .collect()
    }

    #[test]
    fn equations_are_typeset_together_a_page_each_and_drawn_as_their_ink() {
        let _g = quark_pdf::testutil::pdfium_guard();
        let tools = bench("math-many");
        let ctx = egui::Context::default();
        let mut math = MathImages::default();
        math.set_macros(&[
            ("\\R".into(), "\\mathbb{R}".into()),
            ("\\abs".into(), "|#1|".into()),
        ]);
        let texs = [
            "\\[ x \\in \\R \\]",
            "\\[ \\abs{y} \\]",
            "\\begin{align*} a &= b \\end{align*}",
        ];
        assert_eq!(states(&mut math, &ctx, &texs), ["waiting"; 3]);
        math.tend(None);
        assert!(math.work.is_none(), "nothing begins without TeX");
        settle(&mut math, &tools);
        assert_eq!(states(&mut math, &ctx, &texs), ["ready true"; 3]);
        let written =
            std::fs::read_to_string(tools.tex_home.join("builds/visual-math/src/math.tex"))
                .unwrap();
        assert!(
            written.contains("\\providecommand{\\abs}{}\\renewcommand{\\abs}[1]{|#1|}"),
            "{written}"
        );
        assert_eq!(written.matches("\\newpage").count(), 3);
    }

    #[test]
    fn an_equation_tex_says_is_wrong_says_so_and_the_rest_are_drawn() {
        let _g = quark_pdf::testutil::pdfium_guard();
        let tools = bench("math-wrong");
        let ctx = egui::Context::default();
        let mut math = MathImages::default();
        let texs = ["\\[ a \\]", "\\[ \\bad \\]", "\\[ c \\]"];
        states(&mut math, &ctx, &texs);
        settle(&mut math, &tools);
        let got = states(&mut math, &ctx, &texs);
        assert_eq!(
            got.iter().filter(|s| *s == "ready true").count(),
            2,
            "{got:?}"
        );
        assert!(
            got.contains(&"failed: Undefined control sequence.".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn when_the_pages_do_not_pair_up_each_is_typeset_alone() {
        let _g = quark_pdf::testutil::pdfium_guard();
        let tools = bench("math-alone");
        let ctx = egui::Context::default();
        let mut math = MathImages::default();
        // One loses its page: together, two pages for three.
        let texs = ["\\[ a \\]", "\\[ b \\losepage \\]", "\\[ c \\]"];
        states(&mut math, &ctx, &texs);
        settle(&mut math, &tools);
        let got = states(&mut math, &ctx, &texs);
        assert_eq!(got[0], "ready true");
        assert_eq!(got[2], "ready true");
        assert!(
            got[1].starts_with("failed"),
            "alone, it makes nothing: {got:?}"
        );
    }

    #[test]
    fn a_page_is_cut_to_its_ink() {
        let (w, h) = (40, 30);
        let mut rgba = vec![255u8; w * h * 4];
        for y in 10..15 {
            for x in 20..30 {
                let i = (y * w + x) * 4;
                rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let cut = ink(w, h, &rgba).unwrap();
        assert_eq!(
            cut.size,
            [10 + 8, 5 + 8],
            "the ink and four pixels round it"
        );
        assert_eq!(cut.pixels[(4 * cut.size[0]) + 4].a(), 255);
        assert_eq!(cut.pixels[0].a(), 0);
        assert!(ink(w, h, &vec![255u8; w * h * 4]).is_none());
    }
}
