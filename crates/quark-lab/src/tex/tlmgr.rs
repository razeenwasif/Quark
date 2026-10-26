//! TeX's package manager, asked for what a document lacks: which package
//! has `tikz-cd.sty`, and then — the person agreeing — to install it.
//!
//! Both ask CTAN, outside the sandbox: `tlmgr search --global --file` reads
//! the repository's package database, and `tlmgr install` downloads and
//! checks the package against it. Each runs on a thread of its own with
//! TeX's own programs and a home folder of TeX's, and what it came to is
//! the next state.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use serde::{Deserialize, Serialize};

use super::{TexEnv, TexState};

/// A package that has files a document lacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TexPackage {
    pub name: String,
    /// The files asked for that it has.
    pub files: Vec<String>,
}

/// Where finding and installing packages stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Packages {
    #[default]
    Idle,
    /// CTAN is being asked which packages have these files.
    Finding {
        files: Vec<String>,
    },
    /// What it said: the packages to install, and the files no package has.
    Found {
        packages: Vec<TexPackage>,
        unknown: Vec<String>,
    },
    Installing {
        packages: Vec<String>,
    },
    Installed {
        packages: Vec<String>,
    },
    Failed {
        why: String,
        log: Vec<String>,
    },
}

/// Read what `tlmgr search --global --file` said: package names on lines of
/// their own ending in `:`, each followed by its matching files, indented.
pub fn search_result(out: &str) -> Vec<(String, Vec<String>)> {
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    for line in out.lines() {
        if line.starts_with("tlmgr") || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) && line.ends_with(':') {
            found.push((line.trim_end_matches(':').trim().to_string(), Vec::new()));
        } else if let Some((_, files)) = found.last_mut() {
            files.push(line.trim().to_string());
        }
    }
    found
}

/// The package for a file, from what tlmgr found: one with the file under
/// that name, and, of several, the one whose folder is its own name.
pub fn package_for(file: &str, found: &[(String, Vec<String>)]) -> Option<String> {
    let has = |files: &[String]| files.iter().any(|f| f.rsplit('/').next() == Some(file));
    let candidates: Vec<&(String, Vec<String>)> =
        found.iter().filter(|(_, files)| has(files)).collect();
    candidates
        .iter()
        .find(|(name, files)| files.iter().any(|f| f.contains(&format!("/{name}/"))))
        .or_else(|| candidates.first())
        .map(|(name, _)| name.clone())
}

/// Run tlmgr, waiting for it: its output, or why it failed.
fn tlmgr(bin: &Path, home: &Path, args: &[String]) -> Result<String, (String, Vec<String>)> {
    let own_home = home.join(".home");
    let _ = std::fs::create_dir_all(&own_home);
    let output = Command::new(bin.join("tlmgr"))
        .args(args)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &own_home)
        .env("LANG", "en_US.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| (format!("tlmgr could not be run: {e}"), Vec::new()))?;
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(out);
    }
    let log: Vec<String> = out
        .lines()
        .chain(err.lines())
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let all = log.join("\n");
    // A new TeX Live year: this tlmgr may not read the repository's.
    let why = if all.contains("older than remote")
        || all.contains("Cross release updates are only supported")
    {
        "TeX Live has moved on to a new year, and this TinyTeX cannot install from it: remove TeX and install it again to update it.".to_string()
    } else {
        format!(
            "tlmgr stopped with exit status {}: {}",
            output.status.code().unwrap_or(-1),
            log.iter()
                .rev()
                .find(|l| !l.is_empty())
                .cloned()
                .unwrap_or_default()
        )
    };
    Err((why, log.into_iter().rev().take(40).rev().collect()))
}

/// What a worker came back with.
pub(super) type Asked = Receiver<Packages>;

impl TexEnv {
    /// What finding and installing packages stands at.
    pub fn packages(&self) -> &Packages {
        &self.packages
    }

    /// Goes up whenever packages' state does.
    pub fn packages_version(&self) -> u64 {
        self.packages_version
    }

    fn set_packages(&mut self, packages: Packages) {
        if self.packages != packages {
            self.packages = packages;
            self.packages_version += 1;
        }
    }

    /// The bin folder of a ready TeX, or why there is none.
    fn ready_bin(&self) -> Result<PathBuf, String> {
        if self.packages_asking.is_some() {
            return Err("TeX's package manager is busy: wait for it.".into());
        }
        match (&self.state, self.marker()) {
            (TexState::Ready { .. }, Some(marker)) => Ok(marker.bin),
            _ => Err("TeX is not installed.".into()),
        }
    }

