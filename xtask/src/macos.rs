//! Building `Quark.app` on macOS.
//!
//! The rest of xtask drives a Windows toolchain from WSL, and none of that
//! applies on a Mac: the host cargo builds a native binary directly. What this
//! adds is the bundle Finder and Launchpad expect — the executable and PDFium
//! side by side in `Contents/MacOS`, which is the first place
//! `quark_pdf::engine` looks, an `Info.plist`, and an icon generated from the
//! same `icon.rs` the window and the Windows resource are drawn from.
//!
//! Usage: `cargo xtask bundle-mac [--install] [-- extra cargo args]`

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::repo_root;

// The icon module has no dependencies precisely so build scripts and tools
// like this one can include it; see its own header.
#[allow(dead_code)]
mod icon {
    include!("../../crates/quark-app/src/icon.rs");
}

/// Reverse-DNS identifier, derived from the repository the workspace names.
const BUNDLE_ID: &str = "io.github.razeenwasif.quark";

/// Where an installed copy goes.
const INSTALL_DIR: &str = "/Applications";

pub fn bundle(args: &[String]) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("bundle-mac builds a macOS application and has to run on a Mac");
    }
    let install = args.iter().any(|a| a == "--install");
    let extra: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != "--install")
        .skip_while(|a| a.as_str() == "--")
        .cloned()
        .collect();

    let root = repo_root()?;
    let dylib = root.join("vendor/pdfium-mac/libpdfium.dylib");
    if !dylib.exists() {
        bail!(
            "{} is missing — run `cargo xtask fetch-pdfium` first",
            dylib.display()
        );
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "-p", "quark-app", "--bin", "quark"])
        .args(&extra)
        .current_dir(&root)
        .status()
        .context("running cargo")?;
    if !status.success() {
        bail!("the release build failed");
    }

    let exe = target_dir(&root).join("release/quark");
    if !exe.exists() {
        bail!("expected {} after a release build", exe.display());
    }

    let app = root.join("dist/Quark.app");
    if app.exists() {
        std::fs::remove_dir_all(&app).with_context(|| format!("clearing {}", app.display()))?;
    }
    let macos = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;

    std::fs::copy(&exe, macos.join("quark")).context("copying the executable")?;
    std::fs::copy(&dylib, macos.join("libpdfium.dylib")).context("copying PDFium")?;
    let license = root.join("vendor/pdfium-mac/LICENSE-pdfium");
    if license.exists() {
        std::fs::copy(&license, resources.join("LICENSE-pdfium"))?;
    }
    std::fs::write(app.join("Contents/Info.plist"), info_plist(env!("CARGO_PKG_VERSION")))?;
    write_icns(&resources.join("Quark.icns"))?;

    // Apple Silicon refuses to run unsigned code, and copying the binaries
    // into the bundle invalidates the linker's ad-hoc signature on the bundle
    // as a whole. An ad-hoc signature is enough to run locally.
    run(Command::new("codesign").args(["--force", "--deep", "--sign", "-"]).arg(&app))
        .context("ad-hoc signing the bundle")?;

    println!("Built {}", app.display());

    if install {
        let dest = Path::new(INSTALL_DIR).join("Quark.app");
        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .with_context(|| format!("removing the old {}", dest.display()))?;
        }
        // `ditto` rather than a recursive copy: it preserves the signature's
        // extended attributes and the bundle's symlinks exactly.
        run(Command::new("ditto").arg(&app).arg(&dest)).context("copying into /Applications")?;
        println!("Installed {}", dest.display());
    }
    Ok(())
}

/// The target directory the build above wrote into.
///
/// Read off this executable's own location rather than assumed to be
/// `<repo>/target`: a `build.target-dir` in a user's cargo config moves it,
/// and `cargo run` always places xtask at `<target>/debug/xtask`.
fn target_dir(root: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| root.join("target"))
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("starting {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed");
    }
    Ok(())
}

/// The bundle's `Info.plist`.
///
/// It claims PDFs as a viewer, so Quark is in Open With and macOS sends it
/// the files it is asked to open — to the running Quark, not a new one each
/// time. winit does not deliver those Apple Events, so
/// `quark_shell::open_requests` hears them itself. The rank is `Alternate`:
/// Quark offers to open PDFs without taking over as the default; the user
/// chooses that in Finder's Get Info.
pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Quark</string>
    <key>CFBundleDisplayName</key>
    <string>Quark</string>
    <key>CFBundleIdentifier</key>
    <string>{BUNDLE_ID}</string>
    <key>CFBundleExecutable</key>
    <string>quark</string>
    <key>CFBundleIconFile</key>
    <string>Quark</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>PDF document</string>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>com.adobe.pdf</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
"#
    )
}

/// Renders the icon into an `.iconset` and lets `iconutil` pack it.
fn write_icns(out: &Path) -> Result<()> {
    let set = std::env::temp_dir().join("Quark.iconset");
    let _ = std::fs::remove_dir_all(&set);
    std::fs::create_dir_all(&set)?;
    for base in [16usize, 32, 128, 256, 512] {
        for scale in [1usize, 2] {
            let px = base * scale;
            let name = if scale == 1 {
                format!("icon_{base}x{base}.png")
            } else {
                format!("icon_{base}x{base}@2x.png")
            };
            std::fs::write(set.join(name), png(px, px, &padded_icon(px)))?;
        }
    }
    run(Command::new("iconutil").args(["-c", "icns", "-o"]).arg(out).arg(&set))
        .context("packing the icon")?;
    let _ = std::fs::remove_dir_all(&set);
    Ok(())
}

/// The mark inset to macOS's icon grid.
///
/// `icon::render` fills its square edge to edge, which is right for a Windows
/// taskbar but reads as oversized in the Dock: Apple's grid puts the tile at
/// roughly 80% of the canvas.
fn padded_icon(size: usize) -> Vec<u8> {
    let inner = ((size as f32) * 0.8).round() as usize;
    let off = (size - inner) / 2;
    let tile = icon::render(inner);
    let mut out = vec![0u8; size * size * 4];
    for y in 0..inner {
        let src = &tile[y * inner * 4..(y + 1) * inner * 4];
        let start = ((y + off) * size + off) * 4;
        out[start..start + inner * 4].copy_from_slice(src);
    }
    out
}

/// Encodes straight RGBA as a PNG.
///
/// Uncompressed deflate blocks: the icons are a few hundred kilobytes at most,
/// `iconutil` recompresses them anyway, and it keeps xtask free of an image
/// dependency.
pub fn png(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(height * (width * 4 + 1));
    for row in rgba.chunks(width * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }

    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        let len = block.len() as u16;
        z.push(last as u8);
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_output_has_a_valid_signature_and_ihdr() {
        let img = png(2, 2, &[255; 16]);
        assert_eq!(&img[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&img[12..16], b"IHDR");
        assert!(img.ends_with(&[0xae, 0x42, 0x60, 0x82]), "IEND crc");
    }

    #[test]
    fn crc_and_adler_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn the_padded_icon_leaves_a_transparent_margin() {
        let px = padded_icon(64);
        assert_eq!(px.len(), 64 * 64 * 4);
        assert_eq!(px[3], 0, "the corner is outside the tile");
    }

    #[test]
    fn the_plist_names_the_executable_and_icon() {
        let plist = info_plist("1.2.3");
        assert!(plist.contains("<string>quark</string>"));
        assert!(plist.contains("<string>Quark</string>"));
        assert!(plist.contains("1.2.3"));
        assert!(plist.contains("<string>com.adobe.pdf</string>"), "claims PDFs");
        assert!(plist.contains("<string>Viewer</string>"));
    }
}
