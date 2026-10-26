//! TeX for Quark's LaTeX mode: TinyTeX — a TeX Live made small, with
//! pdfLaTeX, XeLaTeX, LuaLaTeX, BibTeX and biber — and latexmk to drive it,
//! with texlab for the editor's completion, all in `~/.quark/tex`: Quark's
//! own, apart from any TeX of the person's, made with their say-so twice as
//! Lean is.
//!
//! First to plan: GitHub is asked what TinyTeX's and texlab's newest
//! releases are and what downloading them costs — exactly, with the
//! digests it publishes. Then to make exactly that (`make`). A TeX is ready
//! only when the marker its making writes last says so and everything it
//! names is still there; nothing is started to find out. Documents are
//! built with it in the Lab's sandbox (`build`).

pub mod build;
pub mod github;
mod make;
pub mod tlmgr;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::disk::free_bytes;
use crate::github::Fetch;
use crate::sandbox::{self, Kind};

/// Where it is kept, under the home folder.
pub const FOLDER: &str = ".quark/tex";

/// The file its making writes last: what a ready TeX is.
pub const MARKER: &str = "ready.json";

/// Which TinyTeX: the full one, with what most documents use, or the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Bundle {
    /// `TinyTeX`: articles, theses and Beamer decks compile without asking
    /// for more.
    #[default]
    Full,
    /// `TinyTeX-1`: the common core; more documents ask for a package.
    Small,
}

impl Bundle {
    /// What its release's files are called before the platform.
    pub fn prefix(self) -> &'static str {
        match self {
            Bundle::Full => "TinyTeX",
            Bundle::Small => "TinyTeX-1",
        }
    }
}

/// One thing a plan downloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TexDownload {
    pub what: String,
    pub host: String,
    pub bytes: u64,
    /// Its size as its source states it, rather than an estimate.
    pub exact: bool,
    /// Checked against a SHA-256 its source publishes.
    pub verified: bool,
}

/// What making TeX would be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TexPlan {
    pub bundle: Bundle,
    /// TinyTeX's release, `v2026.10`, and texlab's, `v5.26.0`.
    pub tinytex: String,
    pub texlab: String,
    pub downloads: Vec<TexDownload>,
    pub download: u64,
    /// The most disk it takes while it is made, and what it keeps once made.
    pub disk: u64,
    pub kept: u64,
    pub free: u64,
    /// Whether there is room for it.
    pub fits: bool,
    pub home: String,
    pub tinytex_url: String,
    pub tinytex_sha256: Option<String>,
    pub texlab_url: String,
    pub texlab_sha256: Option<String>,
}

impl TexPlan {
    /// The bundle, as TinyTeX's releases call it.
    pub fn bundle_name(&self) -> &'static str {
        self.bundle.prefix()
    }
}

/// Where TeX for the LaTeX mode stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum TexState {
    /// Not looked at yet.
    Unknown,
    /// Not made: what it needs that this machine lacks, if anything; the
    /// free disk; where it would go.
    Missing {
        needs: Vec<String>,
        free: u64,
        home: String,
    },
    /// GitHub is being asked what there is.
    Planning,
    Planned {
        plan: TexPlan,
    },
    /// Being made: which step, how far through it, and what it said last.
    Making {
        stage: String,
        done: u64,
        total: u64,
        log: Vec<String>,
    },
    Ready {
        tinytex: String,
        texlab: String,
        bundle: Bundle,
        size: u64,
        home: String,
    },
    Removing,
    Failed {
        step: String,
        why: String,
        log: Vec<String>,
    },
}

/// What this machine's builds are called, for TinyTeX's and texlab's releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub tinytex: &'static str,
    pub texlab: &'static str,
}

