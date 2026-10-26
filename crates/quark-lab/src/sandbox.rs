//! Where a run may reach.
//!
//! Nothing on the network; nothing written outside its own folder;
//! nothing of the person's home folder read except that folder and the
//! Lab's Python, or Quark's Lean — though the folders on the way to them
//! may be looked at, not into: macOS's realpath(3) asks each for its
//! attributes, and Lean finds itself with it. On macOS that is a Seatbelt
//! profile through
//! `sandbox-exec`; on Linux, bubblewrap. Each is tried on this machine
//! before it is trusted — a probe that must be refused — and a machine
//! with neither runs code only when the scientist says so knowing that.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Which sandbox a run is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// macOS's Seatbelt, through `sandbox-exec`.
    Seatbelt,
    /// Linux's bubblewrap.
    Bubblewrap,
    /// None: the code can do what the person can.
    None,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Seatbelt => "Seatbelt",
            Kind::Bubblewrap => "bubblewrap",
            Kind::None => "no sandbox",
        }
    }
}

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// A path as the kernel will see it: Seatbelt matches resolved paths, and
/// on macOS `/var` and `/tmp` are links into `/private`.
pub fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A path inside a Seatbelt string.
fn quoted(path: &Path) -> String {
    let text = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{text}\"")
}

/// The folders strictly between `home` and `path`, when `path` is in it.
fn between(home: &Path, path: &Path) -> Vec<PathBuf> {
    let Ok(rest) = path.strip_prefix(home) else {
        return Vec::new();
    };
    let parts: Vec<_> = rest.components().collect();
    let mut at = home.to_path_buf();
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .map(|part| {
            at.push(part);
            at.clone()
        })
        .collect()
}

/// The Seatbelt profile for a run in `run`, with the Lab's Python (or
/// Quark's Lean) in `env`, for a person whose home is `home`.
pub fn seatbelt(run: &Path, env: &Path, home: &Path) -> String {
    seatbelt_reading(run, &[env.to_path_buf()], home)
}

/// The same, reading each of `reads` besides the run's own folder: a
/// compiler or an interpreter kept in the home folder, as conda's Python,
/// nvm's Node and rustup's Rust are.
pub fn seatbelt_reading(run: &Path, reads: &[PathBuf], home: &Path) -> String {
    let (run, home) = (real(run), real(home));
    let reads: Vec<PathBuf> = reads.iter().map(|p| real(p)).collect();
    let mut ways: Vec<PathBuf> = between(&home, &run)
        .into_iter()
        .chain(reads.iter().flat_map(|r| between(&home, r)))
        .collect();
    ways.sort();
    ways.dedup();
    let look = if ways.is_empty() {
        String::new()
    } else {
        format!(
            "(allow file-read-metadata {})\n",
            ways.iter()
                .map(|p| format!("(literal {})", quoted(p)))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let readable: String = reads
        .iter()
        .map(|r| format!(" (subpath {})", quoted(r)))
        .collect();
    let (run, home) = (quoted(&run), quoted(&home));
    format!(
        "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n\
         (allow file-write* (subpath {run}) (literal \"/dev/null\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/tty\"))\n\
         (deny file-read* (subpath {home}))\n\
         (allow file-read* (subpath {run}){readable} (literal {home}))\n{look}"
    )
}

/// bubblewrap's arguments for a run, up to the program: the root read only,
/// the home folder hidden, the Lab's Python read only and the run's folder
/// writable over it, and no network.
pub fn bubblewrap(run: &Path, env: &Path, home: &Path) -> Vec<String> {
    bubblewrap_reading(run, &[env.to_path_buf()], home)
}

/// The same, each of `reads` bound read only over the hidden home folder.
pub fn bubblewrap_reading(run: &Path, reads: &[PathBuf], home: &Path) -> Vec<String> {
    let (run, home) = (
        real(run).display().to_string(),
        real(home).display().to_string(),
    );
    let mut out: Vec<String> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-net",
        "--unshare-pid",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        &home,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for read in reads {
        let read = real(read).display().to_string();
        out.extend(["--ro-bind".to_string(), read.clone(), read]);
    }
    out.extend([
        "--bind".to_string(),
        run.clone(),
        run.clone(),
        "--chdir".to_string(),
        run,
    ]);
    out
}

fn bwrap() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("bwrap"))
            .find(|p| p.is_file())
    })
}

/// `program args` as started inside the sandbox.
pub fn wrap(
    kind: Kind,
    run: &Path,
    env: &Path,
    home: &Path,
    program: &Path,
    args: &[String],
) -> (PathBuf, Vec<String>) {
    wrap_reading(kind, run, &[env.to_path_buf()], home, program, args)
}

/// The same, reading each of `reads` besides the run's folder.
pub fn wrap_reading(
    kind: Kind,
    run: &Path,
    reads: &[PathBuf],
    home: &Path,
    program: &Path,
    args: &[String],
) -> (PathBuf, Vec<String>) {
    let tail = || std::iter::once(program.display().to_string()).chain(args.iter().cloned());
    match kind {
        Kind::Seatbelt => {
            let mut out = vec!["-p".to_string(), seatbelt_reading(run, reads, home)];
            out.extend(tail());
            (PathBuf::from(SANDBOX_EXEC), out)
        }
        Kind::Bubblewrap => {
            let mut out = bubblewrap_reading(run, reads, home);
            out.push("--".into());
            out.extend(tail());
            (bwrap().unwrap_or_else(|| PathBuf::from("bwrap")), out)
        }
        Kind::None => (program.to_path_buf(), args.to_vec()),
    }
}

