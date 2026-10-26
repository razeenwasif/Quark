//! A program run for the Lab, on threads of its own.
//!
//! What it prints arrives a line at a time, through a channel the core
//! drains on its heartbeat, so a conda that takes minutes to make an
//! environment never holds the window. A line ends at a newline or at a
//! NUL: conda's `--json` mode sends its progress as JSON objects with a NUL
//! between them. Both streams are read as they come, so neither fills its
//! pipe and stalls the program.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// One thing a job said.
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    Out(String),
    Err(String),
    /// It exited, with its code if it had one.
    Exited(Option<i32>),
}

/// What the reading threads send: a line, or that their stream closed.
enum Msg {
    Said(Said),
    Closed,
}

/// How a job is started, beyond its program and arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct How {
    /// Its own process group, stopped as one: what it starts — curl, an
    /// unpacker — goes when it is stopped, rather than downloading on.
    pub group: bool,
    /// None of the person's environment but what a download needs — home,
    /// user, language, temporary folder, proxies and certificates — so no
    /// variable of theirs changes what is made.
    pub clean: bool,
    /// The folder it runs in, if not Quark's own.
    pub dir: Option<PathBuf>,
}

/// What a clean job is still given from the environment.
const PASSED: [&str; 17] = [
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "http_proxy",
    "https_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "no_proxy",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
];

pub struct Job {
    child: Option<Child>,
    /// Its process group, when it has one of its own.
    group: Option<u32>,
    said: Receiver<Msg>,
    /// Streams still open, and whether the exit has been sent.
    open: u8,
    exited: bool,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("running", &self.child.is_some())
            .finish()
    }
}

/// Read a stream in lines that end at `\n` or `\0`, sending each.
fn pump(
    stream: impl Read + Send + 'static,
    send: impl Fn(String) -> bool + Send + 'static,
    done: impl FnOnce() + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line);
            match read {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for part in line.split(|b| *b == b'\0' || *b == b'\n') {
                        let text = String::from_utf8_lossy(part)
                            .trim_end_matches('\r')
                            .to_string();
                        if !text.trim().is_empty() && !send(text) {
                            return;
                        }
                    }
                }
            }
        }
        done();
    });
}

impl Job {
    /// Start `program` with `args` and these environment variables set.
    pub fn start(program: &Path, args: &[String], env: &[(String, String)]) -> Result<Job, String> {
        Job::start_with(program, args, env, How::default())
    }

    /// The same, started as `how` says.
    pub fn start_with(
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        how: How,
    ) -> Result<Job, String> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &how.dir {
            command.current_dir(dir);
        }
        if how.clean {
            command.env_clear();
            for name in PASSED {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
        }
        for (key, value) in env {
            command.env(key, value);
        }
        #[cfg(unix)]
        if how.group {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
        let group = (how.group && cfg!(unix)).then(|| child.id());
        let (tx, said) = channel();
        let (out, err) = (child.stdout.take(), child.stderr.take());
        let mut open = 0;
        if let Some(out) = out {
            open += 1;
            let (lines, closed) = (tx.clone(), tx.clone());
            pump(
                out,
                move |l| lines.send(Msg::Said(Said::Out(l))).is_ok(),
                move || drop(closed.send(Msg::Closed)),
            );
        }
        if let Some(err) = err {
            open += 1;
            let (lines, closed) = (tx.clone(), tx.clone());
            pump(
                err,
                move |l| lines.send(Msg::Said(Said::Err(l))).is_ok(),
                move || drop(closed.send(Msg::Closed)),
            );
        }
        Ok(Job {
            child: Some(child),
            group,
            said,
            open,
            exited: false,
        })
    }

    /// What it has said since last asked, ending with its exit once both
    /// its streams have closed and it has gone.
    pub fn poll(&mut self) -> Vec<Said> {
        let mut out = Vec::new();
        loop {
            match self.said.try_recv() {
                Ok(Msg::Closed) => self.open = self.open.saturating_sub(1),
                Ok(Msg::Said(said)) => out.push(said),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        if self.open == 0 && !self.exited {
            if let Some(child) = &mut self.child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        self.exited = true;
                        self.child = None;
                        out.push(Said::Exited(status.code()));
                    }
                    Ok(None) => {}
                    Err(_) => {
                        self.exited = true;
                        self.child = None;
                        out.push(Said::Exited(None));
                    }
                }
            }
        }
        out
    }