    /// The first say-so: ask CTAN which packages have `files`.
    pub fn find_packages(&mut self, files: Vec<String>) -> Result<(), String> {
        let bin = self.ready_bin()?;
        let files: Vec<String> = files
            .into_iter()
            .filter(|f| !f.trim().is_empty() && !f.contains('/'))
            .collect();
        if files.is_empty() {
            return Err("There is no file to look for.".into());
        }
        let home = self.home.clone();
        let (tx, rx) = channel();
        let asked = files.clone();
        std::thread::spawn(move || {
            let mut packages: Vec<TexPackage> = Vec::new();
            let mut unknown = Vec::new();
            for file in &asked {
                match tlmgr(
                    &bin,
                    &home,
                    &[
                        "search".into(),
                        "--global".into(),
                        "--file".into(),
                        format!("/{file}"),
                    ],
                ) {
                    Ok(out) => match package_for(file, &search_result(&out)) {
                        Some(name) => match packages.iter_mut().find(|p| p.name == name) {
                            Some(package) => package.files.push(file.clone()),
                            None => packages.push(TexPackage {
                                name,
                                files: vec![file.clone()],
                            }),
                        },
                        None => unknown.push(file.clone()),
                    },
                    Err((why, log)) => {
                        let _ = tx.send(Packages::Failed { why, log });
                        return;
                    }
                }
            }
            let _ = tx.send(Packages::Found { packages, unknown });
        });
        self.packages_asking = Some(rx);
        self.set_packages(Packages::Finding { files });
        Ok(())
    }

    /// The second: install exactly these packages.
    pub fn install_packages(&mut self, names: Vec<String>) -> Result<(), String> {
        let bin = self.ready_bin()?;
        let names: Vec<String> = names
            .into_iter()
            .filter(|n| {
                !n.is_empty()
                    && n.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            })
            .collect();
        if names.is_empty() {
            return Err("There is no package to install.".into());
        }
        let home = self.home.clone();
        let (tx, rx) = channel();
        let asked = names.clone();
        std::thread::spawn(move || {
            let mut args = vec!["install".to_string()];
            args.extend(asked.iter().cloned());
            let _ = tx.send(match tlmgr(&bin, &home, &args) {
                Ok(_) => Packages::Installed { packages: asked },
                Err((why, log)) => Packages::Failed { why, log },
            });
        });
        self.packages_asking = Some(rx);
        self.set_packages(Packages::Installing { packages: names });
        Ok(())
    }

    /// Which of `files` — `amsmath.sty`, `beamer.cls` — this TeX lacks, as
    /// kpsewhich finds them: local and quick, nothing asked of CTAN. Before
    /// a new project's first build, so what it needs is offered first.
    pub fn lacking(&self, files: &[String]) -> Result<Vec<String>, String> {
        let bin = match (&self.state, self.marker()) {
            (TexState::Ready { .. }, Some(marker)) => marker.bin,
            _ => return Err("TeX is not installed.".into()),
        };
        let files: Vec<&String> = files
            .iter()
            .filter(|f| !f.is_empty() && !f.contains('/') && !f.starts_with('-'))
            .collect();
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let own_home = self.home.join(".home");
        let _ = std::fs::create_dir_all(&own_home);
        // kpsewhich says where each file it finds is, and nothing of the
        // rest — and exits 1 for them, which is not a failure here.
        let output = Command::new(bin.join("kpsewhich"))
            .args(&files)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", &own_home)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("kpsewhich could not be run: {e}"))?;
        let found = String::from_utf8_lossy(&output.stdout);
        let has = |file: &str| {
            found
                .lines()
                .any(|line| line.trim_end().rsplit('/').next() == Some(file))
        };
        Ok(files.into_iter().filter(|f| !has(f)).cloned().collect())
    }

    /// Put what was found, installed or failed away.
    pub fn forget_packages(&mut self) {
        if self.packages_asking.is_none() {
            self.set_packages(Packages::Idle);
        }
    }

    pub(super) fn poll_packages(&mut self) -> bool {
        let Some(rx) = &self.packages_asking else {
            return false;
        };
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Packages::Failed {
                why: "tlmgr stopped unexpectedly.".into(),
                log: Vec::new(),
            },
        };
        self.packages_asking = None;
        self.set_packages(done);
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::github::tests::Script;
    use super::super::tests::{made, MAC};
    use super::super::Tools;
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// What tlmgr says, as it said it for tikz-cd in TeX Live 2026.
    const SEARCH: &str = "tlmgr: package repository https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet (verified)\n\
        tikz-cd:\n\ttexmf-dist/tex/latex/tikz-cd/tikz-cd.sty\n\
        pgf-tikzcd-extras:\n\ttexmf-dist/tex/latex/pgf-tikzcd-extras/sub/tikz-cd.sty\n";