/// Whether `kind` keeps a probe in. The probe runs as a run would, with a
/// stand-in home folder of its own — the person's is not touched — and
/// must write in its folder, and be refused writing in the home folder
/// and reading a file there.
pub fn holds(kind: Kind) -> bool {
    if kind == Kind::None {
        return false;
    }
    // A folder for each probe: two at once — tests do — would share one, and
    // the first to finish would take the other's away.
    static PROBE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = PROBE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base =
        std::env::temp_dir().join(format!("quark-fence-{}-{:?}-{n}", std::process::id(), kind));
    let _ = std::fs::remove_dir_all(&base);
    let (inside, home) = (base.join("run"), base.join("home"));
    if std::fs::create_dir_all(&inside).is_err()
        || std::fs::create_dir_all(&home).is_err()
        || std::fs::write(home.join("secret"), "secret").is_err()
    {
        return false;
    }
    let script = "echo in > \"$1/in\"; echo out > \"$2/out\" 2>/dev/null; cat \"$2/secret\" > \"$1/read\" 2>/dev/null; exit 0";
    let args: Vec<String> = vec![
        "-c".into(),
        script.into(),
        "probe".into(),
        real(&inside).display().to_string(),
        real(&home).display().to_string(),
    ];
    let (program, args) = wrap(
        kind,
        &inside,
        Path::new("/nonexistent"),
        &home,
        Path::new("/bin/sh"),
        &args,
    );
    let ran = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let read = std::fs::read_to_string(inside.join("read")).unwrap_or_default();
    let held = ran.is_ok()
        && inside.join("in").exists()
        && !home.join("out").exists()
        && !read.contains("secret");
    let _ = std::fs::remove_dir_all(&base);
    held
}

/// The sandbox this machine has, tried once.
pub fn available() -> Kind {
    static KIND: OnceLock<Kind> = OnceLock::new();
    *KIND.get_or_init(|| {
        if cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).is_file() && holds(Kind::Seatbelt) {
            Kind::Seatbelt
        } else if cfg!(target_os = "linux") && bwrap().is_some() && holds(Kind::Bubblewrap) {
            Kind::Bubblewrap
        } else {
            Kind::None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_denies_the_network_and_every_write_but_the_run_and_reads_of_home_but_its_own() {
        let p = seatbelt(
            Path::new("/w/proj/research/x/runs/R001"),
            Path::new("/h/.quark/lab/envs/lab"),
            Path::new("/h"),
        );
        assert!(p.contains("(deny network*)") && p.contains("(deny file-write*)"));
        assert!(p.contains("(allow file-write* (subpath \"/w/proj/research/x/runs/R001\")"));
        assert!(p.contains("(deny file-read* (subpath \"/h\"))"));
        assert!(p.contains("(allow file-read* (subpath \"/w/proj/research/x/runs/R001\") (subpath \"/h/.quark/lab/envs/lab\") (literal \"/h\"))"));
        assert!(
            seatbelt(Path::new("/a\"b"), Path::new("/e"), Path::new("/h")).contains("\"/a\\\"b\""),
            "a quote in a path ends the string"
        );
        // The folders on the way to what it reads, each looked at only.
        assert!(p.contains("(allow file-read-metadata (literal \"/h/.quark\") (literal \"/h/.quark/lab\") (literal \"/h/.quark/lab/envs\"))\n"), "{p}");
        let lean = seatbelt(
            Path::new("/h/Research/sky/proofs/H004/attempt-1"),
            Path::new("/h/.quark/lean"),
            Path::new("/h"),
        );
        assert!(lean.contains("(literal \"/h/.quark\") (literal \"/h/Research\") (literal \"/h/Research/sky\") (literal \"/h/Research/sky/proofs\") (literal \"/h/Research/sky/proofs/H004\"))"), "{lean}");
        assert!(
            !seatbelt(Path::new("/tmp/r"), Path::new("/opt/env"), Path::new("/h"))
                .contains("file-read-metadata"),
            "nothing in the home folder, nothing to look at"
        );
        let b = bubblewrap(
            Path::new("/h/p/run"),
            Path::new("/h/.quark/env"),
            Path::new("/h"),
        );
        let at = |flag: &str, value: &str| b.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(
            b.contains(&"--unshare-net".to_string()) && at("--tmpfs", "/h"),
            "{b:?}"
        );
        let hidden = b.iter().position(|a| a == "/h").unwrap();
        let bound = b.iter().position(|a| a == "/h/p/run").unwrap();
        assert!(
            hidden < bound,
            "the run must be bound over the hidden home, not under it"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_on_this_mac_keeps_a_probe_in() {
        if !Path::new(SANDBOX_EXEC).is_file() {
            return;
        }
        assert!(holds(Kind::Seatbelt), "Seatbelt let the probe out");
        assert!(!holds(Kind::None));
        assert_eq!(available(), Kind::Seatbelt);
    }
}
