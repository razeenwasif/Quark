//! Making TeX: exactly what a plan said, into `~/.quark/tex`.
//!
//! ```text
//! downloads/   TinyTeX and texlab as GitHub has them, checked against
//!              their digests, deleted once unpacked
//! TinyTeX/     TeX Live, made small: its programs in bin/<arch>/
//! texlab/      the language server
//! smoke/       a first document, typeset in the sandbox
//! builds/      every project's builds, later
//! .home/       the home folder every step is given
//! ready.json   written last: what a ready TeX is
//! ```
//!
//! Each step is a program run as its own process group, with none of the
//! person's environment but what a download needs and a home folder of its
//! own, so nothing is written to `~/.texlive` or `~/Library`, and a cancel
//! stops everything a step started. A making cancelled or failed leaves
//! nothing behind; one cut short by Quark closing is taken away the next
//! time Quark looks.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::build::{Build, BuildSpec, Engine};
use super::{bin_of, Marker, TexEnv, TexPlan, TexState, BUSY, MARKER};
use crate::disk::free_bytes;
use crate::job::{How, Job, Said};
use crate::run::{self, State};

/// Lines of a step's output kept to show.
const KEPT: usize = 60;

/// How often what a step has downloaded is measured.
const MEASURE: Duration = Duration::from_secs(1);

/// While a making is under way: the process it is running, to stop if
/// Quark is gone before it is done.
const MAKING: &str = "making.json";

const TINYTEX_FILE: &str = "tinytex.tar.xz";
const TEXLAB_FILE: &str = "texlab.tar.gz";

/// What a made TeX is tried on: a page with a little of everything a
/// document starts with.
const SMOKE: &str = "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n\\section{Quark}\nHello, \\TeX: $e^{i\\pi} + 1 = 0$.\n\\end{document}\n";

/// How long that may take: formats are read from disk for the first time.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(300);

/// Where a making is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// What an earlier making left, taken away first.
    Clear,
    Download,
    Verify,
    Unpack,
    Latexmk,
    TexlabDownload,
    TexlabVerify,
    TexlabUnpack,
    Smoke,
    Finish,
}

impl Step {
    /// The stage the card shows.
    fn stage(self) -> &'static str {
        match self {
            Step::Clear | Step::Download | Step::Verify | Step::Unpack => "tinytex",
            Step::Latexmk => "latexmk",
            Step::TexlabDownload | Step::TexlabVerify | Step::TexlabUnpack => "texlab",
            Step::Smoke => "checking",
            Step::Finish => "finishing",
        }
    }

    /// What it runs, as a failure names it.
    fn program(self) -> &'static str {
        match self {
            Step::Download | Step::TexlabDownload => "curl",
            Step::Unpack | Step::TexlabUnpack => "tar",
            Step::Latexmk => "tlmgr",
            Step::Smoke => "latexmk",
            Step::Clear | Step::Verify | Step::TexlabVerify | Step::Finish => "Quark",
        }
    }
}

/// What a step is doing.
enum Work {
    Job(Job),
    Thread(Receiver<Result<(), String>>),
    Build(Box<Build>, BuildSpec),
}

pub(super) struct Making {
    plan: TexPlan,
    step: Step,
    work: Work,
    log: Vec<String>,
    measured: Option<Instant>,
    /// Bytes on disk of the step's download, of what the plan said it is.
    progress: (u64, u64),
}

impl Making {
    fn note(&mut self, line: &str) {
        // A progress bar redrawn in place: its last state.
        let line = line.rsplit('\r').next().unwrap_or(line).trim_end();
        if line.trim().is_empty() {
            return;
        }
        self.log.push(line.to_string());
        if self.log.len() > KEPT {
            self.log.remove(0);
        }
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// The bytes of the files under `path`, links not followed, looking at no
/// more than `budget` entries.
fn size_of(path: &Path, budget: &mut usize) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        total += size_of(&entry.path(), budget);
    }
    total
}