/// This machine's, if both are built for it: macOS and Linux, as the Lab runs.
pub fn platform() -> Option<Platform> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(Platform {
            tinytex: "darwin",
            texlab: "aarch64-macos",
        }),
        ("macos", "x86_64") => Some(Platform {
            tinytex: "darwin",
            texlab: "x86_64-macos",
        }),
        ("linux", "x86_64") => Some(Platform {
            tinytex: "linux-x86_64",
            texlab: "x86_64-linux",
        }),
        ("linux", "aarch64") => Some(Platform {
            tinytex: "linux-arm64",
            texlab: "aarch64-linux",
        }),
        _ => None,
    }
}

/// The programs making TeX needs from the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tools {
    /// To download TinyTeX and texlab.
    pub curl: Option<PathBuf>,
    /// To unpack them: macOS's reads `.tar.xz` itself.
    pub tar: Option<PathBuf>,
    /// What tlmgr and latexmk are written in.
    pub perl: Option<PathBuf>,
    /// What GNU tar unpacks `.tar.xz` with.
    pub xz: Option<PathBuf>,
}

impl Tools {
    /// This machine's.
    pub fn find() -> Tools {
        let first = |paths: &[&str]| paths.iter().map(PathBuf::from).find(|p| p.is_file());
        Tools {
            curl: first(&[
                "/usr/bin/curl",
                "/opt/homebrew/bin/curl",
                "/usr/local/bin/curl",
                "/bin/curl",
            ]),
            tar: first(&["/usr/bin/tar", "/bin/tar"]),
            perl: first(&[
                "/usr/bin/perl",
                "/opt/homebrew/bin/perl",
                "/usr/local/bin/perl",
                "/bin/perl",
            ]),
            xz: first(&[
                "/usr/bin/xz",
                "/opt/homebrew/bin/xz",
                "/usr/local/bin/xz",
                "/bin/xz",
            ]),
        }
    }
}

/// What a made TeX is, as its making wrote it last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub tinytex: String,
    pub texlab: String,
    pub bundle: Bundle,
    /// TinyTeX's programs: latexmk, pdflatex, tlmgr.
    pub bin: PathBuf,
    pub texlab_bin: PathBuf,
    pub size: u64,
    pub made: u64,
}

impl Marker {
    pub fn read(home: &Path) -> Option<Marker> {
        serde_json::from_str(&std::fs::read_to_string(home.join(MARKER)).ok()?).ok()
    }

    /// Whether everything it names is there, and inside `home` — what the
    /// sandbox lets a build read.
    pub fn holds(&self, home: &Path) -> bool {
        let home = sandbox::real(home);
        let inside = |p: &Path| sandbox::real(p).starts_with(&home);
        inside(&self.bin)
            && ["latexmk", "pdflatex", "tlmgr"]
                .iter()
                .all(|p| self.bin.join(p).exists())
            && inside(&self.texlab_bin)
            && self.texlab_bin.is_file()
    }

    pub fn latexmk(&self) -> PathBuf {
        self.bin.join("latexmk")
    }

    pub fn tlmgr(&self) -> PathBuf {
        self.bin.join("tlmgr")
    }
}

/// The folder TinyTeX's programs are in, once unpacked: `TinyTeX/bin/<arch>`,
/// whichever arch it is — `universal-darwin` on a Mac.
pub fn bin_of(home: &Path) -> Option<PathBuf> {
    let bins = home.join("TinyTeX").join("bin");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&bins)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("tlmgr").exists())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// The machine's TeX for the LaTeX mode.
pub struct TexEnv {
    home: PathBuf,
    /// The person's home folder, which builds are kept out of.
    person: PathBuf,
    fetch: Arc<dyn Fetch>,
    platform: Option<Platform>,
    tools: Tools,
    /// The sandbox builds run in: the machine's, tried when first needed.
    sandbox: Option<Kind>,
    /// More for the making's environment: what a test tells its stand-ins.
    env: Vec<(String, String)>,
    state: TexState,
    version: u64,
    planning: Option<Receiver<Result<TexPlan, String>>>,
    making: Option<make::Making>,
    /// Its folder being taken away: after a cancel, a failure or a removal.
    clearing: Option<Receiver<()>>,
    /// Whether what a Quark gone mid-making left has been looked for.
    reaped: bool,
    /// Packages a document lacks, being found or installed.
    packages: tlmgr::Packages,
    packages_version: u64,
    packages_asking: Option<tlmgr::Asked>,
}