    pub fn running(&self) -> bool {
        !self.exited
    }

    /// Its process id while it runs — its group's too, when it has one.
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Stop it now — and, with a group of its own, everything it started.
    pub fn kill(&mut self) {
        if let (Some(group), false) = (self.group, self.exited) {
            crate::run::signal_group(group, true);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.exited = true;
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn drain(job: &mut Job) -> Vec<Said> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut all = Vec::new();
        while Instant::now() < deadline {
            let said = job.poll();
            let done = said.iter().any(|s| matches!(s, Said::Exited(_)));
            all.extend(said);
            if done {
                return all;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the job never ended: {all:?}");
    }

    #[test]
    fn lines_end_at_newlines_and_nuls_and_both_streams_are_read() {
        let mut job = Job::start(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "printf '{\"fetch\":\"a\"}\\0{\"fetch\":\"b\"}\\0\\n'; printf 'oops\\n' >&2; printf 'last'; exit 3".into()],
            &[],
        )
        .unwrap();
        let said = drain(&mut job);
        assert!(
            said.contains(&Said::Out("{\"fetch\":\"a\"}".into()))
                && said.contains(&Said::Out("{\"fetch\":\"b\"}".into())),
            "{said:?}"
        );
        assert!(
            said.contains(&Said::Err("oops".into())) && said.contains(&Said::Out("last".into())),
            "{said:?}"
        );
        assert_eq!(said.last(), Some(&Said::Exited(Some(3))));
    }

    #[test]
    fn a_job_is_killed_on_asking_and_on_being_let_go() {
        let mut job = Job::start(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "sleep 30".into()],
            &[],
        )
        .unwrap();
        let started = Instant::now();
        job.kill();
        assert!(!job.running() && started.elapsed() < Duration::from_secs(5));
        let job = Job::start(
            &PathBuf::from("/bin/sh"),
            &["-c".into(), "sleep 30".into()],
            &[],
        )
        .unwrap();
        drop(job);
    }

    #[test]
    fn a_job_with_a_group_of_its_own_takes_what_it_started_with_it_and_a_clean_one_none_of_the_environment(
    ) {
        let dir = std::env::temp_dir().join(format!("quark-job-group-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("child.pid");
        // Written where it runs: the folder it was given.
        let script = "sleep 300 & echo $! > child.pid; wait".to_string();
        let how = How {
            group: true,
            dir: Some(dir.clone()),
            ..How::default()
        };
        let mut job =
            Job::start_with(&PathBuf::from("/bin/sh"), &["-c".into(), script], &[], how).unwrap();
        assert!(job.id().is_some());
        // The shell makes the file before it writes to it: wait for the number.
        let deadline = Instant::now() + Duration::from_secs(5);
        let read = || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
        };
        while read().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = read().expect("the job never said what it started");
        job.kill();
        std::thread::sleep(Duration::from_millis(200));
        // SAFETY: signal 0 only asks whether the process exists.
        assert!(
            unsafe { libc::kill(pid, 0) } != 0,
            "what the job started outlived it"
        );

        std::env::set_var("QUARK_JOB_TEST_SECRET", "s3cret");
        let mut job = Job::start_with(
            &PathBuf::from("/bin/sh"),
            &[
                "-c".into(),
                "echo \"${QUARK_JOB_TEST_SECRET:-none}|${GIVEN:-none}|${HOME:+home}\"".into(),
            ],
            &[("GIVEN".into(), "yes".into())],
            How {
                clean: true,
                ..How::default()
            },
        )
        .unwrap();
        let said = drain(&mut job);
        assert!(
            said.contains(&Said::Out("none|yes|home".into())),
            "{said:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_lot_of_output_on_both_streams_does_not_stall_it() {
        let mut job = Job::start(
            &PathBuf::from("/bin/sh"),
            &[
                "-c".into(),
                "i=0; while [ $i -lt 20000 ]; do echo out$i; echo err$i >&2; i=$((i+1)); done"
                    .into(),
            ],
            &[],
        )
        .unwrap();
        let said = drain(&mut job);
        assert_eq!(
            said.iter().filter(|s| matches!(s, Said::Out(_))).count(),
            20000
        );
        assert_eq!(
            said.iter().filter(|s| matches!(s, Said::Err(_))).count(),
            20000
        );
    }
}
