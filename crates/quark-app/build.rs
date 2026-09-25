//! Compiles Quark's icon into the executable as a Windows resource.
//!
//! # Why this exists
//!
//! `ViewportBuilder::with_icon` sets the icon of a *running* window. Explorer
//! never runs the program to draw a pinned taskbar button, a shortcut, or the
//! file listing — it reads an `RT_GROUP_ICON` resource out of the executable.
//! Without one those all render blank, which is exactly what pinning Quark to
//! the taskbar used to look like.
//!
//! The `.ico` is generated here from `src/icon.rs` rather than checked in, so
//! there is one definition of the mark and the pinned icon cannot drift away
//! from the window icon.

use std::path::PathBuf;

// `src/icon.rs` is written to have no dependencies precisely so a build script
// can include it: a build script is a separate crate and cannot see the
// package's own dependency graph.
mod icon {
    include!("src/icon.rs");
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");

    // `CARGO_CFG_TARGET_OS` is the target, not the host — cross-compiling to
    // Windows from elsewhere still needs the resource.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let ico_path = out.join("quark.ico");

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in icon::ICO_SIZES {
        let image = ico::IconImage::from_rgba_data(size, size, icon::render(size as usize));
        dir.add_entry(
            ico::IconDirEntry::encode(&image).expect("the icon is valid RGBA of a known size"),
        );
    }
    let file = std::fs::File::create(&ico_path).expect("OUT_DIR is writable");
    dir.write(file).expect("writing the generated icon");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("OUT_DIR is valid UTF-8"));
    // Fills in the Details tab of the file's Properties dialog from Cargo.toml.
    res.set("FileDescription", "Quark — a PDF reader and editor");
    res.set("ProductName", "Quark");

    // A missing resource compiler must not take the whole build down with it.
    // Compiling resources needs `rc.exe` from the Windows SDK; without it the
    // right outcome is a binary with no icon and a visible warning, not a
    // developer who cannot build at all.
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the icon resource: {e}");
        println!(
            "cargo:warning=Quark will run, but its pinned and Explorer icons \
             will be blank. Install the Windows SDK to get rc.exe."
        );
    }
}