impl std::fmt::Debug for TexEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TexEnv")
            .field("home", &self.home)
            .field("state", &self.state)
            .finish()
    }
}

const BUSY: &str = "TeX is busy: wait for what it is doing, or cancel it.";

impl TexEnv {
    /// In `home`, asking `fetch`, on `platform`, made with `tools`.
    pub fn new(
        home: &Path,
        fetch: Arc<dyn Fetch>,
        platform: Option<Platform>,
        tools: Tools,
    ) -> TexEnv {
        TexEnv {
            home: home.to_path_buf(),
            person: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            fetch,
            platform,
            tools,
            sandbox: None,
            env: Vec::new(),
            state: TexState::Unknown,
            version: 0,
            planning: None,
            making: None,
            clearing: None,
            reaped: false,
            packages: tlmgr::Packages::Idle,
            packages_version: 0,
            packages_asking: None,
        }
    }

    /// The machine's: `~/.quark/tex`, asking GitHub.
    pub fn at_home() -> TexEnv {
        let person = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        TexEnv::new(
            &person.join(FOLDER),
            Arc::new(crate::github::Live::default()),
            platform(),
            Tools::find(),
        )
        .with_person(&person)
    }

    /// For someone whose home folder is `person`.
    pub fn with_person(mut self, person: &Path) -> TexEnv {
        self.person = person.to_path_buf();
        self
    }

    /// Building in `kind`, rather than the machine's sandbox.
    pub fn with_sandbox(mut self, kind: Kind) -> TexEnv {
        self.sandbox = Some(kind);
        self
    }

