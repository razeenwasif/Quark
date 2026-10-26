//! What the TeX builds, the TeX install and OCR share of running a
//! program: how it ended, its group stopped as one, its output's tail, and
//! a file's digest.

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(unix)]
use crate::sandbox;

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Running,
    /// Exited 0.
    Done,
    /// Exited otherwise, or was killed by something else.
    Failed,
    TimedOut,
    Stopped,
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A file's SHA-256, as hex.
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Send a process group made with `process_group(0)` a signal: SIGKILL if
/// `kill`, else SIGTERM.
pub(crate) fn signal_group(group: u32, kill: bool) {
    #[cfg(unix)]
    {
        let signal = if kill { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: killpg only sends a signal, to a group this crate made for
        // a process it started.
        unsafe {
            libc::killpg(group as libc::pid_t, signal);
        }
    }
    #[cfg(not(unix))]
    let _ = (group, kill);
}

/// The folder a process is working in, if it can be told.
#[cfg(unix)]
fn cwd_of(pid: u32) -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        return std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
    }
    let out = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n').map(PathBuf::from))
}

/// Stop the process group `pid` leads if it is still there and working in
/// `dir` — or, if `inside`, anywhere in it: the one started there, not a pid
/// handed to something else since. Whether it was.
#[cfg(unix)]
pub(crate) fn stop_group_in(pid: u32, dir: &Path, inside: bool) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    let dir = sandbox::real(dir);
    let ours = alive
        && cwd_of(pid).is_some_and(|cwd| {
            let cwd = sandbox::real(&cwd);
            if inside {
                cwd.starts_with(&dir)
            } else {
                cwd == dir
            }
        });
    if ours {
        // SAFETY: the group is the one this crate made for the process, and
        // its leader is working where it was started.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    ours
}

/// The last `lines` lines of a file a run wrote, and whether there were more.
pub fn tail(path: &Path, lines: usize) -> (String, bool) {
    let text = std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    let cut = all.len() > lines;
    (all[all.len().saturating_sub(lines)..].join("\n"), cut)
}
