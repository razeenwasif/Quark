//! Quark — a PDF reader and editor.
//!
//! Release builds target the Windows GUI subsystem, so double-clicking a PDF
//! does not open an empty console behind the window. Debug builds keep the
//! console, because that is where `QUARK_LOG` tracing output goes and losing it
//! during development costs more than the stray window.
//!
//! A GUI-subsystem process has no stdio of its own, so the command-line flags
//! below borrow the parent terminal's console — see `attach_parent_console`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assistant;
mod chrome;
mod dialogs;
mod home;
/// Quark's application icon, shared with `build.rs` so the pinned Explorer
/// icon and the runtime window icon are generated from one definition.
mod icon;
mod panels;
mod shortcuts;
mod tab;
mod textures;
mod viewer;

use std::path::PathBuf;

/// Parses the command line.
///
/// The only positional arguments are files to open, which is what the Windows
/// shell passes when Quark is the default handler for a PDF. Flags are handled
/// before the window is created so that `--register` can run headless.
struct Args {
    files: Vec<PathBuf>,
    install: bool,
    register: bool,
    unregister: bool,
    print: Option<PathBuf>,
    help: bool,
    version: bool,
}

fn parse_args(argv: impl Iterator<Item = String>) -> Args {
    let mut a = Args {
        files: Vec::new(),
        install: false,
        register: false,
        unregister: false,
        print: None,
        help: false,
        version: false,
    };
    let mut it = argv.peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--install" => a.install = true,
            "--register" => a.register = true,
            "--unregister" => a.unregister = true,
            "--print" => {
                if let Some(p) = it.next() {
                    a.print = Some(PathBuf::from(p));
                }
            }
            "-h" | "--help" => a.help = true,
            "-V" | "--version" => a.version = true,
            // A leading dash is a flag we do not know; treating it as a
            // filename would try to open something nonsensical.
            s if s.starts_with('-') => {
                eprintln!("quark: unknown option {s}");
            }
            s => a.files.push(PathBuf::from(s)),
        }
    }
    a
}

const HELP: &str = "\
Quark — a PDF reader and editor

USAGE:
    quark [OPTIONS] [FILE...]

OPTIONS:
    --install        Install Quark for the current user and register it
    --register       Register Quark where it stands, without installing
    --unregister     Remove Quark's file associations
    --print <FILE>   Print a document and exit
    -h, --help       Show this message
    -V, --version    Show the version
";