    /// With more set in the environment it is made in.
    pub fn with_env(mut self, env: &[(&str, &str)]) -> TexEnv {
        self.env = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The person's home folder, which builds are kept out of.
    pub fn person(&self) -> &Path {
        &self.person
    }

    pub fn state(&self) -> &TexState {
        &self.state
    }

    /// Goes up whenever the state does.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The sandbox builds run in, tried on this machine the first time.
    pub fn sandbox(&mut self) -> Kind {
        *self.sandbox.get_or_insert_with(sandbox::available)
    }

    pub fn busy(&self) -> bool {
        self.planning.is_some() || self.making.is_some() || self.clearing.is_some()
    }

    /// Whether anything at all is under way: busy, or packages being found
    /// or installed.
    pub fn working(&self) -> bool {
        self.busy() || self.packages_asking.is_some()
    }

    fn set(&mut self, state: TexState) {
        if self.state != state {
            self.state = state;
            self.version += 1;
        }
    }

    /// What a build needs, when TeX is ready.
    pub fn marker(&self) -> Option<Marker> {
        matches!(self.state, TexState::Ready { .. })
            .then(|| Marker::read(&self.home))
            .flatten()
    }

    /// What it lacks to be made here.
    fn needs(&self) -> Vec<String> {
        let mut needs = Vec::new();
        if self.platform.is_none() {
            needs.push("TeX for the LaTeX mode is made on macOS and Linux".to_string());
        }
        if self.tools.curl.is_none() {
            needs.push("curl, to download TinyTeX".to_string());
        }
        if self.tools.tar.is_none() {
            needs.push("tar, to unpack TinyTeX".to_string());
        }
        if self.tools.perl.is_none() {
            needs.push("perl, which TeX's package manager and latexmk are written in".to_string());
        }
        if cfg!(target_os = "linux") && self.tools.xz.is_none() {
            needs.push("xz, to unpack TinyTeX".to_string());
        }
        needs
    }

    fn missing(&mut self) {
        let missing = TexState::Missing {
            needs: self.needs(),
            free: free_bytes(&self.home),
            home: self.home.display().to_string(),
        };
        self.set(missing);
    }

    /// Look at what is there — never starting a process, never asking the
    /// network: ready if its marker holds, else missing. The first time,
    /// what a Quark gone before its making was done left is taken away.
    pub fn check(&mut self) {
        if self.busy() {
            return;
        }
        if !self.reaped {
            self.reaped = true;
            if make::reap(&self.home) {
                self.clear_after(None);
                return self.set(TexState::Removing);
            }
        }
        if let Some(marker) = Marker::read(&self.home).filter(|m| m.holds(&self.home)) {
            let ready = TexState::Ready {
                tinytex: marker.tinytex,
                texlab: marker.texlab,
                bundle: marker.bundle,
                size: marker.size,
                home: self.home.display().to_string(),
            };
            self.set(ready);
            return;
        }
        if matches!(
            self.state,
            TexState::Planned { .. } | TexState::Failed { .. }
        ) {
            return;
        }
        self.missing();
    }

    /// The first say-so: ask GitHub what there is, on a worker.
    pub fn plan(&mut self, bundle: Bundle) -> Result<(), String> {
        if self.busy() {
            return Err(BUSY.into());
        }
        if matches!(self.state, TexState::Ready { .. }) {
            return Err("TeX is installed already.".into());
        }
        let needs = self.needs();
        if !needs.is_empty() {
            return Err(needs.join("; "));
        }
        let (fetch, platform) = (self.fetch.clone(), self.platform.expect("checked by needs"));
        let (home, free) = (self.home.display().to_string(), free_bytes(&self.home));
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("quark-tex-plan".into())
            .spawn(move || {
                let _ = tx.send(github::plan(&*fetch, &platform, bundle, &home, free));
            })
            .map_err(|e| e.to_string())?;
        self.planning = Some(rx);
        self.set(TexState::Planning);
        Ok(())
    }

    /// Stop what is being done, or put a plan or a failure away. A making
    /// stopped leaves nothing behind: its folder is taken away.
    pub fn cancel(&mut self) {
        if self.planning.take().is_some() {
            return self.missing();
        }
        if self.making.is_some() {
            return self.stop_making();
        }
        if self.clearing.is_some() {
            if matches!(self.state, TexState::Failed { .. }) {
                self.set(TexState::Removing);
            }
            return;
        }
        if matches!(
            self.state,
            TexState::Planned { .. } | TexState::Failed { .. }
        ) {
            self.missing();
        }
    }

    /// Take what has happened since last asked. `true` if anything did.
    pub fn poll(&mut self) -> bool {
        let planned = self.poll_planning();
        let made = self.poll_making();
        let cleared = self.poll_clearing();
        let packages = self.poll_packages();
        planned || made || cleared || packages
    }

    fn poll_planning(&mut self) -> bool {
        let Some(rx) = &self.planning else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.planning = None;
                match result {
                    Ok(plan) => self.set(TexState::Planned { plan }),
                    Err(why) => self.set(TexState::Failed {
                        step: "plan".into(),
                        why,
                        log: Vec::new(),
                    }),
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.planning = None;
                self.set(TexState::Failed {
                    step: "plan".into(),
                    why: "the plan stopped unexpectedly".into(),
                    log: Vec::new(),
                });
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::github::tests::Script;
    use super::*;
    use std::time::{Duration, Instant};

    pub(super) const MAC: Platform = Platform {
        tinytex: "darwin",
        texlab: "aarch64-macos",
    };

    pub(super) fn all() -> Tools {
        Tools {
            curl: Some("curl".into()),
            tar: Some("tar".into()),
            perl: Some("perl".into()),
            xz: Some("xz".into()),
        }
    }

    fn home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quark-tex-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A made TeX, as far as its marker is concerned.
    pub(super) fn made(home: &Path) -> Marker {
        let bin = home.join("TinyTeX/bin/universal-darwin");
        std::fs::create_dir_all(&bin).unwrap();
        for program in ["latexmk", "pdflatex", "tlmgr"] {
            std::fs::write(bin.join(program), b"#!/bin/sh\n").unwrap();
        }
        let texlab = home.join("texlab/texlab");
        std::fs::create_dir_all(texlab.parent().unwrap()).unwrap();
        std::fs::write(&texlab, b"#!/bin/sh\n").unwrap();
        let marker = Marker {
            tinytex: "v2026.10".into(),
            texlab: "v5.26.0".into(),
            bundle: Bundle::Full,
            bin,
            texlab_bin: texlab,
            size: 800_000_000,
            made: 1,
        };
        std::fs::write(home.join(MARKER), serde_json::to_string(&marker).unwrap()).unwrap();
        marker
    }

    fn wait(tex: &mut TexEnv) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while tex.busy() && Instant::now() < deadline {
            tex.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nothing_is_asked_until_a_plan_is_and_a_plan_is_what_github_said() {
        let dir = home("plan");
        let script = Arc::new(Script::recorded());
        let mut tex = TexEnv::new(&dir, script.clone(), Some(MAC), all());
        tex.check();
        assert!(
            matches!(tex.state(), TexState::Missing { needs, free, .. } if needs.is_empty() && *free > 0),
            "{:?}",
            tex.state()
        );
        assert!(
            script.asked.lock().unwrap().is_empty(),
            "GitHub was asked before a plan was"
        );
        tex.plan(Bundle::Full).unwrap();
        assert_eq!(tex.state(), &TexState::Planning);
        wait(&mut tex);
        let TexState::Planned { plan } = tex.state() else {
            panic!("{:?}", tex.state())
        };
        assert_eq!(
            (plan.tinytex.as_str(), plan.texlab.as_str(), plan.bundle),
            ("v2026.10", "v5.26.0", Bundle::Full)
        );
        tex.check();
        assert!(
            matches!(tex.state(), TexState::Planned { .. }),
            "a plan stays until it is made or put away"
        );
        tex.cancel();
        assert!(matches!(tex.state(), TexState::Missing { .. }));
    }

    #[test]
    fn what_this_machine_lacks_is_said_and_no_plan_is_made_without_it() {
        let dir = home("needs");
        let mut tex = TexEnv::new(
            &dir,
            Arc::new(Script::recorded()),
            Some(MAC),
            Tools {
                perl: None,
                ..all()
            },
        );
        tex.check();
        assert!(
            matches!(tex.state(), TexState::Missing { needs, .. } if needs.len() == 1 && needs[0].contains("perl")),
            "{:?}",
            tex.state()
        );
        assert!(tex.plan(Bundle::Full).unwrap_err().contains("perl"));
        let mut nowhere = TexEnv::new(&dir, Arc::new(Script::default()), None, all());
        nowhere.check();
        assert!(
            matches!(nowhere.state(), TexState::Missing { needs, .. } if needs[0].contains("macOS and Linux"))
        );
    }

    #[test]
    fn a_tex_is_ready_only_when_its_marker_holds_and_names_nothing_outside_it() {
        let dir = home("ready");
        let marker = made(&dir);
        let mut tex = TexEnv::new(&dir, Arc::new(Script::default()), Some(MAC), all());
        tex.check();
        assert!(
            matches!(tex.state(), TexState::Ready { tinytex, bundle, .. } if tinytex == "v2026.10" && *bundle == Bundle::Full),
            "{:?}",
            tex.state()
        );
        assert_eq!(tex.marker(), Some(marker.clone()));
        assert!(
            tex.plan(Bundle::Full).is_err(),
            "a plan for a TeX installed already"
        );
        assert_eq!(bin_of(&dir), Some(marker.bin.clone()));

        // latexmk gone: not ready.
        std::fs::remove_file(marker.bin.join("latexmk")).unwrap();
        let mut gone = TexEnv::new(&dir, Arc::new(Script::default()), Some(MAC), all());
        gone.check();
        assert!(matches!(gone.state(), TexState::Missing { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn this_machine_is_one_tex_is_built_for() {
        if cfg!(any(target_os = "macos", target_os = "linux"))
            && cfg!(any(target_arch = "aarch64", target_arch = "x86_64"))
        {
            assert!(platform().is_some());
        }
    }
}
