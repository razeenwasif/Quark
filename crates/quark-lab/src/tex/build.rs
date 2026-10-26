//! A build: a LaTeX project's main file typeset by latexmk with Quark's
//! TinyTeX, in a folder of its own outside the project.
//!
//! ```text
//! out/          what TeX writes: the PDF, its log, the aux files, the
//!               .synctex.gz — and the project's folders, mirrored, so an
//!               \include'd chapter can write its own .aux
//! var/ texmf/   TeX's caches and a TEXMFHOME of its own
//! home/ tmp/    a home folder and a temporary one
//! latexmk.out   what latexmk said, and latexmk.err
//! build.json    its record: the file, the engine, the sandbox, the limits,
//!               and how it ended
//! ```
//!
//! TeX runs what a document says — `\write18` with shell escape on, and
//! `\openout` always — so a build is fenced as a Lab run is: no network,
//! nothing written outside its folder (the project's own sources least of
//! all), nothing of the home folder read but the project and Quark's TeX.
//! It is its own process group, with limits on its CPU time and on what it
//! writes, and an environment of its own. Shell escape is off unless the
//! project asks for it.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::run::{self, State};
use crate::sandbox::{self, Kind};

/// How long a build has after being asked to stop before it is made to.
const GRACE: Duration = Duration::from_secs(2);

/// The most a build may write to any one file.
const FILE_BYTES: u64 = 1 << 30;

/// CPU seconds a build has beyond its time.
const CPU_SPARE: u64 = 30;

/// How many of the project's folders are mirrored into `out/`, at most.
const MIRRORED: usize = 2000;

/// Which TeX engine latexmk drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    #[default]
    Pdflatex,
    Xelatex,
    Lualatex,
}

impl Engine {
    /// latexmk's flag for it.
    pub fn flag(self) -> &'static str {
        match self {
            Engine::Pdflatex => "-pdf",
            Engine::Xelatex => "-pdfxe",
            Engine::Lualatex => "-pdflua",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Engine::Pdflatex => "pdfLaTeX",
            Engine::Xelatex => "XeLaTeX",
            Engine::Lualatex => "LuaLaTeX",
        }
    }

    /// Read from what a `% !TEX program` line or a setting calls it.
    pub fn from_name(name: &str) -> Option<Engine> {
        match name.trim().to_ascii_lowercase().as_str() {
            "pdflatex" | "pdftex" | "latex" => Some(Engine::Pdflatex),
            "xelatex" | "xetex" => Some(Engine::Xelatex),
            "lualatex" | "luatex" => Some(Engine::Lualatex),
            _ => None,
        }
    }
}

/// What a build is given.
#[derive(Debug, Clone)]
pub struct BuildSpec {
    /// The project: read, never written.
    pub root: PathBuf,
    /// The file typeset, inside the project.
    pub main: PathBuf,
    /// The build's own folder: made, and everything it writes goes in it.
    pub dir: PathBuf,
    /// TinyTeX's programs.
    pub bin: PathBuf,
    /// Quark's TeX, `~/.quark/tex`: the one other tree a build reads.
    pub tex_home: PathBuf,
    /// The person's home folder, which the sandbox keeps it out of.
    pub home: PathBuf,
    pub engine: Engine,
    pub timeout: Duration,
    pub sandbox: Kind,
    /// Whether TeX may run programs (`\write18`): only when the project says.
    pub shell_escape: bool,
    /// Anything else a build may read: the person's fonts, when they allow it.
    pub reads: Vec<PathBuf>,
}

impl BuildSpec {
    fn stem(&self) -> String {
        self.main
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "main".into())
    }

    /// Where the PDF is, once made.
    pub fn pdf(&self) -> PathBuf {
        self.dir.join("out").join(format!("{}.pdf", self.stem()))
    }

    /// Where TeX's log is.
    pub fn log(&self) -> PathBuf {
        self.dir.join("out").join(format!("{}.log", self.stem()))
    }

    /// Where SyncTeX's map is.
    pub fn synctex(&self) -> PathBuf {
        self.dir
            .join("out")
            .join(format!("{}.synctex.gz", self.stem()))
    }
}