/// Take TeX's folder away, whole — never one that is the person's home
/// folder or holds it.
fn take_away(home: &Path, person: &Path) {
    if home.is_absolute() && home.components().count() >= 4 && !person.starts_with(home) {
        let _ = std::fs::remove_dir_all(home);
    }
}

/// Whether `file` is what its source published a digest of, if it did.
fn verify(file: &Path, want: Option<&str>, what: &str) -> Result<(), String> {
    let Some(want) = want else { return Ok(()) };
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let got = run::digest(&bytes);
    if got.eq_ignore_ascii_case(want) {
        Ok(())
    } else {
        Err(format!("{what}'s download does not match the SHA-256 GitHub published for it ({got}, not {want}), so it was not unpacked."))
    }
}

/// Why a step's program failed: how it ended, and the last thing it said.
fn failed(step: Step, code: Option<i32>, log: &[String]) -> String {
    let how = match code {
        Some(c) => format!("{} stopped with exit status {c}", step.program()),
        None => format!("{} was stopped", step.program()),
    };
    match log.iter().rev().find(|l| !l.starts_with("$ ")) {
        Some(last) => format!("{how}: {last}"),
        None => format!("{how}."),
    }
}

/// Whether the made TeX typeset its first document: a PDF, and SyncTeX's map.
fn smoke_result(build: &Build, spec: &BuildSpec) -> Result<(), String> {
    match build.state() {
        State::Done if spec.pdf().is_file() && spec.synctex().is_file() => Ok(()),
        State::TimedOut => Err(format!(
            "TeX took more than {} seconds to typeset a first page.",
            SMOKE_TIMEOUT.as_secs()
        )),
        _ => {
            let log = std::fs::read_to_string(spec.log()).unwrap_or_default();
            let error = log
                .lines()
                .find(|l| l.starts_with('!') || l.contains(".tex:"))
                .map(str::to_string);
            Err(format!(
                "TeX could not typeset a first page: {}",
                error.unwrap_or_else(|| build.said())
            ))
        }
    }
}

/// What a made TeX ends with: what was downloaded only to be unpacked
/// deleted, its size measured, and its marker written last.
fn finish(home: &Path, mut marker: Marker) -> Result<(), String> {
    for downloaded in ["downloads", "smoke"] {
        let _ = std::fs::remove_dir_all(home.join(downloaded));
    }
    marker.size = size_of(home, &mut usize::MAX.clone());
    marker.made = run::now();
    if !marker.holds(home) {
        return Err("What was made is not all where it should be.".into());
    }
    run::write(
        &home.join(MARKER),
        &(serde_json::to_string_pretty(&marker).unwrap_or_default() + "\n"),
    )?;
    let _ = std::fs::remove_file(home.join(MAKING));
    Ok(())
}

/// How far a step's download has got, as bytes on disk, of what the plan
/// said it would be.
fn measure(home: &Path, making: &Making) -> (u64, u64) {
    let planned = |i: usize| making.plan.downloads.get(i).map_or(0, |d| d.bytes);
    let (path, total) = match making.step {
        Step::Download => (home.join("downloads").join(TINYTEX_FILE), planned(0)),
        Step::TexlabDownload => (home.join("downloads").join(TEXLAB_FILE), planned(2)),
        _ => return (0, 0),
    };
    let done = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    (done.min(total), total)
}

/// What a Quark gone before its making was done left: the step it was
/// running stopped, if it is still going in TeX's folder. Whether there
/// was a making to take away.
pub(super) fn reap(home: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(home.join(MAKING)) else {
        return false;
    };
    #[cfg(unix)]
    if let Some(pid) = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["pid"].as_u64())
    {
        run::stop_group_in(pid as u32, home, true);
    }
    #[cfg(not(unix))]
    let _ = text;
    true
}

