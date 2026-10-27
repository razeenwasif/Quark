//! Typesetting a LaTeX project: its main file found, built by latexmk with
//! Quark's TinyTeX in a folder of its own outside the project
//! (`quark_lab::tex::build`), and what came of it read — the log, the PDF,
//! SyncTeX's map — for the dock to show. From Proton's core (`tex/mod.rs`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use quark_lab::run::State as RunState;
use quark_lab::sandbox::Kind;
use quark_lab::tex::Marker;
use quark_lab::tex::build::{Build, BuildSpec, Engine};
use quark_tex::log::Log;
use quark_tex::outline::Outline;
use quark_tex::synctex::{Found, Map};

/// How long a build may take.
const TIMEOUT: Duration = Duration::from_secs(240);

/// The files whose saving typesets the project again.
const SOURCES: [&str; 6] = ["tex", "bib", "sty", "cls", "bst", "ltx"];

/// Whether saving `path` is saving some of a LaTeX project.
pub(crate) fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCES.contains(&e.to_ascii_lowercase().as_str()))
}

/// How a build ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    Running,
    /// A PDF was made, perhaps with errors.
    Done,
    /// None was.
    Failed,
    Stopped,
    TimedOut,
    /// No file of the project begins a document.
    NoMain,
}

/// Where the last build stands, for the dock.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Built {
    pub state: Ended,
    /// The main file, as the project names it.
    pub main: Option<String>,
    pub engine: &'static str,
    pub seconds: f32,
    /// The PDF to show: the last one made, when this build made none.
    pub pdf: Option<PathBuf>,
    /// Goes up with each PDF made, so the dock reads it again.
    pub version: u64,
    pub errors: usize,
    pub warnings: usize,
    pub boxes: usize,
    pub pages: Option<u32>,
    /// What went wrong when there is no log to say: latexmk's last words.
    pub said: Option<String>,
}

/// What a build is given from the machine's TeX.
pub(crate) struct Tools {
    pub marker: Marker,
    pub tex_home: PathBuf,
    pub person: PathBuf,
    pub sandbox: Kind,
}

/// A build under way.
struct Building {
    build: Build,
    spec: BuildSpec,
    root: PathBuf,
    engine: Engine,
}

/// A project's builds: the one under way, and what the last came to.
#[derive(Default)]
pub(crate) struct Builder {
    building: Option<Building>,
    pub last: Option<Built>,
    version: u64,
    /// SyncTeX's map of the last PDF made, and the project as TeX named it.
    synctex: Option<(Map, PathBuf)>,
    /// What the last build's log said.
    pub log: Option<Log>,
}

impl Builder {
    pub(crate) fn running(&self) -> bool {
        self.building.is_some()
    }

    /// Typeset the project at `root`, from the file being edited, `active`:
    /// its main file found and built, a build under way stopped first.
    pub(crate) fn compile(&mut self, root: &Path, active: Option<&Path>, tools: &Tools) -> Result<(), String> {
        let Some(main) = main_of(root, active) else {
            self.last = Some(Built {
                state: Ended::NoMain,
                main: None,
                engine: "",
                seconds: 0.0,
                pdf: self.last.as_ref().and_then(|l| l.pdf.clone()),
                version: self.version,
                errors: 0,
                warnings: 0,
                boxes: 0,
                pages: None,
                said: None,
            });
            return Ok(());
        };
        let text = std::fs::read_to_string(&main).unwrap_or_default();
        let engine = Engine::from_name(&quark_tex::project::engine(&text, None)).unwrap_or_default();
        // A save while a build runs: that one stops, with everything it started.
        self.building = None;
        let spec = BuildSpec {
            root: root.to_path_buf(),
            main: main.clone(),
            dir: build_dir(&tools.tex_home, root, &main),
            bin: tools.marker.bin.clone(),
            tex_home: tools.tex_home.clone(),
            home: tools.person.clone(),
            engine,
            timeout: TIMEOUT,
            sandbox: tools.sandbox,
            shell_escape: false,
            reads: Vec::new(),
        };
        let build = Build::start(&spec)?;
        self.building = Some(Building { build, spec, root: root.to_path_buf(), engine });
        self.last = Some(Built {
            state: Ended::Running,
            main: Some(relative(root, &main)),
            engine: engine.name(),
            seconds: 0.0,
            pdf: self.last.as_ref().and_then(|l| l.pdf.clone()),
            version: self.version,
            errors: 0,
            warnings: 0,
            boxes: 0,
            pages: None,
            said: None,
        });
        Ok(())
    }

