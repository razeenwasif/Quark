//! Build driver for developing in WSL and building with the Windows toolchain.
//!
//! # The problem this exists to solve
//!
//! The source lives on the WSL filesystem, which Windows reaches only as the
//! UNC path `\\wsl.localhost\<distro>\...`. Much of the MSVC toolchain refuses
//! UNC working directories outright — `cmd.exe` announces "UNC paths are not
//! supported. Defaulting to Windows directory" and silently continues in the
//! wrong place, which turns into baffling build-script failures.
//!
//! So this maps the WSL share to a drive letter and runs the Windows
//! `cargo.exe` with an ordinary drive-letter working directory. No tool in the
//! chain ever sees a UNC path.
//!
//! It also forces `CARGO_TARGET_DIR` onto local NTFS. Leaving the target
//! directory on the WSL side routes every intermediate object file through the
//! 9p bridge, and that single change is worth more compile time than everything
//! else here combined.
//!
//! Usage: `cargo xtask build|run|test|check|clippy|dist|clean [-- extra args]`

use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Drive letters tried when mapping the WSL share, in order. Deliberately from
/// the back of the alphabet to avoid colliding with real volumes.
const CANDIDATE_DRIVES: &[char] = &['Q', 'X', 'Y', 'Z', 'W', 'V', 'U', 'T'];

/// The PDFium build the vendored binaries are pinned to.
///
/// This must match the `pdfium_*` feature selected for `pdfium-render` in the
/// workspace manifest: the FPDF ABI is not stable across Chromium rolls, and a
/// mismatched pair fails as memory corruption at runtime rather than as a link
/// error.
const PDFIUM_BUILD: &str = "7543";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let (cmd, rest) = args
        .split_first()
        .map(|(c, r)| (c.as_str(), r))
        .unwrap_or(("help", &[]));

    match cmd {
        "build" => cargo(&["build"], rest),
        "run" => cargo(&["run", "--bin", "quark"], rest),
        "test" => cargo(&["test"], rest),
        "check" => cargo(&["check"], rest),
        "clippy" => cargo(&["clippy"], rest),
        "clean" => cargo(&["clean"], rest),
        "dist" => dist(rest),
        "fetch-pdfium" => fetch_pdfium(),
        "where" => {
            let env = WinEnv::detect()?;
            println!("distro:     {}", env.distro);
            println!("unc:        {}", env.unc_root);
            println!("drive:      {}:", env.drive);
            println!("source:     {}", env.win_source_dir);
            println!("target dir: {}", env.win_target_dir);
            println!("cargo:      {}", env.cargo_exe.display());
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <build|run|test|check|clippy|dist|clean|fetch-pdfium|where> \
                 [-- cargo args]"
            );
            Ok(())
        }
    }
}

struct WinEnv {
    distro: String,
    /// `\\wsl.localhost\<distro>`
    unc_root: String,
    drive: char,
    /// Source dir as Windows sees it, e.g. `Q:\home\amaterasu\Quark`.
    win_source_dir: String,
    /// Target dir on local NTFS.
    win_target_dir: String,
    /// Linux-side path to the Windows cargo.
    cargo_exe: PathBuf,
}

impl WinEnv {
    fn detect() -> Result<Self> {
        let distro = env::var("WSL_DISTRO_NAME")
            .context("WSL_DISTRO_NAME is unset — xtask must run inside WSL")?;
        let unc_root = format!(r"\\wsl.localhost\{distro}");

        let repo = repo_root()?;
        let drive = ensure_mapped(&unc_root)?;

        let rel = repo
            .to_str()
            .context("repo path is not valid UTF-8")?
            .trim_start_matches('/')
            .replace('/', r"\");
        let win_source_dir = format!(r"{drive}:\{rel}");

        let win_target_dir = env::var("QUARK_WIN_TARGET_DIR").unwrap_or_else(|_| {
            let user = env::var("QUARK_WIN_USER").unwrap_or_else(|_| detect_win_user());
            format!(r"C:\Users\{user}\.quark-target")
        });

        let cargo_exe = find_windows_cargo()?;

        Ok(Self {
            distro,
            unc_root,
            drive,
            win_source_dir,
            win_target_dir,
            cargo_exe,
        })
    }
}

fn repo_root() -> Result<PathBuf> {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop(); // CARGO_MANIFEST_DIR is <repo>/xtask
    Ok(dir)
}

fn detect_win_user() -> String {
    // `cmd.exe /c echo %USERNAME%` would need a valid working directory, which
    // is the very thing we may not have yet. Reading the mounted Users
    // directory avoids the chicken-and-egg problem.
    if let Ok(entries) = std::fs::read_dir("/mnt/c/Users") {
        let skip = [
            "All Users",
            "Default",
            "Default User",
            "Public",
            "desktop.ini",
        ];
        let mut candidates: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !skip.contains(&n.as_str()))
            .collect();
        candidates.sort();
        if let Some(first) = candidates.into_iter().next() {
            return first;
        }
    }
    "Default".to_owned()
}