impl TexEnv {
    /// The second say-so: make exactly what was planned.
    pub fn create(&mut self) -> Result<(), String> {
        if self.busy() {
            return Err(BUSY.into());
        }
        let TexState::Planned { plan } = &self.state else {
            return Err("There is no plan to install TeX from.".into());
        };
        let plan = plan.clone();
        let needs = self.needs();
        if !needs.is_empty() {
            return Err(needs.join("; "));
        }
        let free = free_bytes(&self.home);
        if free < plan.disk {
            return Err(format!(
                "There is not room for it: {} is free, and installing it takes up to {}.",
                gb(free),
                gb(plan.disk)
            ));
        }
        if !self.reaped {
            self.reaped = true;
            reap(&self.home);
        }
        self.making = Some(Making {
            plan,
            step: Step::Clear,
            work: Work::Thread(channel().1),
            log: Vec::new(),
            measured: None,
            progress: (0, 0),
        });
        let begun = if self.home.exists() {
            let (home, person) = (self.home.clone(), self.person.clone());
            self.thread(Step::Clear, move || {
                take_away(&home, &person);
                Ok(())
            });
            Ok(())
        } else {
            self.download(Step::Download)
        };
        match begun {
            Ok(()) => {
                self.show();
                Ok(())
            }
            Err(why) => {
                self.making = None;
                Err(why)
            }
        }
    }

    /// Take TeX away: its marker first, so nothing takes it for ready, then
    /// its folder — every project's builds with it.
    pub fn remove(&mut self) -> Result<(), String> {
        if self.busy() {
            return Err(BUSY.into());
        }
        let _ = std::fs::remove_file(self.home.join(MARKER));
        self.clear_after(None);
        self.set(TexState::Removing);
        Ok(())
    }

    /// Quark is closing: stop the step being run. What the making left is
    /// taken away the next time Quark looks.
    pub fn quit(&mut self) {
        self.planning = None;
        self.making = None;
    }

    /// A making stopped: what it was running with it, and its folder taken away.
    pub(super) fn stop_making(&mut self) {
        let Some(Making { work, .. }) = self.making.take() else {
            return;
        };
        let before = match work {
            Work::Thread(rx) => Some(rx),
            other => {
                drop(other);
                None
            }
        };
        self.clear_after(before);
        self.set(TexState::Removing);
    }