    /// Stop the build under way.
    pub(crate) fn stop(&mut self) {
        if let Some(b) = &mut self.building {
            b.build.stop();
        }
    }

    /// A build that ended: its log read, its PDF copied where the dock reads
    /// it. `true` when one ended — a build stopped for another says nothing.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(building) = &mut self.building else { return false };
        if !building.build.poll() {
            if let Some(last) = &mut self.last {
                last.seconds = building.build.seconds() as f32;
            }
            return false;
        }
        let Building { build, spec, root, engine } = self.building.take().expect("a build");
        // As TeX names them: its log has the paths it was given, resolved.
        let real_root = quark_lab::sandbox::real(&root);
        let main_dir = quark_lab::sandbox::real(spec.main.parent().unwrap_or(&root)).display().to_string();
        let root_text = real_root.display().to_string();
        let mut log = quark_tex::log::read(&std::fs::read_to_string(spec.log()).unwrap_or_default(), &main_dir, &root_text);
        if let Ok(text) = std::fs::read_to_string(spec.log().with_extension("blg")) {
            let bib = quark_tex::log::read_bib(&text, &main_dir, &root_text);
            log.entries.extend(bib.entries);
            for missing in bib.missing {
                if !log.missing.contains(&missing) {
                    log.missing.push(missing);
                }
            }
        }
        let made = spec.pdf();
        // SyncTeX's map goes with the PDF it describes.
        if made.is_file() {
            if let Ok(map) = std::fs::read(spec.synctex()).map_err(|e| e.to_string()).and_then(|b| Map::read(&b)) {
                self.synctex = Some((map, real_root.clone()));
            }
        }
        let pdf = match build.state() {
            RunState::Done | RunState::Failed if made.is_file() => match show(&spec, &made) {
                Ok(view) => {
                    self.version += 1;
                    Some(view)
                }
                Err(_) => None,
            },
            _ => None,
        };
        let state = match (build.state(), &pdf) {
            (RunState::Done | RunState::Failed, Some(_)) => Ended::Done,
            (RunState::Stopped, _) => Ended::Stopped,
            (RunState::TimedOut, _) => Ended::TimedOut,
            _ => Ended::Failed,
        };
        let said = (pdf.is_none() && log.entries.is_empty()).then(|| build.said()).filter(|s| !s.is_empty());
        self.last = Some(Built {
            state,
            main: Some(relative(&root, &spec.main)),
            engine: engine.name(),
            seconds: build.seconds() as f32,
            pdf: pdf.or_else(|| self.last.as_ref().and_then(|l| l.pdf.clone())),
            version: self.version,
            errors: log.errors(),
            warnings: log.warnings(),
            boxes: log.boxes(),
            pages: log.pages,
            said,
        });
        if state == Ended::Stopped {
            return false;
        }
        self.log = Some(log);
        true
    }

    /// Where `line` (one-based) of `file`, a path in the project, is in the
    /// PDF last made.
    pub(crate) fn forward(&self, root: &Path, file: &Path, line: u32) -> Result<Found, String> {
        let Some((map, _)) = &self.synctex else { return Err("Typeset the document first.".into()) };
        let rel = relative(root, file);
        map.forward(&rel, line).ok_or_else(|| format!("{rel}:{line} is not in the PDF."))
    }

    /// The file and line (one-based) that put something at a point on a page
    /// (one-based, in points from its top left) of the PDF last made, as the
    /// project names the file.
    pub(crate) fn inverse(&self, page: u32, x: f64, y: f64) -> Option<(String, u32)> {
        let (map, real_root) = self.synctex.as_ref()?;
        let source = map.inverse(page, x, y)?;
        let file = PathBuf::from(normalise(&source.file));
        let file = file.strip_prefix(real_root).map(Path::to_path_buf).unwrap_or(file);
        Some((file.display().to_string(), source.line))
    }
}