pub struct Build {
    dir: PathBuf,
    child: Option<Child>,
    started: Instant,
    deadline: Instant,
    /// When it was asked to stop, and why.
    stopping: Option<(Instant, State)>,
    state: State,
    exit: Option<i32>,
    seconds: f64,
    record: Value,
}

impl std::fmt::Debug for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Build")
            .field("dir", &self.dir)
            .field("state", &self.state)
            .finish()
    }
}

/// What a build's environment is: only this. Everything TeX writes on its
/// own account — caches, a home folder, configuration — is in the build's
/// folder; its log lines are not wrapped; it writes nowhere it was not
/// started, and runs nothing unless allowed.
pub fn environment(spec: &BuildSpec) -> Vec<(String, String)> {
    let dir = sandbox::real(&spec.dir);
    let at = |rel: &str| dir.join(rel).display().to_string();
    vec![
        (
            "PATH".into(),
            format!("{}:/usr/bin:/bin", spec.bin.display()),
        ),
        ("HOME".into(), at("home")),
        ("TMPDIR".into(), at("tmp")),
        ("TEXMFVAR".into(), at("var")),
        ("TEXMFCONFIG".into(), at("var/config")),
        ("TEXMFHOME".into(), at("texmf")),
        ("TEXMFOUTPUT".into(), at("out")),
        ("XDG_CACHE_HOME".into(), at("var/cache")),
        ("max_print_line".into(), "10000".into()),
        ("error_line".into(), "254".into()),
        ("half_error_line".into(), "238".into()),
        ("openout_any".into(), "p".into()),
        (
            "shell_escape".into(),
            if spec.shell_escape { "t" } else { "f" }.into(),
        ),
        ("LANG".into(), "en_US.UTF-8".into()),
    ]
}

/// latexmk's arguments: the engine, SyncTeX, never stopping to ask, file
/// and line on every error, on past errors to a PDF where it can, and
/// everything written to the build's own `out/`. No latexmkrc is read: one
/// is a Perl program.
pub fn arguments(spec: &BuildSpec) -> Vec<String> {
    let out = sandbox::real(&spec.dir).join("out").display().to_string();
    vec![
        "-norc".into(),
        "-cd".into(),
        spec.engine.flag().into(),
        "-synctex=1".into(),
        "-interaction=nonstopmode".into(),
        "-file-line-error".into(),
        "-f".into(),
        format!("-outdir={out}"),
        format!("-auxdir={out}"),
        sandbox::real(&spec.main).display().to_string(),
    ]
}

/// The project's folders, made again under `out/`: TeX writes an included
/// file's `.aux` beside where its source is, relative to the output folder.
fn mirror(root: &Path, out: &Path) {
    let mut made = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let skip = name.to_string_lossy().starts_with('.')
                || ["node_modules", "target", "build"].contains(&name.to_string_lossy().as_ref());
            if skip || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if made >= MIRRORED {
                return;
            }
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(root) {
                let _ = std::fs::create_dir_all(out.join(rel));
                made += 1;
            }
            stack.push(path);
        }
    }
}

fn pretty(record: &Value) -> String {
    serde_json::to_string_pretty(record).unwrap_or_default() + "\n"
}