fn find_windows_cargo() -> Result<PathBuf> {
    if let Ok(p) = env::var("QUARK_WIN_CARGO") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        bail!("QUARK_WIN_CARGO points at {}, which does not exist", p.display());
    }

    let user = detect_win_user();
    let candidates = [
        PathBuf::from(format!("/mnt/c/Users/{user}/.cargo/bin/cargo.exe")),
        PathBuf::from("/mnt/c/Program Files/Rust/bin/cargo.exe"),
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    bail!(
        "no Windows cargo.exe found (looked in {}). \
         Install rustup on the Windows side, or set QUARK_WIN_CARGO.",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn ensure_mapped(unc_root: &str) -> Result<char> {
    if let Ok(d) = env::var("QUARK_DRIVE") {
        if let Some(c) = d.chars().next() {
            return Ok(c.to_ascii_uppercase());
        }
    }
    if let Some(existing) = find_existing_mapping(unc_root) {
        return Ok(existing);
    }

    for &drive in CANDIDATE_DRIVES {
        let out = Command::new("net.exe")
            .args(["use", &format!("{drive}:"), unc_root, "/persistent:yes"])
            .current_dir("/mnt/c")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("failed to run net.exe — is WSL interop enabled?")?;
        if out.status.success() {
            eprintln!("xtask: mapped {drive}: -> {unc_root}");
            return Ok(drive);
        }
    }

    bail!(
        "could not map any of {CANDIDATE_DRIVES:?} to {unc_root}. \
         Map one manually (`net use Q: {unc_root} /persistent:yes`) and set QUARK_DRIVE=Q."
    )
}

fn find_existing_mapping(unc_root: &str) -> Option<char> {
    let out = Command::new("net.exe")
        .args(["use"])
        .current_dir("/mnt/c")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&out.stdout);
    let target = unc_root.to_ascii_lowercase();
    for line in text.lines() {
        if !line.to_ascii_lowercase().contains(&target) {
            continue;
        }
        for token in line.split_whitespace() {
            let b = token.as_bytes();
            if b.len() == 2 && b[1] == b':' && (b[0] as char).is_ascii_alphabetic() {
                return Some((b[0] as char).to_ascii_uppercase());
            }
        }
    }
    None
}

/// Runs the Windows cargo against the mapped-drive copy of the workspace.
///
/// The working directory is set to `/mnt/c` rather than the repo. That looks
/// wrong but is deliberate: `Command::current_dir` is resolved by the Linux
/// side, and WSL interop translates any path under `/home` back into
/// `\\wsl.localhost\...` — reintroducing the exact UNC working directory this
/// wrapper exists to avoid. Pointing cargo at the workspace with
/// `--manifest-path` on the mapped drive sidesteps the translation entirely.
fn cargo(subcommand: &[&str], extra: &[String]) -> Result<()> {
    let env = WinEnv::detect()?;
    let root = repo_root()?;
    if !root.join("Cargo.toml").exists() {
        bail!("workspace root not found at {}", root.display());
    }

    let extra: Vec<&String> = extra.iter().skip_while(|a| a.as_str() == "--").collect();
    let manifest = format!(r"{}\Cargo.toml", env.win_source_dir);

    let mut cmd = Command::new(&env.cargo_exe);
    cmd.args(subcommand)
        .arg("--manifest-path")
        .arg(&manifest)
        // Passed as a flag, not as CARGO_TARGET_DIR. Environment variables set
        // here do not survive the WSL→Windows interop boundary, and when the
        // target directory silently falls back to the WSL share the build fails
        // deep in rustc with "could not create session directory lock file"
        // (9p does not implement LockFileEx). A CLI flag cannot be lost.
        .arg("--target-dir")
        .arg(&env.win_target_dir)
        .args(&extra)
        // The Windows cargo must not inherit the Linux toolchain's environment
        // or it will try to invoke a Linux rustc and fail confusingly.
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTUP_HOME")
        .env_remove("RUSTC")
        .env_remove("CARGO")
        .env_remove("CARGO_HOME")
        .env_remove("LD_LIBRARY_PATH")
        .current_dir("/mnt/c");

    eprintln!(
        "xtask: cargo {} --manifest-path {manifest}  (target {})",
        subcommand.join(" "),
        env.win_target_dir
    );

    let status = cmd.status().context("failed to launch Windows cargo.exe")?;
    if !status.success() {
        bail!("cargo {} failed", subcommand.join(" "));
    }
    Ok(())
}

/// Builds a release binary and stages a distributable folder beside it.
///
/// PDFium is copied in alongside the executable, because that is the first
/// place `quark_pdf::engine` looks; a Quark without it starts and then fails to
/// open anything, which is a much worse failure than not starting at all.
fn dist(extra: &[String]) -> Result<()> {
    let mut args: Vec<String> = vec!["--release".into()];
    args.extend(extra.iter().skip_while(|a| a.as_str() == "--").cloned());
    cargo(&["build"], &args)?;

    let env = WinEnv::detect()?;
    let target = win_path_to_wsl(&env.win_target_dir)?;
    let exe = target.join("release").join("quark.exe");
    if !exe.exists() {
        bail!(
            "expected {} to exist after a release build; did the build target change?",
            exe.display()
        );
    }

    let root = repo_root()?;
    let out = root.join("dist");
    std::fs::create_dir_all(&out)?;

    std::fs::copy(&exe, out.join("quark.exe"))
        .with_context(|| format!("copying {}", exe.display()))?;

    let dll = root.join("vendor/pdfium/pdfium.dll");
    if !dll.exists() {
        bail!(
            "{} is missing — run `cargo xtask fetch-pdfium`",
            dll.display()
        );
    }
    std::fs::copy(&dll, out.join("pdfium.dll"))?;
    let license = root.join("vendor/pdfium/LICENSE-pdfium");
    if license.exists() {
        std::fs::copy(&license, out.join("LICENSE-pdfium"))?;
    }

    std::fs::write(out.join("install.cmd"), INSTALL_CMD)?;

    println!("Staged into {}", out.display());
    println!("  quark.exe");
    println!("  pdfium.dll");
    println!("  install.cmd    (registers Quark as a PDF handler)");
    Ok(())
}

/// A one-click registration script for the staged folder.
const INSTALL_CMD: &str = r#"@echo off
rem Installs Quark for the current user and registers it as a PDF handler.
rem
rem Quark is copied into %LOCALAPPDATA%\Programs\Quark first, and it is that
rem copy that gets registered. Registering the executable where it was built
rem would point Windows at a path inside build output, so cleaning the build
rem directory would silently break the file associations and drop the taskbar
rem pin. Installing first means this folder can be deleted afterwards.
rem
rem This does not silently take over the .pdf association: Windows protects
rem that choice and only the user can make it. After this runs, Quark appears
rem in the "Open with" menu and in Settings > Default apps, and Windows will
rem offer it the next time a PDF is opened.
setlocal
set HERE=%~dp0
"%HERE%quark.exe" --install
if errorlevel 1 (
  echo Installation failed.
  exit /b 1
)
echo.
echo Quark is installed and registered.
echo Pin it from %LOCALAPPDATA%\Programs\Quark, not from this folder.
echo.
echo To make it the default PDF reader, either:
echo   * right-click any PDF, choose "Open with" then "Choose another app",
echo     pick Quark and tick "Always use this app"; or
echo   * open Settings ^> Apps ^> Default apps, find Quark, and set it for .pdf
echo.
pause
"#;

/// Translates `C:\Users\x\.quark-target` into `/mnt/c/Users/x/.quark-target`.
fn win_path_to_wsl(p: &str) -> Result<PathBuf> {
    let bytes = p.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' {
        bail!("{p} does not look like a Windows path");
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let rest = p[2..].replace('\\', "/");
    Ok(PathBuf::from(format!("/mnt/{drive}{rest}")))
}

/// Downloads the pinned PDFium build for both platforms.
fn fetch_pdfium() -> Result<()> {
    let root = repo_root()?;
    let targets = [
        ("win-x64", "vendor/pdfium", "bin/pdfium.dll", "pdfium.dll"),
        (
            "linux-x64",
            "vendor/pdfium-linux",
            "lib/libpdfium.so",
            "libpdfium.so",
        ),
    ];

    for (platform, dest_dir, inner, name) in targets {
        let dest = root.join(dest_dir);
        std::fs::create_dir_all(&dest)?;
        let url = format!(
            "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F{PDFIUM_BUILD}/pdfium-{platform}.tgz"
        );
        eprintln!("xtask: fetching {url}");

        let tmp = std::env::temp_dir().join(format!("pdfium-{platform}.tgz"));
        let status = Command::new("curl")
            .args(["-sSL", "-o"])
            .arg(&tmp)
            .arg(&url)
            .status()
            .context("curl is required to fetch PDFium")?;
        if !status.success() {
            bail!("could not download {url}");
        }

        let extract = std::env::temp_dir().join(format!("pdfium-{platform}"));
        let _ = std::fs::remove_dir_all(&extract);
        std::fs::create_dir_all(&extract)?;
        let status = Command::new("tar")
            .arg("xzf")
            .arg(&tmp)
            .arg("-C")
            .arg(&extract)
            .status()
            .context("tar is required to unpack PDFium")?;
        if !status.success() {
            bail!("could not unpack {}", tmp.display());
        }

        std::fs::copy(extract.join(inner), dest.join(name))
            .with_context(|| format!("copying {inner}"))?;
        let lic = extract.join("LICENSE");
        if lic.exists() {
            let _ = std::fs::copy(lic, dest.join("LICENSE-pdfium"));
        }
        let ver = extract.join("VERSION");
        if ver.exists() {
            let _ = std::fs::copy(ver, dest.join("VERSION"));
        }
        println!("  {} -> {}", platform, dest.join(name).display());
    }

    println!("PDFium build {PDFIUM_BUILD} vendored.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_map_onto_the_wsl_mount() {
        assert_eq!(
            win_path_to_wsl(r"C:\Users\me\.quark-target").unwrap(),
            PathBuf::from("/mnt/c/Users/me/.quark-target")
        );
        assert_eq!(
            win_path_to_wsl(r"D:\build").unwrap(),
            PathBuf::from("/mnt/d/build")
        );
    }

    #[test]
    fn a_path_without_a_drive_letter_is_rejected() {
        assert!(win_path_to_wsl("/home/me").is_err());
        assert!(win_path_to_wsl("C").is_err());
    }

    #[test]
    fn the_pinned_pdfium_build_matches_the_manifest() {
        // The vendored binary and the bindings the crate was compiled against
        // must be the same build, or FPDF calls corrupt memory at runtime.
        let manifest = include_str!("../../Cargo.toml");
        assert!(
            manifest.contains(&format!("pdfium_{PDFIUM_BUILD}")),
            "xtask pins PDFium {PDFIUM_BUILD} but the workspace selects a different feature"
        );
    }

    #[test]
    fn the_install_script_does_not_claim_to_set_the_default() {
        // Windows does not permit it, and saying otherwise would be a lie the
        // user discovers only when it silently does not happen.
        assert!(INSTALL_CMD.contains("--install"));
        assert!(INSTALL_CMD.to_lowercase().contains("default apps"));
    }
}