/// The document's outline and word count, read from its files as saved.
pub(crate) fn outline_of(root: &Path, active: Option<&Path>) -> (Option<PathBuf>, Outline) {
    let main = main_of(root, active);
    let outline = main.as_deref().map(|m| quark_tex::outline::outline(root, m)).unwrap_or_default();
    (main, outline)
}

/// A path with its `.` and `..` worked out.
fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("{}{}", if path.starts_with('/') { "/" } else { "" }, parts.join("/"))
}

/// The main file of the project at `root`, from the file being edited, and
/// the one pinned in `.quark/tex.json`, if any.
pub(crate) fn main_of(root: &Path, active: Option<&Path>) -> Option<PathBuf> {
    quark_tex::project::find_main(root, active, pinned_main(root).as_deref())
}

fn pinned_main(root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(root.join(".quark").join("tex.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["main"].as_str().map(PathBuf::from)
}

/// A path as the project names it.
pub(crate) fn relative(root: &Path, path: &Path) -> String {
    let real = quark_lab::sandbox::real(root);
    path.strip_prefix(root).or_else(|_| path.strip_prefix(&real)).unwrap_or(path).display().to_string()
}

/// Where a project's main file is built: under TeX's folder, never the
/// project's — `<project>-<hash>/<main, as a name>`.
pub(crate) fn build_dir(tex_home: &Path, root: &Path, main: &Path) -> PathBuf {
    let tidy = |s: &str, with: char| -> String {
        s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { with }).collect()
    };
    let name = tidy(&root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into()), '-');
    let hash = quark_lab::run::digest(root.display().to_string().as_bytes());
    let main = tidy(&relative(root, main), '_');
    tex_home.join("builds").join(format!("{name}-{}", &hash[..10])).join(main)
}