impl Build {
    /// Make the build's folder and start latexmk on the main file.
    pub fn start(spec: &BuildSpec) -> Result<Build, String> {
        let dir = &spec.dir;
        if !spec.main.starts_with(&spec.root) {
            return Err(format!("{} is not in the project", spec.main.display()));
        }
        for sub in ["out", "var/config", "var/cache", "texmf", "home", "tmp"] {
            std::fs::create_dir_all(dir.join(sub))
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        mirror(&spec.root, &dir.join("out"));
        // The PDF an earlier build made goes: one there after this build is
        // this build's.
        let _ = std::fs::remove_file(spec.pdf());
        let stdout = File::create(dir.join("latexmk.out")).map_err(|e| e.to_string())?;
        let stderr = File::create(dir.join("latexmk.err")).map_err(|e| e.to_string())?;

        let cpu = spec.timeout.as_secs() + CPU_SPARE;
        let mut reads = vec![spec.root.clone(), spec.tex_home.clone()];
        reads.extend(spec.reads.iter().cloned());
        let mut record = json!({
            "main": spec.main.display().to_string(),
            "root": spec.root.display().to_string(),
            "engine": spec.engine,
            "shell_escape": spec.shell_escape,
            "sandbox": {
                "kind": spec.sandbox,
                "network": spec.sandbox == Kind::None,
                "writes": if spec.sandbox == Kind::None { "anywhere the person can" } else { "its own folder" },
                "reads": reads.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            },
            "limits": { "seconds": spec.timeout.as_secs(), "cpu_seconds": cpu, "file_bytes": FILE_BYTES },
            "started": run::now(),
            "state": State::Running,
        });

        // The limits are the shell's to set, then it becomes latexmk.
        let script = format!(
            "ulimit -t {cpu} 2>/dev/null; ulimit -f {} 2>/dev/null; exec \"$0\" \"$@\"",
            FILE_BYTES / 512
        );
        let mut args = vec![
            "-c".to_string(),
            script,
            spec.bin.join("latexmk").display().to_string(),
        ];
        args.extend(arguments(spec));
        let (program, args) = sandbox::wrap_reading(
            spec.sandbox,
            dir,
            &reads,
            &spec.home,
            Path::new("/bin/sh"),
            &args,
        );
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(sandbox::real(dir))
            .env_clear()
            .envs(environment(spec))
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
        record["pid"] = json!(child.id());
        let started = Instant::now();
        let build = Build {
            dir: dir.clone(),
            child: Some(child),
            started,
            deadline: started + spec.timeout,
            stopping: None,
            state: State::Running,
            exit: None,
            seconds: 0.0,
            record,
        };
        // Unrecorded, it does not run: dropped here, it is stopped.
        run::write(&dir.join("build.json"), &pretty(&build.record))?;
        Ok(build)
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn exit(&self) -> Option<i32> {
        self.exit
    }

    pub fn seconds(&self) -> f64 {
        if self.state == State::Running {
            self.started.elapsed().as_secs_f64()
        } else {
            self.seconds
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn signal(&self, kill: bool) {
        if let Some(child) = &self.child {
            run::signal_group(child.id(), kill);
        }
    }

    /// Ask it to stop: politely, then not.
    pub fn stop(&mut self) {
        if self.state == State::Running && self.stopping.is_none() {
            self.stopping = Some((Instant::now(), State::Stopped));
            self.signal(false);
        }
    }

    /// Whether it has ended; ends it if its time is up. `true` once, when it ends.
    pub fn poll(&mut self) -> bool {
        if self.state != State::Running {
            return false;
        }
        if self.stopping.is_none() && Instant::now() >= self.deadline {
            self.stopping = Some((Instant::now(), State::TimedOut));
            self.signal(false);
        }
        if let Some((asked, _)) = self.stopping {
            if asked.elapsed() >= GRACE {
                self.signal(true);
            }
        }
        let Some(child) = &mut self.child else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                // What it started goes with it.
                self.signal(true);
                self.child = None;
                self.exit = status.code();
                self.seconds = self.started.elapsed().as_secs_f64();
                self.state = match (self.stopping, status.code()) {
                    (Some((_, why)), _) => why,
                    (None, Some(0)) => State::Done,
                    _ => State::Failed,
                };
                self.finish();
                true
            }
            Ok(None) | Err(_) => false,
        }
    }

    fn finish(&mut self) {
        self.record["state"] = json!(self.state);
        self.record["exit"] = json!(self.exit);
        self.record["ended"] = json!(run::now());
        self.record["seconds"] = json!((self.seconds * 100.0).round() / 100.0);
        let _ = run::write(&self.dir.join("build.json"), &pretty(&self.record));
    }

    /// What latexmk printed last, to say why a build that left no log failed.
    pub fn said(&self) -> String {
        let (err, _) = run::tail(&self.dir.join("latexmk.err"), 12);
        let (out, _) = run::tail(&self.dir.join("latexmk.out"), 12);
        [out.trim(), err.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for Build {
    /// A build let go of is stopped: nothing it started outlives it.
    fn drop(&mut self) {
        if self.state == State::Running {
            self.signal(true);
            if let Some(child) = &mut self.child {
                let _ = child.kill();
                let _ = child.wait();
            }
            self.child = None;
            self.state = State::Stopped;
            self.seconds = self.started.elapsed().as_secs_f64();
            self.finish();
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in latexmk: it writes what it was asked, its environment and
    /// where it ran to `$CALLS`, then a PDF, a log and a SyncTeX map where
    /// `-outdir` says — or, given FAIL, an error in the log.
    pub const LATEXMK: &str = r#"#!/bin/sh
echo "latexmk $*|$PWD|$HOME|$TEXMFVAR|$openout_any|$shell_escape|$max_print_line" >> "$CALLS"
out=""; main=""
for a in "$@"; do case "$a" in -outdir=*) out="${a#-outdir=}" ;; -*) ;; *) main="$a" ;; esac; done
stem=$(basename "$main" .tex)
if [ -n "$SLOW" ]; then sleep 300 & echo $! > "$CALLS.child"; wait; fi
if [ -n "$FAIL" ]; then printf '%s\n' "./$stem.tex:3: Undefined control sequence." "l.3 \\foo" > "$out/$stem.log"; exit 12; fi
printf '%%PDF-1.5\n%%stand-in\n' > "$out/$stem.pdf"
printf 'This is pdfTeX\nOutput written on %s.pdf (1 page).\n' "$stem" > "$out/$stem.log"
printf 'SyncTeX Version:1\n' | gzip > "$out/$stem.synctex.gz"
"#;

    pub(crate) fn fake_bin(at: &Path) -> PathBuf {
        let bin = at.join("TinyTeX/bin/universal-darwin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, body) in [
            ("latexmk", LATEXMK),
            ("pdflatex", "#!/bin/sh\n"),
            ("tlmgr", "#!/bin/sh\n"),
        ] {
            std::fs::write(bin.join(name), body).unwrap();
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        bin
    }

    struct Bench {
        base: PathBuf,
        spec: BuildSpec,
        calls: PathBuf,
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn bench(tag: &str, sandbox: Kind) -> Bench {
        let base =
            std::env::temp_dir().join(format!("quark-tex-build-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, tex_home) = (base.join("paper"), base.join("tex"));
        std::fs::create_dir_all(root.join("chapters/one")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\nHi.\n\\end{document}\n",
        )
        .unwrap();
        let bin = fake_bin(&tex_home);
        let spec = BuildSpec {
            root: root.clone(),
            main: root.join("main.tex"),
            dir: tex_home.join("builds/paper-1/main"),
            bin,
            tex_home,
            home: base.join("person"),
            engine: Engine::Pdflatex,
            timeout: Duration::from_secs(20),
            sandbox,
            shell_escape: false,
            reads: Vec::new(),
        };
        Bench {
            calls: base.join("calls.txt"),
            base,
            spec,
        }
    }

    fn finish(build: &mut Build) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !build.poll() {
            assert!(Instant::now() < deadline, "the build never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The stand-in latexmk is told where to write what it was asked.
    fn with_calls(spec: &BuildSpec, calls: &Path, more: &str) {
        let script = spec.bin.join("latexmk");
        let body = LATEXMK.replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\nCALLS={}\n{more}\n", calls.display()),
            1,
        );
        std::fs::write(&script, body).unwrap();
    }

    #[test]
    fn a_build_runs_latexmk_with_the_engine_synctex_and_its_own_folder_and_makes_a_pdf() {
        let b = bench("made", Kind::None);
        with_calls(&b.spec, &b.calls, "");
        let mut build = Build::start(&b.spec).unwrap();
        finish(&mut build);
        assert_eq!(build.state(), State::Done, "{}", build.said());
        assert!(b.spec.pdf().is_file() && b.spec.log().is_file() && b.spec.synctex().is_file());
        let call = std::fs::read_to_string(&b.calls).unwrap();
        let fields: Vec<&str> = call.trim().split('|').collect();
        let out = sandbox::real(&b.spec.dir).join("out").display().to_string();
        assert_eq!(
            fields[0],
            format!("latexmk -norc -cd -pdf -synctex=1 -interaction=nonstopmode -file-line-error -f -outdir={out} -auxdir={out} {}", sandbox::real(&b.spec.main).display())
        );
        let dir = sandbox::real(&b.spec.dir);
        assert_eq!(
            fields[2],
            dir.join("home").display().to_string(),
            "a home folder of its own"
        );
        assert_eq!(
            fields[3],
            dir.join("var").display().to_string(),
            "TeX's caches in the build's folder"
        );
        assert_eq!(
            &fields[4..],
            ["p", "f", "10000"],
            "no writing elsewhere, no shell escape, no wrapped lines"
        );
        assert!(
            b.spec.dir.join("out/chapters/one").is_dir() && !b.spec.dir.join("out/.git").exists(),
            "the project's folders are mirrored, hidden ones not"
        );
        let record: Value =
            serde_json::from_str(&std::fs::read_to_string(b.spec.dir.join("build.json")).unwrap())
                .unwrap();
        assert_eq!(
            (record["state"].as_str(), record["engine"].as_str()),
            (Some("done"), Some("pdflatex"))
        );
    }

    #[test]
    fn a_failed_build_says_so_and_a_stopped_one_stops_everything_it_started() {
        let b = bench("failed", Kind::None);
        with_calls(&b.spec, &b.calls, "FAIL=1");
        let mut build = Build::start(&b.spec).unwrap();
        finish(&mut build);
        assert_eq!((build.state(), build.exit()), (State::Failed, Some(12)));
        assert!(std::fs::read_to_string(b.spec.log())
            .unwrap()
            .contains("Undefined control sequence"));

        with_calls(&b.spec, &b.calls, "SLOW=1");
        let mut slow = Build::start(&b.spec).unwrap();
        let child = PathBuf::from(format!("{}.child", b.calls.display()));
        let deadline = Instant::now() + Duration::from_secs(10);
        let pid: i32 = loop {
            if let Some(pid) = std::fs::read_to_string(&child)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        slow.stop();
        finish(&mut slow);
        assert_eq!(slow.state(), State::Stopped);
        std::thread::sleep(Duration::from_millis(100));
        // SAFETY: signal 0 only asks whether the process exists.
        assert!(
            unsafe { libc::kill(pid, 0) } != 0,
            "what latexmk started outlived the stop"
        );
    }

    #[test]
    fn a_main_file_outside_the_project_is_not_built() {
        let b = bench("outside", Kind::None);
        let mut spec = b.spec.clone();
        spec.main = std::env::temp_dir().join("elsewhere.tex");
        assert!(Build::start(&spec)
            .unwrap_err()
            .contains("not in the project"));
    }

    #[test]
    fn in_the_sandbox_a_build_cannot_write_into_the_project() {
        let kind = sandbox::available();
        if kind == Kind::None {
            return;
        }
        let b = bench("fenced", kind);
        // The project, under a stand-in home folder the sandbox keeps it out of.
        let script = b.spec.bin.join("latexmk");
        let target = b.spec.root.join("written.txt");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho no > {} && echo wrote\nexit 0\n",
                target.display()
            ),
        )
        .unwrap();
        let mut spec = b.spec.clone();
        spec.home = b.base.clone();
        let mut build = Build::start(&spec).unwrap();
        finish(&mut build);
        assert!(
            !target.exists(),
            "a build wrote into the project: {}",
            build.said()
        );
    }
}