fn main() -> eframe::Result<()> {
    let args = parse_args(std::env::args().skip(1));

    // Every path below this point that writes to stdout or stderr needs a
    // console to write to, and a GUI-subsystem process has none of its own.
    // Only the flag paths print, so the GUI never borrows a console and never
    // makes a stray window appear.
    #[cfg(windows)]
    if args.help || args.version || args.install || args.register || args.unregister || args.print.is_some() {
        quark_shell::attach_parent_console();
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("QUARK_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    if args.help {
        print!("{HELP}");
        return Ok(());
    }
    if args.version {
        println!("quark {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    #[cfg(windows)]
    {
        if args.install {
            return match quark_shell::install() {
                Ok(dir) => {
                    println!("Quark is installed in {}", dir.display());
                    println!("It now appears in Open With and in Settings ▸ Default apps.");
                    println!(
                        "Pin the copy in that folder, not the one you built — \
                         cleaning the build directory cannot break it."
                    );
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Install failed: {e}");
                    std::process::exit(1);
                }
            };
        }
        if args.register {
            return match quark_shell::register() {
                Ok(()) => {
                    println!("Quark is registered as a PDF handler.");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Registration failed: {e}");
                    std::process::exit(1);
                }
            };
        }
        if args.unregister {
            return match quark_shell::unregister() {
                Ok(()) => {
                    println!("Quark's file associations were removed.");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Could not remove the associations: {e}");
                    std::process::exit(1);
                }
            };
        }
        if let Some(path) = &args.print {
            return match quark_shell::print_document(path) {
                Ok(()) => Ok(()),
                Err(e) => {
                    eprintln!("Could not print: {e}");
                    std::process::exit(1);
                }
            };
        }
    }
    #[cfg(not(windows))]
    {
        if args.install || args.register || args.unregister || args.print.is_some() {
            eprintln!("quark: file association and printing are Windows-only");
            std::process::exit(1);
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Quark")
            .with_inner_size([1360.0, 900.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(load_icon()),
        ..Default::default()
    };

    let files = args.files;
    eframe::run_native(
        "Quark",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, files)))),
    )
}

/// The window and taskbar icon of the *running* process.
///
/// This is only half the story: a pinned shortcut is drawn by Explorer from
/// the icon resource `build.rs` compiles into the executable, not from this.
/// Both come from [`icon::render`] so they cannot disagree.
fn load_icon() -> egui::IconData {
    // 128 rather than 64: Windows scales the taskbar icon up on high-DPI
    // displays, and upscaling a 64px source is visibly soft.
    const S: u32 = 128;
    egui::IconData {
        rgba: icon::render(S as usize),
        width: S,
        height: S,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn bare_paths_are_files_to_open() {
        // This is what the Windows shell passes when Quark handles a PDF.
        let a = args(&["C:\\Users\\me\\report.pdf"]);
        assert_eq!(a.files.len(), 1);
        assert!(!a.register);
    }

    #[test]
    fn several_files_all_open() {
        let a = args(&["a.pdf", "b.pdf", "c.pdf"]);
        assert_eq!(a.files.len(), 3);
    }


    #[test]
    fn install_is_recognised_and_distinct_from_register() {
        // They are different operations: --install copies to a stable location
        // and registers that, --register registers wherever the binary stands.
        let a = args(&["--install"]);
        assert!(a.install);
        assert!(!a.register, "--install must not imply --register");
        let b = args(&["--register"]);
        assert!(b.register);
        assert!(!b.install);
    }
    #[test]
    fn registration_flags_are_recognised() {
        assert!(args(&["--register"]).register);
        assert!(args(&["--unregister"]).unregister);
    }

    #[test]
    fn the_print_flag_takes_a_path() {
        let a = args(&["--print", "doc.pdf"]);
        assert_eq!(a.print, Some(PathBuf::from("doc.pdf")));
        assert!(a.files.is_empty(), "the printed file is not also opened");
    }

    #[test]
    fn a_dangling_print_flag_does_not_panic() {
        let a = args(&["--print"]);
        assert_eq!(a.print, None);
    }

    #[test]
    fn unknown_flags_are_not_mistaken_for_filenames() {
        let a = args(&["--nonsense", "real.pdf"]);
        assert_eq!(a.files, vec![PathBuf::from("real.pdf")]);
    }

    #[test]
    fn help_and_version_are_recognised_in_both_forms() {
        assert!(args(&["-h"]).help);
        assert!(args(&["--help"]).help);
        assert!(args(&["-V"]).version);
        assert!(args(&["--version"]).version);
    }


    #[test]
    fn the_icon_survives_every_size_windows_asks_for() {
        // The `.ico` ships all of these, and 16px is the real test: every
        // dimension is a fraction of the size, so at that scale a bond or a
        // dot can round away to nothing or flood the whole tile.
        for size in icon::ICO_SIZES {
            let s = size as usize;
            let rgba = icon::render(s);
            assert_eq!(rgba.len(), s * s * 4, "{size}px is the wrong length");

            let centre = ((s / 2) * s + s / 2) * 4;
            assert_eq!(rgba[centre + 3], 255, "{size}px centre is not solid");
            assert_eq!(rgba[3], 0, "{size}px corner is not transparent");

            // Still a mark rather than a flat slab. This is what "reads at
            // 16px" has to mean in a test.
            let shades: std::collections::HashSet<_> = rgba
                .chunks_exact(4)
                .filter(|px| px[3] == 255)
                .map(|px| (px[0] / 32, px[1] / 32, px[2] / 32))
                .collect();
            assert!(
                shades.len() >= 3,
                "{size}px rendered only {} distinct shades",
                shades.len()
            );
        }
    }
    #[test]
    fn the_icon_is_the_size_it_claims() {
        let icon = load_icon();
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
        assert!(icon.width > 0);
    }

    #[test]
    fn the_icon_centre_is_opaque_and_the_corner_is_not() {
        let icon = load_icon();
        let s = icon.width as usize;
        let centre = ((s / 2) * s + s / 2) * 4;
        assert_eq!(icon.rgba[centre + 3], 255, "the tile should be solid");
        assert_eq!(icon.rgba[3], 0, "the corner should be transparent");
    }

    /// Samples the icon at a point given as a fraction of its size.
    fn icon_pixel(icon: &egui::IconData, fx: f32, fy: f32) -> [u8; 4] {
        let s = icon.width as usize;
        let x = ((fx * s as f32) as usize).min(s - 1);
        let y = ((fy * s as f32) as usize).min(s - 1);
        let i = (y * s + x) * 4;
        [
            icon.rgba[i],
            icon.rgba[i + 1],
            icon.rgba[i + 2],
            icon.rgba[i + 3],
        ]
    }

    #[test]
    fn the_three_quarks_are_three_different_colours() {
        // A triplet that renders in one colour is a smudge at taskbar size,
        // which is the whole reason the dots are not brand purple.
        let icon = load_icon();
        // Matches the -90/30/150 degree placement at 0.2 of the icon size.
        let top = icon_pixel(&icon, 0.5, 0.5 - 0.2);
        let lower_right = icon_pixel(&icon, 0.5 + 0.173, 0.5 + 0.1);
        let lower_left = icon_pixel(&icon, 0.5 - 0.173, 0.5 + 0.1);

        for (name, px) in [
            ("top", top),
            ("lower right", lower_right),
            ("lower left", lower_left),
        ] {
            assert_eq!(px[3], 255, "the {name} quark should be opaque");
        }
        // Each quark leads on a different channel: pink, green, blue.
        assert!(top[0] > top[1] && top[0] > top[2], "top: {top:?}");
        assert!(
            lower_right[1] > lower_right[0] && lower_right[1] > lower_right[2],
            "lower right: {lower_right:?}"
        );
        assert!(
            lower_left[2] > lower_left[0] && lower_left[2] > lower_left[1],
            "lower left: {lower_left:?}"
        );
    }

    #[test]
    fn the_icon_is_a_tile_not_a_disc() {
        // The corners are rounded but the edge midpoints are not, which is what
        // separates this silhouette from the plain disc it replaced.
        let icon = load_icon();
        let edge_middle = icon_pixel(&icon, 0.5, 0.06);
        assert_eq!(edge_middle[3], 255, "the top edge should be solid tile");
        let corner = icon_pixel(&icon, 0.06, 0.06);
        assert_eq!(corner[3], 0, "the corner should be cut away");
    }

    #[test]
    fn the_icon_has_no_premultiplied_fringe() {
        // Un-premultiplying with a zero alpha divides by zero; a fringe of
        // black-but-transparent pixels is how that shows up.
        let icon = load_icon();
        for px in icon.rgba.chunks_exact(4) {
            if px[3] == 0 {
                assert_eq!(
                    [px[0], px[1], px[2]],
                    [0, 0, 0],
                    "transparent pixel carries colour: {px:?}"
                );
            }
        }
    }
}