/// The PDF, copied whole to where the dock reads it: a read under way keeps
/// the PDF before, and never sees one half written.
fn show(spec: &BuildSpec, made: &Path) -> Result<PathBuf, String> {
    let view = spec.dir.join("view");
    std::fs::create_dir_all(&view).map_err(|e| e.to_string())?;
    let name = made.file_name().unwrap_or_default();
    let target = view.join(name);
    let partial = view.join(format!(".{}.part", name.to_string_lossy()));
    std::fs::copy(made, &partial).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// latexmk, standing in: writes a PDF (a real one, copied from `@PDF@`,
    /// filled in by `bench` — a build's environment is its own), a log with an error and a warning, and a SyncTeX map
    /// putting line 3 of the main file on page 1 — or, with `$FAIL`, only
    /// the log.
    pub(crate) const LATEXMK: &str = r#"#!/bin/sh
out=""; main=""
for a in "$@"; do case "$a" in -outdir=*) out="${a#-outdir=}" ;; -*) ;; *) main="$a" ;; esac; done
stem=$(basename "$main" .tex)
dir=$(cd "$(dirname "$main")" && pwd -P)
printf '%s\n' "($dir/$stem.tex" "$dir/$stem.tex:3: Undefined control sequence." "l.3 \\foo" "" "LaTeX Warning: Reference \`sec:x' on page 1 undefined on input line 5." "Output written on $stem.pdf (2 pages)." > "$out/$stem.log"
if [ -n "$FAIL" ]; then exit 12; fi
cp "@PDF@" "$out/$stem.pdf"
printf 'SyncTeX Version:1\nInput:1:%s\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n{1\n[1,3:4736286,4736286:30000000,1000000,0\nh1,3:4736286,4736286:30000000,1000000,0\n]\n}1\nPostamble:\n' "$dir/$stem.tex" > "$out/$stem.synctex.gz"
"#;

    /// A TeX with the stand-in for latexmk, and a project with a main file.
    pub(crate) fn bench(name: &str) -> (PathBuf, Tools) {
        let base = std::env::temp_dir().join(format!("quark-texbuild-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bin = base.join("tex/TinyTeX/bin/universal-darwin");
        std::fs::create_dir_all(&bin).unwrap();
        let latexmk = LATEXMK.replace("@PDF@", &quark_pdf::testutil::sample_pdf().display().to_string());
        for (tool, body) in [("latexmk", latexmk.as_str()), ("pdflatex", "#!/bin/sh\n"), ("tlmgr", "#!/bin/sh\n")] {
            std::fs::write(bin.join(tool), body).unwrap();
            std::fs::set_permissions(bin.join(tool), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let root = base.join("paper");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.tex"), "\\documentclass{article}\n\\begin{document}\n\\foo\n\\section{Intro}\\label{sec:i}\n\\end{document}\n").unwrap();
        let marker = Marker {
            tinytex: "v1".into(),
            texlab: "v1".into(),
            bundle: Default::default(),
            bin,
            texlab_bin: base.join("tex/texlab"),
            size: 0,
            made: 0,
        };
        (root, Tools { marker, tex_home: base.join("tex"), person: base.clone(), sandbox: Kind::None })
    }

    pub(crate) fn finish(b: &mut Builder) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if b.poll() {
                return true;
            }
            if !b.running() {
                return false;
            }
            assert!(std::time::Instant::now() < deadline, "the build never ended");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_save_typesets_the_project_and_its_log_pdf_and_map_are_read() {
        let (root, tools) = bench("made");
        let mut b = Builder::default();
        b.compile(&root, Some(&root.join("main.tex")), &tools).unwrap();
        assert_eq!(b.last.as_ref().unwrap().state, Ended::Running);
        assert!(finish(&mut b));
        let last = b.last.clone().unwrap();
        assert_eq!(last.state, Ended::Done, "{last:?}");
        assert_eq!((last.main.as_deref(), last.engine), (Some("main.tex"), "pdfLaTeX"));
        assert_eq!((last.errors, last.warnings, last.version), (1, 1, 1));
        assert_eq!(last.pages, Some(2));
        let shown = last.pdf.clone().unwrap();
        assert!(shown.ends_with("view/main.pdf") && shown.is_file());
        assert!(shown.starts_with(tools.tex_home.join("builds")), "built outside the project: {shown:?}");
        assert!(!root.join("main.pdf").exists(), "nothing written into the project");
        // Each error a click from its line.
        let log = b.log.as_ref().unwrap();
        let error = log.entries.iter().find(|e| e.kind == quark_tex::log::Kind::Error).unwrap();
        assert_eq!((error.file.as_deref(), error.line), (Some("main.tex"), Some(3)));
        // SyncTeX both ways.
        let found = b.forward(&root, &root.join("main.tex"), 3).unwrap();
        assert_eq!(found.page, 1);
        let (file, line) = b.inverse(1, found.rects[0].x as f64 + 1.0, found.rects[0].y as f64 + 1.0).unwrap();
        assert_eq!((file.as_str(), line), ("main.tex", 3));
        // Again: the version moves, so the dock reads it again.
        b.compile(&root, None, &tools).unwrap();
        assert!(finish(&mut b));
        assert_eq!(b.last.as_ref().unwrap().version, 2);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_project_with_no_document_says_so_and_sources_are_known_by_name() {
        let (root, tools) = bench("nomain");
        std::fs::write(root.join("main.tex"), "\\section{Only a chapter}\n").unwrap();
        let mut b = Builder::default();
        b.compile(&root, None, &tools).unwrap();
        assert_eq!(b.last.as_ref().unwrap().state, Ended::NoMain);
        assert!(!b.running());
        assert!(is_source(Path::new("a/b.TEX")) && is_source(Path::new("refs.bib")));
        assert!(!is_source(Path::new("figure.png")) && !is_source(Path::new("Makefile")));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn builds_go_under_tex_s_folder_named_for_the_project_and_its_main_file() {
        let dir = build_dir(Path::new("/h/.quark/tex"), Path::new("/w/My Paper"), Path::new("/w/My Paper/ch/one.tex"));
        let s = dir.display().to_string();
        assert!(s.starts_with("/h/.quark/tex/builds/My-Paper-"), "{s}");
        assert!(s.ends_with("/ch_one_tex"), "{s}");
        assert_eq!(normalise("/a/./b/../c"), "/a/c");
    }
}
