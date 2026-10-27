//! Making a scanned PDF's pages searchable: a page, drawn as a picture by
//! the one who asks (Quark draws it with PDFium), read by `tesseract` into a
//! PDF of its own that holds only the words — invisible, where they are on
//! the page — for the document to lay over the page it came from.
//!
//! Each page is read by one tesseract, in its own process group and the
//! sandbox, which writes only in the folder the picture is in. It can be
//! stopped, and is stopped when it takes too long. Nothing is fetched.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::run;
use crate::sandbox::{self, Kind};

/// How long a page may take: a dense page at 300 dpi takes a few seconds.
const PER_PAGE: Duration = Duration::from_secs(90);

/// How long a stopped reading is given to go before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// Why a page was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// Stopped by the one who asked.
    Stopped,
    /// It took longer than a page may.
    TimedOut,
    /// Tesseract said why: its last line.
    Failed(String),
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unread::Stopped => f.write_str("stopped"),
            Unread::TimedOut => f.write_str("a page took too long to read"),
            Unread::Failed(why) => f.write_str(why),
        }
    }
}

/// The tesseract this machine has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tesseract {
    pub path: PathBuf,
}

/// A program, where Homebrew and the system keep them, or on PATH.
fn find(name: &str) -> Option<PathBuf> {
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        let path = Path::new(dir).join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// Language names as tesseract takes them: letters, digits and `_`, `+`
/// between; English when none are given.
pub fn languages(langs: &[String]) -> Result<String, String> {
    if langs.is_empty() {
        return Ok("eng".into());
    }
    if langs.iter().any(|l| l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        return Err("not a language tesseract knows".into());
    }
    Ok(langs.join("+"))
}

impl Tesseract {
    /// The machine's, if it has one.
    pub fn find() -> Option<Tesseract> {
        find("tesseract").map(|path| Tesseract { path })
    }

    /// The languages it can read, as `tesseract --list-langs` says; `osd`,
    /// which is not a language, left out.
    pub fn languages(&self) -> Vec<String> {
        let Ok(out) = Command::new(&self.path).arg("--list-langs").stdin(Stdio::null()).output() else {
            return Vec::new();
        };
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.contains(' ') && *l != "osd")
            .map(String::from)
            .collect()
    }

    /// Read the picture `picture` in `langs`: what it says, as a text-only
    /// PDF. The picture's folder is where tesseract works, and all it may
    /// write; `stop`, set, stops it.
    pub fn read(&self, picture: &Path, langs: &[String], kind: Kind, stop: &AtomicBool) -> Result<Vec<u8>, Unread> {
        let langs = languages(langs).map_err(Unread::Failed)?;
        let dir = picture.parent().ok_or_else(|| Unread::Failed("the picture is in no folder".into()))?;
        let name = picture.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = picture.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let out = format!("{stem}-words");
        let args: Vec<String> =
            [name.as_str(), out.as_str(), "-l", langs.as_str(), "-c", "textonly_pdf=1", "pdf"].map(String::from).to_vec();
        let reads: Vec<PathBuf> = self.path.parent().and_then(Path::parent).map(Path::to_path_buf).into_iter().collect();
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let (program, args) = sandbox::wrap_reading(kind, dir, &reads, &home, &self.path, &args);
        let log = std::fs::File::create(dir.join(format!("{stem}.log"))).map_err(|e| Unread::Failed(e.to_string()))?;
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(sandbox::real(dir))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", sandbox::real(dir))
            .env("TMPDIR", sandbox::real(dir))
            .env("OMP_THREAD_LIMIT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|e| Unread::Failed(format!("{}: {e}", program.display())))?;
        let started = Instant::now();
        let mut stopping: Option<(Instant, Unread)> = None;
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            match &stopping {
                Some((since, _)) if since.elapsed() > GRACE => run::signal_group(child.id(), true),
                Some(_) => {}
                None if stop.load(Ordering::Relaxed) => {
                    run::signal_group(child.id(), false);
                    stopping = Some((Instant::now(), Unread::Stopped));
                }
                None if started.elapsed() > PER_PAGE => {
                    run::signal_group(child.id(), false);
                    stopping = Some((Instant::now(), Unread::TimedOut));
                }
                None => {}
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if let Some((_, why)) = stopping {
            return Err(why);
        }
        if !status.success() {
            let log = std::fs::read_to_string(dir.join(format!("{stem}.log"))).unwrap_or_default();
            let said = log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("tesseract stopped").to_string();
            return Err(Unread::Failed(said));
        }
        std::fs::read(dir.join(format!("{out}.pdf"))).map_err(|e| Unread::Failed(format!("it wrote nothing: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// tesseract, standing in: `script` run with its arguments.
    fn stand_in(at: &Path, script: &str) -> Tesseract {
        std::fs::create_dir_all(at).unwrap();
        let path = at.join("tesseract");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Tesseract { path }
    }

    fn picture(base: &Path) -> PathBuf {
        let dir = base.join("work");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("p3.png");
        std::fs::write(&png, b"picture").unwrap();
        png
    }

    #[test]
    fn a_picture_is_read_into_a_pdf_of_its_words_in_the_languages_asked() {
        let base = std::env::temp_dir().join(format!("quark-ocr-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Writes `<out>.pdf` holding its arguments.
        let t = stand_in(&base.join("bin"), "echo \"$@\" > \"$2.pdf\"");
        let png = picture(&base);
        let read = t.read(&png, &["eng".into(), "deu".into()], Kind::None, &AtomicBool::new(false)).unwrap();
        assert_eq!(String::from_utf8_lossy(&read).trim(), "p3.png p3-words -l eng+deu -c textonly_pdf=1 pdf");
        let err = t.read(&png, &["eng; rm -rf /".into()], Kind::None, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(err, Unread::Failed("not a language tesseract knows".into()), "only language names");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_reading_that_fails_says_why_and_one_stopped_stops() {
        let base = std::env::temp_dir().join(format!("quark-ocr-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let png = picture(&base);
        let t = stand_in(&base.join("bin"), "echo 'Error opening data file eng.traineddata' >&2; exit 1");
        match t.read(&png, &[], Kind::None, &AtomicBool::new(false)) {
            Err(Unread::Failed(why)) => assert!(why.contains("traineddata"), "{why}"),
            other => panic!("{other:?}"),
        }
        let slow = stand_in(&base.join("slow"), "sleep 30");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopper = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                stop.store(true, Ordering::Relaxed);
            })
        };
        let started = Instant::now();
        assert_eq!(slow.read(&png, &[], Kind::None, &stop), Err(Unread::Stopped));
        assert!(started.elapsed() < Duration::from_secs(5), "stopped at once, not when it ended");
        stopper.join().unwrap();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn the_languages_asked_are_names_only() {
        assert_eq!(languages(&[]), Ok("eng".into()));
        assert_eq!(languages(&["chi_sim".into(), "eng".into()]), Ok("chi_sim+eng".into()));
        assert!(languages(&["".into()]).is_err());
        assert!(languages(&["../x".into()]).is_err());
    }
}
