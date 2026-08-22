//! Quark — a PDF reader and editor.

mod app;
mod chrome;
mod dialogs;
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
    register: bool,
    unregister: bool,
    print: Option<PathBuf>,
    help: bool,
    version: bool,
}

fn parse_args(argv: impl Iterator<Item = String>) -> Args {
    let mut a = Args {
        files: Vec::new(),
        register: false,
        unregister: false,
        print: None,
        help: false,
        version: false,
    };
    let mut it = argv.peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
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
    --register       Register Quark as a PDF handler for the current user
    --unregister     Remove Quark's file associations
    --print <FILE>   Print a document and exit
    -h, --help       Show this message
    -V, --version    Show the version
";

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("QUARK_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args = parse_args(std::env::args().skip(1));

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
        if args.register || args.unregister || args.print.is_some() {
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

/// The window and taskbar icon.
///
/// Drawn rather than loaded from a file so the binary stays self-contained and
/// cannot start up without its icon.
fn load_icon() -> egui::IconData {
    const S: usize = 64;
    let mut rgba = vec![0u8; S * S * 4];
    let c = (S as f32 - 1.0) / 2.0;
    for y in 0..S {
        for x in 0..S {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let i = (y * S + x) * 4;
            // A purple disc with a soft edge, matching the accent colour.
            let a = ((28.0 - d) / 3.0).clamp(0.0, 1.0);
            let t = (d / 28.0).clamp(0.0, 1.0);
            rgba[i] = (0xC0 as f32 * (1.0 - t) + 0x7E as f32 * t) as u8;
            rgba[i + 1] = (0x84 as f32 * (1.0 - t) + 0x22 as f32 * t) as u8;
            rgba[i + 2] = (0xFC as f32 * (1.0 - t) + 0xCE as f32 * t) as u8;
            rgba[i + 3] = (a * 255.0) as u8;
        }
    }
    egui::IconData {
        rgba,
        width: S as u32,
        height: S as u32,
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
        assert_eq!(icon.rgba[centre + 3], 255, "the disc should be solid");
        assert_eq!(icon.rgba[3], 0, "the corner should be transparent");
    }
}
