//! Making a scanned PDF's pages searchable: each page drawn with Poppler's
//! `pdftoppm` at 300 dpi, then read by `tesseract` into a PDF of its own
//! that holds only the words — invisible, where they are on the page — for
//! the PDF mode to lay over the page it came from.
//!
//! One shell, in its own process group and the sandbox, reads the pages one
//! at a time into its own folder, which is all it writes; it can be stopped,
//! and says how far it has got by the files it leaves. Nothing is fetched.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::run::{self, State};
use crate::sandbox::{self, Kind};

/// How long a page may take, at most, in all: a scan of a dense page at
/// 300 dpi takes a few seconds.
const PER_PAGE: Duration = Duration::from_secs(90);

/// How long a stopped job is given to go before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// The programs OCR needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrTools {
    pub pdftoppm: PathBuf,
    pub tesseract: PathBuf,
}

/// A program, where Homebrew and the system keep them, or on PATH.
fn find(name: &str) -> Option<PathBuf> {
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        let path = Path::new(dir).join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

impl OcrTools {
    /// The machine's, if it has both.
    pub fn find() -> Option<OcrTools> {
        Some(OcrTools {
            pdftoppm: find("pdftoppm")?,
            tesseract: find("tesseract")?,
        })
    }

    /// The languages tesseract can read, and the folder its data is in — as
    /// `tesseract --list-langs` says; `osd`, which is not a language, left out.
    pub fn languages(&self) -> (Vec<String>, Option<PathBuf>) {
        let Ok(out) = Command::new(&self.tesseract)
            .arg("--list-langs")
            .stdin(Stdio::null())
            .output()
        else {
            return (Vec::new(), None);
        };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let folder = text
            .lines()
            .find_map(|l| l.split('"').nth(1))
            .map(PathBuf::from);
        let langs = text
            .lines()
            .filter(|l| !l.contains(' ') && !l.trim().is_empty() && *l != "osd")
            .map(|l| l.trim().to_string())
            .collect();
        (langs, folder)
    }
}

/// A job reading some pages of a PDF.
#[derive(Debug)]
pub struct OcrJob {
    dir: PathBuf,
    child: Option<Child>,
    pages: Vec<u32>,
    started: Instant,
    deadline: Instant,
    stopping: Option<Instant>,
    state: State,
    /// What its log said, when it failed.
    said: String,
}

/// Language names as tesseract takes them: letters, digits and `_`, `+`
/// between.
fn languages(langs: &[String]) -> Result<String, String> {
    if langs.is_empty() {
        return Ok("eng".into());
    }
    if langs
        .iter()
        .any(|l| l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err("not a language tesseract knows".into());
    }
    Ok(langs.join("+"))
}

impl OcrJob {
    /// Start reading `pages` (from one) of the PDF `pdf`, in `langs`, in a
    /// folder of its own under `parent`.
    pub fn start(
        tools: &OcrTools,
        pdf: &[u8],
        pages: &[u32],
        langs: &[String],
        parent: &Path,
        kind: Kind,
        reads: &[PathBuf],
    ) -> Result<OcrJob, String> {
        if pages.is_empty() {
            return Err("no pages to read".into());
        }
        let langs = languages(langs)?;
        let dir = parent.join(format!("ocr-{}-{}", std::process::id(), run::now()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // Its own, and no one else's: a protected PDF's pages are in it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::write(dir.join("in.pdf"), pdf).map_err(|e| format!("{}: {e}", dir.display()))?;
        let list = pages
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            "for n in {list}; do \
               \"$0\" -r 300 -f $n -l $n -png -singlefile in.pdf p$n 2>>log.txt || exit 3; \
               \"$1\" p$n.png t$n -l {langs} -c textonly_pdf=1 pdf 2>>log.txt || exit 4; \
               rm -f p$n.png; echo $n >> done.txt; \
             done"
        );
        let args = vec![
            "-c".to_string(),
            script,
            tools.pdftoppm.display().to_string(),
            tools.tesseract.display().to_string(),
        ];
        let mut allowed: Vec<PathBuf> = reads.to_vec();
        for tool in [&tools.pdftoppm, &tools.tesseract] {
            if let Some(dir) = tool.parent().and_then(Path::parent) {
                allowed.push(dir.to_path_buf());
            }
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let (program, args) =
            sandbox::wrap_reading(kind, &dir, &allowed, &home, Path::new("/bin/sh"), &args);
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(sandbox::real(&dir))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", sandbox::real(&dir))
            .env("TMPDIR", sandbox::real(&dir))
            .env("OMP_THREAD_LIMIT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
        let started = Instant::now();
        Ok(OcrJob {
            dir,
            child: Some(child),
            pages: pages.to_vec(),
            started,
            deadline: started + PER_PAGE * pages.len() as u32,
            stopping: None,
            state: State::Running,
            said: String::new(),
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// How many pages are read, of how many.
    pub fn progress(&self) -> (usize, usize) {
        let done = std::fs::read_to_string(self.dir.join("done.txt"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        (done, self.pages.len())
    }

    /// What its log said, when it failed.
    pub fn said(&self) -> &str {
        &self.said
    }

    pub fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn signal(&self, kill: bool) {
        if let Some(child) = &self.child {
            run::signal_group(child.id(), kill);
        }
    }

    /// Stop it: politely, then not.
    pub fn stop(&mut self) {
        if self.state == State::Running && self.stopping.is_none() {
            self.stopping = Some(Instant::now());
            self.signal(false);
        }
    }

    /// Whether it has ended, and settle it if it has.
    pub fn poll(&mut self) -> bool {
        if self.state != State::Running {
            return true;
        }
        if let Some(since) = self.stopping {
            if since.elapsed() > GRACE {
                self.signal(true);
            }
        } else if Instant::now() > self.deadline {
            self.stopping = Some(Instant::now());
            self.signal(false);
        }
        let Some(child) = &mut self.child else {
            return true;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.child = None;
                self.state = match (self.stopping, status.success()) {
                    (Some(_), _) if Instant::now() > self.deadline => State::TimedOut,
                    (Some(_), _) => State::Stopped,
                    (None, true) => State::Done,
                    (None, false) => State::Failed,
                };
                if self.state == State::Failed {
                    let log = std::fs::read_to_string(self.dir.join("log.txt")).unwrap_or_default();
                    self.said = log
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("it stopped")
                        .to_string();
                }
                true
            }
            Ok(None) => false,
            Err(_) => true,
        }
    }

    /// What it read: each page's text-only PDF, by page.
    pub fn read(&self) -> Vec<(u32, Vec<u8>)> {
        self.pages
            .iter()
            .filter_map(|n| {
                std::fs::read(self.dir.join(format!("t{n}.pdf")))
                    .ok()
                    .map(|b| (*n, b))
            })
            .collect()
    }
}

impl Drop for OcrJob {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.signal(true);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// pdftoppm and tesseract, standing in: a picture is a file, and the
    /// text a page's number.
    fn stand_ins(at: &Path, tesseract: &str) -> OcrTools {
        std::fs::create_dir_all(at).unwrap();
        let pdftoppm = at.join("pdftoppm");
        std::fs::write(
            &pdftoppm,
            "#!/bin/sh\nfor last; do :; done\necho picture > \"$last.png\"\n",
        )
        .unwrap();
        let reader = at.join("tesseract");
        std::fs::write(&reader, format!("#!/bin/sh\n{tesseract}\n")).unwrap();
        for tool in [&pdftoppm, &reader] {
            std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        OcrTools {
            pdftoppm,
            tesseract: reader,
        }
    }

    fn settle(job: &mut OcrJob) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.poll() {
            assert!(Instant::now() < deadline, "it never ended");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn each_page_is_drawn_and_read_into_a_pdf_of_its_words() {
        let base = std::env::temp_dir().join(format!("quark-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // The stand-in writes `t<n>.pdf` holding its arguments.
        let tools = stand_ins(&base.join("bin"), "echo \"$@\" > \"$2.pdf\"");
        let mut job = OcrJob::start(
            &tools,
            b"%PDF-1.7",
            &[2, 5],
            &["eng".into(), "deu".into()],
            &base,
            Kind::None,
            &[],
        )
        .unwrap();
        settle(&mut job);
        assert_eq!(job.state(), State::Done, "{}", job.said());
        assert_eq!(job.progress(), (2, 2));
        let read = job.read();
        assert_eq!(read.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [2, 5]);
        assert_eq!(
            String::from_utf8_lossy(&read[0].1).trim(),
            "p2.png t2 -l eng+deu -c textonly_pdf=1 pdf"
        );
        let dir = job.dir.clone();
        drop(job);
        assert!(!dir.exists(), "its folder goes with it");
        assert!(
            OcrJob::start(
                &tools,
                b"%PDF",
                &[1],
                &["eng; rm -rf /".into()],
                &base,
                Kind::None,
                &[]
            )
            .is_err(),
            "only language names"
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_job_that_fails_says_why_and_one_stopped_stops() {
        let base = std::env::temp_dir().join(format!("quark-ocr-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tools = stand_ins(
            &base.join("bin"),
            "echo 'Error opening data file eng.traineddata' >&2; exit 1",
        );
        let mut job = OcrJob::start(&tools, b"%PDF", &[1], &[], &base, Kind::None, &[]).unwrap();
        settle(&mut job);
        assert_eq!(job.state(), State::Failed);
        assert!(job.said().contains("traineddata"), "{}", job.said());
        let slow = stand_ins(&base.join("slow"), "sleep 30");
        let mut job =
            OcrJob::start(&slow, b"%PDF", &[1, 2, 3], &[], &base, Kind::None, &[]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        job.stop();
        settle(&mut job);
        assert_eq!(job.state(), State::Stopped);
        let _ = std::fs::remove_dir_all(base);
    }
}