    /// Take TeX's folder away on a thread — once `before`, a step's thread
    /// that may still be writing in it, is done.
    pub(super) fn clear_after(&mut self, before: Option<Receiver<Result<(), String>>>) {
        let (home, person) = (self.home.clone(), self.person.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Some(before) = before {
                let _ = before.recv();
            }
            take_away(&home, &person);
            let _ = tx.send(());
        });
        self.clearing = Some(rx);
    }

    pub(super) fn poll_clearing(&mut self) -> bool {
        let Some(rx) = &self.clearing else {
            return false;
        };
        if let Err(TryRecvError::Empty) = rx.try_recv() {
            return false;
        }
        self.clearing = None;
        if matches!(self.state, TexState::Removing | TexState::Unknown) {
            self.missing();
        }
        true
    }

    fn fail(&mut self, why: String) {
        let Some(Making {
            step, log, work, ..
        }) = self.making.take()
        else {
            return;
        };
        let before = match work {
            Work::Thread(rx) => Some(rx),
            other => {
                drop(other);
                None
            }
        };
        self.clear_after(before);
        self.set(TexState::Failed {
            step: step.stage().into(),
            why,
            log,
        });
    }

    /// The card's view of the making.
    fn show(&mut self) {
        let Some(making) = &self.making else { return };
        let state = TexState::Making {
            stage: making.step.stage().into(),
            done: making.progress.0,
            total: making.progress.1,
            log: making.log.clone(),
        };
        self.set(state);
    }

    /// What every step runs with: TeX's own programs, a home folder of its
    /// own, and a PATH to the tools — nothing of the person's.
    fn environment(&self) -> Vec<(String, String)> {
        let mut path: Vec<PathBuf> = bin_of(&self.home).into_iter().collect();
        for tool in [
            &self.tools.curl,
            &self.tools.tar,
            &self.tools.perl,
            &self.tools.xz,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(dir) = tool.parent().filter(|d| d.is_absolute()) {
                path.push(dir.to_path_buf());
            }
        }
        path.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
        let mut seen = std::collections::HashSet::new();
        path.retain(|p| seen.insert(p.clone()));
        let path = std::env::join_paths(path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut env = vec![
            (
                "HOME".to_string(),
                self.home.join(".home").display().to_string(),
            ),
            ("PATH".into(), path),
        ];
        env.extend(self.env.iter().cloned());
        env
    }

    /// Start a step's program, in `dir`.
    fn run(&mut self, step: Step, program: &Path, args: &[&str], dir: &Path) -> Result<(), String> {
        let own_home = self.home.join(".home");
        for folder in [dir, own_home.as_path()] {
            std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let how = How {
            group: true,
            clean: true,
            dir: Some(dir.to_path_buf()),
        };
        let job = Job::start_with(program, &args, &self.environment(), how)?;
        let _ = run::write(
            &self.home.join(MAKING),
            &json!({ "pid": job.id(), "step": step.stage() }).to_string(),
        );
        let making = self.making.as_mut().expect("a making");
        let name = program
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        making.note(&format!("$ {name} {}", args.join(" ")));
        making.step = step;
        making.work = Work::Job(job);
        making.measured = None;
        making.progress = (0, 0);
        Ok(())
    }

    /// Do a step's work on a thread.
    fn thread(&mut self, step: Step, work: impl FnOnce() -> Result<(), String> + Send + 'static) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        let making = self.making.as_mut().expect("a making");
        making.step = step;
        making.work = Work::Thread(rx);
        making.progress = (0, 0);
    }

    /// Download TinyTeX, or texlab, with curl.
    fn download(&mut self, step: Step) -> Result<(), String> {
        let curl = self
            .tools
            .curl
            .clone()
            .ok_or("There is no curl to download TeX with.")?;
        let plan = &self.making.as_ref().expect("a making").plan;
        let (url, file) = match step {
            Step::TexlabDownload => (plan.texlab_url.clone(), TEXLAB_FILE),
            _ => (plan.tinytex_url.clone(), TINYTEX_FILE),
        };
        // -q first: no .curlrc is read.
        let args = [
            "-q",
            "-fsSL",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--retry",
            "3",
            "--connect-timeout",
            "30",
            "--speed-limit",
            "1000",
            "--speed-time",
            "60",
            "-o",
            file,
            url.as_str(),
        ];
        self.run(step, &curl, &args, &self.home.join("downloads"))
    }

    /// Typeset a first document with what was made, in the sandbox builds
    /// will run in.
    fn smoke(&mut self) -> Result<(), String> {
        let sandbox = self.sandbox();
        let bin = bin_of(&self.home).ok_or("TinyTeX's programs are not where they should be.")?;
        let src = self.home.join("smoke").join("src");
        std::fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        run::write(&src.join("main.tex"), SMOKE)?;
        let spec = BuildSpec {
            root: src.clone(),
            main: src.join("main.tex"),
            dir: self.home.join("smoke").join("build"),
            bin,
            tex_home: self.home.clone(),
            home: self.person.clone(),
            engine: Engine::Pdflatex,
            timeout: SMOKE_TIMEOUT,
            sandbox,
            shell_escape: false,
            reads: Vec::new(),
        };
        let build = Build::start(&spec)?;
        let making = self.making.as_mut().expect("a making");
        making.note("$ latexmk -pdf main.tex, in the sandbox");
        making.step = Step::Smoke;
        making.progress = (0, 0);
        making.work = Work::Build(Box::new(build), spec);
        Ok(())
    }

    /// The step that ended well: on to the next.
    fn next(&mut self) -> Result<(), String> {
        let making = self.making.as_ref().expect("a making");
        let (plan, step) = (making.plan.clone(), making.step);
        let home = self.home.clone();
        let downloads = home.join("downloads");
        match step {
            Step::Clear => self.download(Step::Download),
            Step::Download => {
                let (file, want) = (downloads.join(TINYTEX_FILE), plan.tinytex_sha256.clone());
                self.thread(Step::Verify, move || {
                    verify(&file, want.as_deref(), "TinyTeX")
                });
                Ok(())
            }
            Step::Verify => {
                let tar = self
                    .tools
                    .tar
                    .clone()
                    .ok_or("There is no tar to unpack TinyTeX with.")?;
                let into = home.display().to_string();
                self.run(
                    Step::Unpack,
                    &tar,
                    &["-xJf", TINYTEX_FILE, "-C", &into],
                    &downloads,
                )
            }
            Step::Unpack => {
                let bin = bin_of(&home).ok_or("TinyTeX's download had no programs in it.")?;
                let _ = std::fs::remove_file(downloads.join(TINYTEX_FILE));
                if bin.join("latexmk").exists() {
                    return self.download(Step::TexlabDownload);
                }
                self.run(
                    Step::Latexmk,
                    &bin.join("tlmgr"),
                    &["install", "latexmk"],
                    &home,
                )
            }
            Step::Latexmk => self.download(Step::TexlabDownload),
            Step::TexlabDownload => {
                let (file, want) = (downloads.join(TEXLAB_FILE), plan.texlab_sha256.clone());
                self.thread(Step::TexlabVerify, move || {
                    verify(&file, want.as_deref(), "texlab")
                });
                Ok(())
            }
            Step::TexlabVerify => {
                let tar = self
                    .tools
                    .tar
                    .clone()
                    .ok_or("There is no tar to unpack texlab with.")?;
                let into = home.join("texlab");
                std::fs::create_dir_all(&into).map_err(|e| format!("{}: {e}", into.display()))?;
                let into = into.display().to_string();
                self.run(
                    Step::TexlabUnpack,
                    &tar,
                    &["-xzf", TEXLAB_FILE, "-C", &into],
                    &downloads,
                )
            }
            Step::TexlabUnpack => {
                if !home.join("texlab").join("texlab").is_file() {
                    return Err("texlab's download had no texlab in it.".into());
                }
                self.smoke()
            }
            Step::Smoke => {
                let bin = bin_of(&home).unwrap_or_default();
                let marker = Marker {
                    tinytex: plan.tinytex.clone(),
                    texlab: plan.texlab.clone(),
                    bundle: plan.bundle,
                    bin,
                    texlab_bin: home.join("texlab").join("texlab"),
                    size: 0,
                    made: 0,
                };
                self.thread(Step::Finish, move || finish(&home, marker));
                Ok(())
            }
            Step::Finish => {
                self.making = None;
                self.check();
                Ok(())
            }
        }
    }

    /// Take what the making's step has done; on to the next when it ends.
    pub(super) fn poll_making(&mut self) -> bool {
        let Some(making) = self.making.as_mut() else {
            return false;
        };
        let mut changed = false;
        let mut ended = None;
        let said = match &mut making.work {
            Work::Job(job) => job.poll(),
            _ => Vec::new(),
        };
        for s in said {
            changed = true;
            match s {
                Said::Out(line) | Said::Err(line) => making.note(&line),
                Said::Exited(Some(0)) => ended = Some(Ok(())),
                Said::Exited(code) => ended = Some(Err(failed(making.step, code, &making.log))),
            }
        }
        match &mut making.work {
            Work::Thread(rx) => match rx.try_recv() {
                Ok(result) => ended = Some(result),
                Err(TryRecvError::Disconnected) => {
                    ended = Some(Err(format!(
                        "{} stopped unexpectedly.",
                        making.step.program()
                    )))
                }
                Err(TryRecvError::Empty) => {}
            },
            Work::Build(build, spec) => {
                if build.poll() {
                    ended = Some(smoke_result(build, spec));
                }
            }
            Work::Job(_) => {}
        }
        if ended.is_none() && making.measured.is_none_or(|t| t.elapsed() >= MEASURE) {
            making.measured = Some(Instant::now());
            let progress = measure(&self.home, making);
            if progress != making.progress {
                making.progress = progress;
                changed = true;
            }
        }
        match ended {
            Some(Ok(())) => {
                match self.next() {
                    Ok(()) => self.show(),
                    Err(why) => self.fail(why),
                }
                true
            }
            Some(Err(why)) => {
                self.fail(why);
                true
            }
            None => {
                if changed {
                    self.show();
                }
                changed
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::github::tests::Script;
    use super::super::tests::{made, MAC};
    use super::super::{Bundle, TexDownload, Tools};
    use super::*;
    use crate::sandbox::Kind;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::Arc;

    fn script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Each stand-in writes what it was asked, and its HOME, to $CALLS.
    const CURL: &str = r#"echo "curl $*|$HOME" >> "$CALLS"
out=""; url=""; while [ $# -gt 0 ]; do if [ "$1" = "-o" ]; then shift; out="$1"; fi; url="$1"; shift; done
if [ -n "$SLOW" ]; then sleep 300 & echo $! > "$CALLS.child"; wait; fi
case "$url" in *texlab*) cp "$TEXLAB_TARBALL" "$out" ;; *) cp "$TINYTEX_TARBALL" "$out" ;; esac"#;

    /// tlmgr installs latexmk: the stand-in latexmk, beside it.
    const TLMGR: &str = r#"echo "tlmgr $*|$HOME" >> "$CALLS"
cp "$FAKES/latexmk" "$(dirname "$0")/latexmk""#;

    struct Bench {
        base: PathBuf,
        home: PathBuf,
        person: PathBuf,
        calls: PathBuf,
        tinytex_sha: String,
        texlab_sha: String,
        env: Vec<(String, String)>,
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    /// A person's home with nothing in it, stand-ins for curl and for what
    /// TinyTeX's and texlab's tarballs hold, and the tarballs.
    fn bench(tag: &str, with_latexmk: bool) -> Bench {
        let base = std::env::temp_dir().join(format!("quark-texmake-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (fakes, person) = (base.join("fakes"), base.join("person"));
        std::fs::create_dir_all(&fakes).unwrap();
        std::fs::create_dir_all(&person).unwrap();
        let calls = base.join("calls.txt");
        script(&fakes.join("curl"), CURL);
        std::fs::write(
            fakes.join("latexmk"),
            super::super::build::tests::LATEXMK.replacen(
                "#!/bin/sh\n",
                &format!("#!/bin/sh\nCALLS={}\n", calls.display()),
                1,
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            fakes.join("latexmk"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        // TinyTeX as GitHub has it: TinyTeX/bin/universal-darwin/…, in an xz tarball.
        let tree = base.join("tree");
        let bin = tree.join("TinyTeX/bin/universal-darwin");
        std::fs::create_dir_all(&bin).unwrap();
        script(&bin.join("tlmgr"), TLMGR);
        script(&bin.join("pdflatex"), "exit 0");
        if with_latexmk {
            std::fs::copy(fakes.join("latexmk"), bin.join("latexmk")).unwrap();
        }
        let tinytex = base.join("TinyTeX.tar.xz");
        assert!(Command::new("/usr/bin/tar")
            .arg("-cJf")
            .arg(&tinytex)
            .arg("-C")
            .arg(&tree)
            .arg("TinyTeX")
            .status()
            .unwrap()
            .success());
        let lab = base.join("lab");
        std::fs::create_dir_all(&lab).unwrap();
        script(&lab.join("texlab"), "exit 0");
        let texlab = base.join("texlab.tar.gz");
        assert!(Command::new("/usr/bin/tar")
            .arg("-czf")
            .arg(&texlab)
            .arg("-C")
            .arg(&lab)
            .arg("texlab")
            .status()
            .unwrap()
            .success());
        let tinytex_sha = run::digest(&std::fs::read(&tinytex).unwrap());
        let texlab_sha = run::digest(&std::fs::read(&texlab).unwrap());
        let env = [
            ("CALLS", &calls),
            ("FAKES", &fakes),
            ("TINYTEX_TARBALL", &tinytex),
            ("TEXLAB_TARBALL", &texlab),
        ]
        .map(|(k, v)| (k.to_string(), v.display().to_string()))
        .to_vec();
        Bench {
            home: person.join(".quark/tex"),
            base,
            person,
            calls,
            tinytex_sha,
            texlab_sha,
            env,
        }
    }

    fn plan(home: &Path, tinytex: Option<&str>, texlab: Option<&str>) -> TexPlan {
        let download = |what: &str, bytes: u64| TexDownload {
            what: what.into(),
            host: "github.com".into(),
            bytes,
            exact: true,
            verified: true,
        };
        TexPlan {
            bundle: Bundle::Full,
            tinytex: "v2026.10".into(),
            texlab: "v5.26.0".into(),
            downloads: vec![download("TinyTeX", 1000), download("latexmk", 10), download("texlab", 100)],
            download: 1110,
            disk: 10_000,
            kept: 5_000,
            free: u64::MAX,
            fits: true,
            home: home.display().to_string(),
            tinytex_url: "https://github.com/rstudio/tinytex-releases/releases/download/v2026.10/TinyTeX-darwin-v2026.10.tar.xz".into(),
            tinytex_sha256: tinytex.map(str::to_string),
            texlab_url: "https://github.com/latex-lsp/texlab/releases/download/v5.26.0/texlab-aarch64-macos.tar.gz".into(),
            texlab_sha256: texlab.map(str::to_string),
        }
    }

    impl Bench {
        fn tex(&self, more: &[(&str, &str)]) -> TexEnv {
            let tools = Tools {
                curl: Some(self.base.join("fakes/curl")),
                tar: Some("/usr/bin/tar".into()),
                perl: Some("/usr/bin/perl".into()),
                xz: None,
            };
            let mut env: Vec<(&str, &str)> = self
                .env
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            env.extend(more);
            TexEnv::new(&self.home, Arc::new(Script::default()), Some(MAC), tools)
                .with_person(&self.person)
                .with_sandbox(Kind::None)
                .with_env(&env)
        }

        fn planned(&self, tinytex: Option<&str>, more: &[(&str, &str)]) -> TexEnv {
            let mut tex = self.tex(more);
            tex.state = TexState::Planned {
                plan: plan(&self.home, tinytex, Some(&self.texlab_sha)),
            };
            tex
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.calls)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    fn wait(tex: &mut TexEnv) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while tex.busy() && Instant::now() < deadline {
            tex.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!tex.busy(), "it never finished: {:?}", tex.state());
    }

    #[test]
    fn tex_is_made_as_planned_latexmk_added_and_a_first_page_typeset_before_it_is_ready() {
        let b = bench("made", false);
        let mut tex = b.planned(Some(&b.tinytex_sha), &[]);
        tex.create().unwrap();
        let order = ["tinytex", "latexmk", "texlab", "checking", "finishing"];
        let mut seen: Vec<String> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while tex.busy() && Instant::now() < deadline {
            tex.poll();
            if let TexState::Making { stage, .. } = tex.state() {
                if seen.last() != Some(stage) {
                    seen.push(stage.clone());
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let at: Vec<usize> = seen
            .iter()
            .map(|s| order.iter().position(|o| o == s).unwrap())
            .collect();
        assert!(
            at.windows(2).all(|w| w[0] < w[1]),
            "the stages went out of order: {seen:?}"
        );
        assert!(
            matches!(tex.state(), TexState::Ready { tinytex, texlab, .. } if tinytex == "v2026.10" && texlab == "v5.26.0"),
            "{:?}",
            tex.state()
        );

        let calls = b.calls();
        let asked: Vec<&str> = calls.iter().map(|c| c.split('|').next().unwrap()).collect();
        assert_eq!(asked[0].split(' ').next_back(), Some("https://github.com/rstudio/tinytex-releases/releases/download/v2026.10/TinyTeX-darwin-v2026.10.tar.xz"));
        assert_eq!(
            asked[1], "tlmgr install latexmk",
            "latexmk added, as the bundle had none"
        );
        assert!(asked[2].ends_with("texlab-aarch64-macos.tar.gz"));
        assert!(
            asked[3].starts_with("latexmk -norc -cd -pdf -synctex=1"),
            "a first page typeset: {asked:?}"
        );
        let own_home = b.home.join(".home").display().to_string();
        assert!(
            calls[..3]
                .iter()
                .all(|c| c.split('|').nth(1) == Some(own_home.as_str())),
            "{calls:?}"
        );

        for left in ["downloads", "smoke", MAKING] {
            assert!(!b.home.join(left).exists(), "{left} was left");
        }
        let marker = Marker::read(&b.home).unwrap();
        assert!(marker.holds(&b.home) && marker.size > 0, "{marker:?}");
        let in_home: Vec<_> = std::fs::read_dir(&b.person)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            in_home,
            [".quark"],
            "something was written in the person's home"
        );

        let mut again = b.tex(&[]);
        again.check();
        assert!(
            matches!(again.state(), TexState::Ready { .. }),
            "{:?}",
            again.state()
        );
    }

    #[test]
    fn a_bundle_with_latexmk_asks_nothing_more_of_ctan() {
        let b = bench("bundled", true);
        let mut tex = b.planned(Some(&b.tinytex_sha), &[]);
        tex.create().unwrap();
        wait(&mut tex);
        assert!(
            matches!(tex.state(), TexState::Ready { .. }),
            "{:?}",
            tex.state()
        );
        assert!(
            !b.calls().iter().any(|c| c.starts_with("tlmgr")),
            "{:?}",
            b.calls()
        );
    }

    #[test]
    fn tinytex_that_does_not_match_its_digest_is_not_unpacked_and_nothing_is_left() {
        let b = bench("digest", false);
        let mut tex = b.planned(Some(&"0".repeat(64)), &[]);
        tex.create().unwrap();
        wait(&mut tex);
        assert!(
            matches!(tex.state(), TexState::Failed { step, why, .. } if step == "tinytex" && why.contains("SHA-256")),
            "{:?}",
            tex.state()
        );
        assert!(!b.home.exists(), "a failed making left its folder");
        assert_eq!(b.calls().len(), 1, "something ran after the download");
        tex.cancel();
        assert!(matches!(tex.state(), TexState::Missing { .. }));
    }

    #[test]
    fn cancelled_while_downloading_it_stops_at_once_and_leaves_nothing() {
        let b = bench("cancel", false);
        let mut tex = b.planned(Some(&b.tinytex_sha), &[("SLOW", "1")]);
        tex.create().unwrap();
        let file = PathBuf::from(format!("{}.child", b.calls.display()));
        let deadline = Instant::now() + Duration::from_secs(20);
        let pid: i32 = loop {
            tex.poll();
            if let Some(pid) = std::fs::read_to_string(&file)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the download never started");
            std::thread::sleep(Duration::from_millis(5));
        };
        tex.cancel();
        assert_eq!(tex.state(), &TexState::Removing);
        let gone = Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                Instant::now() < gone,
                "what curl started outlived the cancel"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        wait(&mut tex);
        assert!(
            matches!(tex.state(), TexState::Missing { .. }) && !b.home.exists(),
            "{:?}",
            tex.state()
        );
    }

    #[test]
    fn removing_takes_the_marker_first_and_the_folder_after() {
        let b = bench("remove", false);
        made(&b.home);
        let mut tex = b.tex(&[]);
        tex.check();
        assert!(
            matches!(tex.state(), TexState::Ready { .. }),
            "{:?}",
            tex.state()
        );
        tex.remove().unwrap();
        assert_eq!(tex.state(), &TexState::Removing);
        assert!(
            Marker::read(&b.home).is_none(),
            "the marker was not taken first"
        );
        wait(&mut tex);
        assert!(
            matches!(tex.state(), TexState::Missing { .. })
                && !b.home.exists()
                && b.person.exists()
        );
    }

    #[test]
    fn without_room_for_it_nothing_is_begun() {
        let b = bench("room", false);
        let mut tex = b.tex(&[]);
        let mut big = plan(&b.home, Some(&b.tinytex_sha), None);
        big.disk = u64::MAX;
        tex.state = TexState::Planned { plan: big };
        assert!(tex.create().unwrap_err().contains("not room"));
        assert!(!tex.busy() && b.calls().is_empty() && !b.home.exists());
    }
}