    #[test]
    fn what_tlmgr_found_is_read_and_the_package_chosen_is_the_files_own() {
        let found = search_result(SEARCH);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0],
            (
                "tikz-cd".to_string(),
                vec!["texmf-dist/tex/latex/tikz-cd/tikz-cd.sty".to_string()]
            )
        );
        assert_eq!(package_for("tikz-cd.sty", &found), Some("tikz-cd".into()));
        assert_eq!(package_for("nothing.sty", &found), None);
        assert!(search_result("tlmgr: package repository https://x (verified)\n").is_empty());
    }

    /// A ready TeX whose tlmgr is a script writing what it was asked.
    fn ready(tag: &str, tlmgr: &str) -> (PathBuf, TexEnv) {
        let dir = std::env::temp_dir().join(format!("quark-tlmgr-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let marker = made(&dir);
        std::fs::write(
            marker.bin.join("tlmgr"),
            format!(
                "#!/bin/sh\necho \"$*\" >> {}/asked.txt\n{tlmgr}\n",
                dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            marker.bin.join("tlmgr"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut tex = TexEnv::new(
            &dir,
            Arc::new(Script::default()),
            Some(MAC),
            Tools {
                curl: Some("curl".into()),
                tar: Some("tar".into()),
                perl: Some("perl".into()),
                xz: None,
            },
        );
        tex.check();
        (dir, tex)
    }

    fn settle(tex: &mut TexEnv) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while tex.packages_asking.is_some() && Instant::now() < deadline {
            tex.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_missing_file_is_found_in_its_package_and_installed_when_asked() {
        let (dir, mut tex) = ready(
            "found",
            &format!(
                "case \"$1\" in search) printf '{}' ;; install) exit 0 ;; esac",
                SEARCH.replace('\n', "\\n").replace('\t', "\\t")
            ),
        );
        tex.find_packages(vec!["tikz-cd.sty".into(), "nosuch.sty".into()])
            .unwrap();
        assert!(matches!(tex.packages(), Packages::Finding { .. }));
        settle(&mut tex);
        let Packages::Found { packages, unknown } = tex.packages().clone() else {
            panic!("{:?}", tex.packages())
        };
        assert_eq!(
            packages,
            [TexPackage {
                name: "tikz-cd".into(),
                files: vec!["tikz-cd.sty".into()]
            }]
        );
        assert_eq!(unknown, ["nosuch.sty"], "a file no package has is said");
        tex.install_packages(vec!["tikz-cd".into(), "bad; rm -rf".into()])
            .unwrap();
        settle(&mut tex);
        assert_eq!(
            tex.packages(),
            &Packages::Installed {
                packages: vec!["tikz-cd".into()]
            },
            "only package names are passed on"
        );
        let asked = std::fs::read_to_string(dir.join("asked.txt")).unwrap();
        assert_eq!(
            asked.lines().collect::<Vec<_>>(),
            [
                "search --global --file /tikz-cd.sty",
                "search --global --file /nosuch.sty",
                "install tikz-cd"
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_tlmgr_older_than_the_repository_says_to_update_tex() {
        let (dir, mut tex) = ready("old", "echo 'tlmgr: Local TeX Live (2026) is older than remote repository (2027).' >&2; exit 1");
        tex.find_packages(vec!["x.sty".into()]).unwrap();
        settle(&mut tex);
        assert!(
            matches!(tex.packages(), Packages::Failed { why, .. } if why.contains("new year")),
            "{:?}",
            tex.packages()
        );
        tex.forget_packages();
        assert_eq!(tex.packages(), &Packages::Idle);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_a_new_project_lacks_is_what_kpsewhich_does_not_find() {
        let (dir, tex) = ready("lacking", "exit 0");
        let bin = tex.marker().unwrap().bin;
        // As kpsewhich does: where each file it has is, nothing of the rest.
        std::fs::write(bin.join("kpsewhich"), "#!/bin/sh\nfor f in \"$@\"; do case $f in amsmath.sty|article.cls) echo /tex/dist/$f ;; esac; done\nexit 1\n").unwrap();
        std::fs::set_permissions(
            bin.join("kpsewhich"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let asked: Vec<String> = [
            "article.cls",
            "amsmath.sty",
            "natbib.sty",
            "-debug",
            "../x.sty",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            tex.lacking(&asked).unwrap(),
            ["natbib.sty"],
            "found ones and odd names left out"
        );
        assert_eq!(tex.lacking(&[]).unwrap(), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(dir);
        let missing = TexEnv::new(
            &std::env::temp_dir().join("quark-tlmgr-none"),
            Arc::new(Script::default()),
            Some(MAC),
            Tools::default(),
        );
        assert!(missing
            .lacking(&asked)
            .unwrap_err()
            .contains("not installed"));
    }
}
