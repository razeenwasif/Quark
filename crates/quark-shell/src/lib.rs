//! Windows shell integration: file associations, the Open With list, and
//! printing.
//!
//! # What Windows will and will not let an application do
//!
//! Quark can register itself as a *handler* for PDFs — which is what puts it in
//! the Open With list, in Settings ▸ Default apps, and in a file manager's
//! context menu. It **cannot** silently make itself the default.
//!
//! Since Windows 8, the `UserChoice` registry value that records the default
//! handler is protected by a hash over the ProgID, the user's SID and a salt
//! that Microsoft does not document. Writing it directly either fails or is
//! detected and reset, and Windows 10 shows a "an app default was reset"
//! notification when it happens. Applications that appear to do this are
//! either lying about it or are being run during OEM image creation.
//!
//! So [`register`] does the part that works, and [`prompt_set_default`] opens
//! the system dialog where the user confirms in one click. That is the same
//! path every well-behaved Windows application takes.

#![cfg_attr(not(windows), allow(unused))]

use std::path::{Path, PathBuf};

/// Errors from the shell layer.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("this operation is only available on Windows")]
    NotWindows,
    #[error("could not locate the Quark executable: {0}")]
    NoExe(String),
    #[error("registry operation failed: {0}")]
    Registry(String),
    #[error("{0}")]
    Shell(String),
}

/// The ProgID Quark registers under.
///
/// Versioned by convention so a future incompatible handler can coexist with
/// this one rather than silently replacing it.
pub const PROGID: &str = "Quark.Document.1";

/// The name shown in the Open With list and in Settings.
pub const APP_NAME: &str = "Quark";

/// Extensions Quark claims.
pub const EXTENSIONS: &[&str] = &[".pdf", ".fdf", ".xfdf"];

/// The running executable's path.
pub fn exe_path() -> Result<PathBuf, ShellError> {
    std::env::current_exe().map_err(|e| ShellError::NoExe(e.to_string()))
}

/// The `shell\open\command` value for an executable.
///
/// `%1` must be quoted: without the quotes a path containing a space arrives as
/// several arguments, which is why "Program Files" broke so many installers.
pub fn open_command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

/// The `shell\print\command` value.
pub fn print_command(exe: &Path) -> String {
    format!("\"{}\" --print \"%1\"", exe.display())
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
    };
    use windows::Win32::UI::Shell::{
        OAIF_EXEC, OAIF_FORCE_REGISTRATION, OAIF_REGISTER_EXT, OPENASINFO, SHCNE_ASSOCCHANGED,
        SHCNF_IDLIST, SHChangeNotify, SHOpenWithDialog, ShellExecuteW,
    };
    use windows::core::PCWSTR;

    /// A NUL-terminated UTF-16 buffer, as every wide Win32 call wants.
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Creates (or opens) a key under HKCU and sets one string value.
    ///
    /// `value_name` empty means the key's default value, which is where the
    /// shell looks for a command string.
    fn set_string(subkey: &str, value_name: &str, data: &str) -> Result<(), ShellError> {
        let path = wide(subkey);
        let mut key = HKEY::default();
        // SAFETY: `path` is NUL-terminated and outlives the call; `key` is a
        // valid out-pointer. Failure is reported through the return code
        // rather than through the handle.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE | KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(ShellError::Registry(format!(
                "could not create HKCU\\{subkey} (error {})",
                status.0
            )));
        }

        let name = wide(value_name);
        let value = wide(data);
        // `RegSetValueExW` takes a byte count, not a character count, and it
        // must include the NUL terminator. Passing the UTF-16 length instead
        // writes half the string and the value reads back truncated.
        //
        // SAFETY: `value` is a live `Vec<u16>`; reinterpreting it as bytes of
        // exactly twice the length stays inside the same allocation, and the
        // borrow ends before `value` is dropped.
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2)
        };
        let status = unsafe {
            RegSetValueExW(
                key,
                if value_name.is_empty() {
                    PCWSTR::null()
                } else {
                    PCWSTR(name.as_ptr())
                },
                None,
                REG_SZ,
                Some(bytes),
            )
        };
        // SAFETY: the key was successfully created above and is not used again.
        unsafe {
            let _ = RegCloseKey(key);
        }
        if status != ERROR_SUCCESS {
            return Err(ShellError::Registry(format!(
                "could not set HKCU\\{subkey}\\{value_name} (error {})",
                status.0
            )));
        }
        Ok(())
    }

    pub(super) fn register_exe(exe: &Path) -> Result<(), ShellError> {
        let exe_str = exe.display().to_string();

        // 1. The ProgID: what the file type *is*, and how to open it.
        let progid = format!("Software\\Classes\\{PROGID}");
        set_string(&progid, "", "PDF Document")?;
        set_string(&progid, "FriendlyTypeName", "PDF Document")?;
        set_string(
            &format!("{progid}\\DefaultIcon"),
            "",
            &format!("\"{exe_str}\",0"),
        )?;
        set_string(
            &format!("{progid}\\shell\\open\\command"),
            "",
            &open_command(&exe),
        )?;
        set_string(
            &format!("{progid}\\shell\\open"),
            "FriendlyAppName",
            APP_NAME,
        )?;
        set_string(
            &format!("{progid}\\shell\\print\\command"),
            "",
            &print_command(&exe),
        )?;

        // 2. The application entry: what Settings ▸ Default apps shows.
        let app = "Software\\Quark";
        set_string(
            &format!("{app}\\Capabilities"),
            "ApplicationName",
            APP_NAME,
        )?;
        set_string(
            &format!("{app}\\Capabilities"),
            "ApplicationDescription",
            "Read, annotate and edit PDF documents",
        )?;
        for ext in EXTENSIONS {
            set_string(
                &format!("{app}\\Capabilities\\FileAssociations"),
                ext,
                PROGID,
            )?;
        }
        // Announcing the application is what makes it appear in Settings.
        set_string(
            "Software\\RegisteredApplications",
            APP_NAME,
            &format!("{app}\\Capabilities"),
        )?;

        // 3. Offer Quark in the Open With list for each extension. This is the
        //    part that makes it selectable; it does not make it the default.
        for ext in EXTENSIONS {
            set_string(
                &format!("Software\\Classes\\{ext}\\OpenWithProgids"),
                PROGID,
                "",
            )?;
        }

        // 4. The application's own registration, so the shell can find the
        //    executable by name.
        set_string(
            &format!("Software\\Classes\\Applications\\{}", exe_file_name(&exe)),
            "FriendlyAppName",
            APP_NAME,
        )?;
        set_string(
            &format!(
                "Software\\Classes\\Applications\\{}\\shell\\open\\command",
                exe_file_name(&exe)
            ),
            "",
            &open_command(&exe),
        )?;
        set_string(
            &format!(
                "Software\\Classes\\Applications\\{}\\SupportedTypes",
                exe_file_name(&exe)
            ),
            ".pdf",
            "",
        )?;

        notify_shell();
        Ok(())
    }

    pub(super) fn unregister() -> Result<(), ShellError> {
        let exe = exe_path().ok();
        let mut keys = vec![
            format!("Software\\Classes\\{PROGID}"),
            "Software\\Quark".to_string(),
        ];
        if let Some(exe) = &exe {
            keys.push(format!(
                "Software\\Classes\\Applications\\{}",
                exe_file_name(exe)
            ));
        }
        for key in keys {
            let path = wide(&key);
            // SAFETY: `path` is NUL-terminated. A missing key returns an error
            // code, which is not a failure of the overall operation.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()));
            }
        }
        notify_shell();
        Ok(())
    }

    /// Opens the system "Open with" dialog for a file, with the option to make
    /// the choice permanent.
    pub(super) fn prompt_set_default(path: &Path) -> Result<(), ShellError> {
        let file = wide(&path.display().to_string());
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            // FORCE_REGISTRATION shows the "always use this app" checkbox,
            // which is the only supported way to change the default handler.
            oaifInFlags: OAIF_EXEC | OAIF_FORCE_REGISTRATION | OAIF_REGISTER_EXT,
        };
        // SAFETY: `file` outlives the call and `info` is fully initialised.
        unsafe {
            SHOpenWithDialog(None, &info)
                .map_err(|e| ShellError::Shell(format!("could not open the chooser: {e}")))
        }
    }

    /// Prints a document through the shell's registered print verb.
    ///
    /// Delegating rather than driving the printer directly means the user gets
    /// their normal printer dialog and their normal defaults.
    pub(super) fn print_document(path: &Path) -> Result<(), ShellError> {
        let verb = wide("print");
        let file = wide(&path.display().to_string());
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                windows::Win32::UI::WindowsAndMessaging::SW_HIDE,
            )
        };
        // ShellExecuteW returns a value above 32 on success; below is an error
        // code, which is a quirk of its 16-bit ancestry.
        if result.0 as isize <= 32 {
            return Err(ShellError::Shell(format!(
                "the shell refused to print (code {})",
                result.0 as isize
            )));
        }
        Ok(())
    }

    /// Tells the shell that file associations changed, so open windows and the
    /// Start menu pick it up without a sign-out.
    fn notify_shell() {
        // SAFETY: null pointers are valid for a global association change.
        unsafe {
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
        }
    }

    fn exe_file_name(exe: &Path) -> String {
        exe.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "quark.exe".into())
    }

    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };

    /// Stores a secret under `target`, replacing any existing one.
    pub(super) fn store_secret(target: &str, secret: &str) -> Result<(), ShellError> {
        let target_w = wide(target);
        // The blob is bytes, not a string: the API does not assume an encoding,
        // and UTF-8 is what we read back.
        let mut blob = secret.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: windows::core::PWSTR(target_w.as_ptr() as *mut u16),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: `cred` points at buffers that outlive the call, and the
        // lengths match the buffers they describe.
        unsafe { CredWriteW(&cred, 0) }
            .map_err(|e| ShellError::Shell(format!("could not store the credential: {e}")))
    }

    pub(super) fn load_secret(target: &str) -> Result<Option<String>, ShellError> {
        let target_w = wide(target);
        let mut ptr = std::ptr::null_mut();
        // SAFETY: `ptr` receives an allocation the API owns; it is freed with
        // `CredFree` below on every path that gets one.
        let read = unsafe {
            CredReadW(
                windows::core::PCWSTR(target_w.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut ptr,
            )
        };
        if read.is_err() || ptr.is_null() {
            // Not found is the ordinary first-run case, not a failure.
            return Ok(None);
        }
        // SAFETY: the call above succeeded, so `ptr` is a valid CREDENTIALW
        // whose blob pointer and length the API filled in.
        let secret = unsafe {
            let cred = &*ptr;
            let bytes =
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);
            let s = String::from_utf8_lossy(bytes).into_owned();
            CredFree(ptr as *const _);
            s
        };
        Ok(Some(secret))
    }

    pub(super) fn delete_secret(target: &str) -> Result<(), ShellError> {
        let target_w = wide(target);
        // SAFETY: the pointer is a valid null-terminated wide string.
        let _ = unsafe {
            CredDeleteW(
                windows::core::PCWSTR(target_w.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
            )
        };
        // Deleting something that was never stored is success, not an error:
        // callers use this to clear a key that may or may not be set.
        Ok(())
    }
}


/// Stores an API key in the Windows Credential Manager.
///
/// # Why not settings.toml
///
/// Quark's preferences are plaintext TOML in the user's profile. An API key
/// there is readable by anything that can read the file, ends up in backups and
/// in any support bundle, and survives uninstalling the application. The
/// credential manager encrypts at rest with the user's own credentials and is
/// the mechanism Windows provides for exactly this.
///
/// `target` is the credential name, e.g. `quark/anthropic`.
pub fn store_secret(target: &str, secret: &str) -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::store_secret(target, secret)
    }
    #[cfg(not(windows))]
    {
        let _ = (target, secret);
        Err(ShellError::NotWindows)
    }
}

/// Reads a stored API key back.
///
/// `Ok(None)` means there is no credential under that name — a first run, not
/// a failure.
pub fn load_secret(target: &str) -> Result<Option<String>, ShellError> {
    #[cfg(windows)]
    {
        win::load_secret(target)
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err(ShellError::NotWindows)
    }
}

/// Removes a stored API key. Deleting one that is not there is not an error.
pub fn delete_secret(target: &str) -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::delete_secret(target)
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        Err(ShellError::NotWindows)
    }
}

/// Where a per-user install of Quark lives.
///
/// `%LOCALAPPDATA%\Programs\Quark`, which is the convention for a per-user
/// install that needs no elevation and is where Windows expects to find one.
pub fn install_dir() -> Result<PathBuf, ShellError> {
    let base = std::env::var("LOCALAPPDATA")
        .map_err(|_| ShellError::Shell("LOCALAPPDATA is not set".into()))?;
    Ok(PathBuf::from(base).join("Programs").join(APP_NAME))
}

/// The files an install needs beside the executable.
///
/// PDFium is not optional: it is the first thing `quark_pdf::engine` looks for,
/// and an installed Quark without it starts and then fails to open anything.
const PAYLOAD: &[&str] = &["pdfium.dll", "LICENSE-pdfium"];

/// Copies Quark into [`install_dir`] and registers *that* copy as the handler.
///
/// # Why this exists
///
/// Registering the executable where it happens to be built points the shell at
/// a path inside build output. A `cargo clean` then deletes the handler out
/// from under Windows: file associations silently stop working and the taskbar
/// pin disappears, because Windows drops a pin whose target is gone. Copying to
/// a stable location first means the repository can be cleaned, moved or
/// deleted without touching the installation.
///
/// Returns the directory it installed into.
pub fn install() -> Result<PathBuf, ShellError> {
    #[cfg(windows)]
    {
        let source_exe = exe_path()?;
        let source_dir = source_exe
            .parent()
            .ok_or_else(|| ShellError::NoExe("the executable has no parent".into()))?;
        let dir = install_dir()?;
        let dest_exe = dir.join("quark.exe");

        std::fs::create_dir_all(&dir)
            .map_err(|e| ShellError::Shell(format!("creating {}: {e}", dir.display())))?;

        // Re-running the installed copy is a repair, not a copy: Windows locks
        // a running executable, so copying it onto itself fails. Registering is
        // still worth doing, which is what makes `--install` idempotent.
        if source_exe != dest_exe {
            std::fs::copy(&source_exe, &dest_exe).map_err(|e| {
                ShellError::Shell(format!(
                    "copying Quark to {}: {e}. Close any running copy and try again",
                    dest_exe.display()
                ))
            })?;
        }

        for name in PAYLOAD {
            let from = source_dir.join(name);
            if !from.exists() {
                // Only PDFium is fatal; the licence file is a courtesy.
                if *name == "pdfium.dll" {
                    return Err(ShellError::Shell(format!(
                        "{} is missing from {}. Install from a staged `dist` folder, \
                         not straight out of `target`",
                        name,
                        source_dir.display()
                    )));
                }
                continue;
            }
            let to = dir.join(name);
            if from != to {
                std::fs::copy(&from, &to)
                    .map_err(|e| ShellError::Shell(format!("copying {name}: {e}")))?;
            }
        }

        win::register_exe(&dest_exe)?;
        Ok(dir)
    }
    #[cfg(not(windows))]
    {
        Err(ShellError::NotWindows)
    }
}

/// Registers Quark as a PDF handler for the current user.
///
/// Writes only under `HKEY_CURRENT_USER`, so no elevation is needed and the
/// registration cannot affect other accounts on the machine.
pub fn register() -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::register_exe(&exe_path()?)
    }
    #[cfg(not(windows))]
    {
        Err(ShellError::NotWindows)
    }
}

/// Removes Quark's file associations.
pub fn unregister() -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::unregister()
    }
    #[cfg(not(windows))]
    {
        Err(ShellError::NotWindows)
    }
}

/// Opens the Windows chooser so the user can make Quark the default.
pub fn prompt_set_default(path: &Path) -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::prompt_set_default(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(ShellError::NotWindows)
    }
}

/// Prints a document.
pub fn print_document(path: &Path) -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::print_document(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(ShellError::NotWindows)
    }
}

/// Attaches Quark to the console it was launched from, if there is one.
///
/// In release Quark is a GUI-subsystem binary, which is what stops Windows
/// opening an empty terminal behind the window when a PDF is double-clicked.
/// The cost is that such a process starts with no standard output at all, so
/// `--version`, `--help` and the registration flags would otherwise print into
/// nothing. Borrowing the parent's console gives them somewhere to write on the
/// one path where a user can actually read it.
///
/// Returns `false` when there was no console to attach to. That is the normal
/// case for a double-click from Explorer, not an error.
pub fn attach_parent_console() -> bool {
    #[cfg(windows)]
    {
        // SAFETY: `AttachConsole` takes no pointers and borrows nothing. It
        // fails harmlessly when the parent has no console, or when this
        // process already has one.
        unsafe {
            windows::Win32::System::Console::AttachConsole(
                windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
            )
            .is_ok()
        }
    }
    #[cfg(not(windows))]
    {
        // Everywhere else a process simply inherits its parent's stdio.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_directory_is_a_per_user_programs_folder() {
        // Per-user so it needs no elevation, and under Programs because that is
        // where Windows and every other per-user app put themselves.
        let Ok(dir) = install_dir() else {
            // No LOCALAPPDATA (a bare CI container); nothing to assert.
            return;
        };
        let s = dir.display().to_string();
        assert!(s.ends_with("Quark"), "{s}");
        assert!(s.contains("Programs"), "{s}");
    }

    #[test]
    fn the_install_carries_pdfium_with_it() {
        // An installed Quark without PDFium starts and then fails to open
        // anything, which is a worse failure than not installing at all.
        assert!(
            PAYLOAD.contains(&"pdfium.dll"),
            "the install would leave PDFium behind"
        );
    }

    #[test]
    fn the_registered_command_quotes_the_path_and_the_argument() {
        // Program Files and LOCALAPPDATA both contain spaces on plenty of
        // machines; an unquoted command breaks on every one of them.
        let exe = Path::new(r"C:\Users\a b\AppData\Local\Programs\Quark\quark.exe");
        let cmd = open_command(exe);
        assert!(cmd.starts_with('"'), "{cmd}");
        assert!(cmd.contains(r#"" "%1""#), "{cmd}");
        assert!(print_command(exe).contains("--print"));
    }

    #[test]
    fn the_open_command_quotes_the_argument() {
        // Without the quotes, "C:\Program Files\..." arrives as two arguments
        // and the document silently fails to open.
        let cmd = open_command(Path::new(r"C:\Program Files\Quark\quark.exe"));
        assert_eq!(cmd, r#""C:\Program Files\Quark\quark.exe" "%1""#);
        assert!(cmd.ends_with(r#""%1""#));
    }

    #[test]
    fn the_print_command_passes_the_print_flag() {
        let cmd = print_command(Path::new(r"C:\Quark\quark.exe"));
        assert!(cmd.contains("--print"));
        assert!(cmd.ends_with(r#""%1""#));
    }

    #[test]
    fn the_progid_is_versioned() {
        // An unversioned ProgID cannot be superseded without breaking the old
        // registration.
        assert!(PROGID.ends_with(".1"));
        assert!(PROGID.starts_with("Quark."));
    }

    #[test]
    fn every_claimed_extension_has_a_leading_dot() {
        // The registry keys are literally `.pdf`; a missing dot writes to the
        // wrong key and the association silently does nothing.
        for e in EXTENSIONS {
            assert!(e.starts_with('.'), "{e} is missing its dot");
            assert_eq!(e.to_lowercase(), **e, "{e} should be lower case");
        }
    }

    #[test]
    fn pdf_is_among_the_claimed_extensions() {
        assert!(EXTENSIONS.contains(&".pdf"));
    }

    #[cfg(not(windows))]
    #[test]
    fn the_windows_entry_points_refuse_politely_elsewhere() {
        assert!(matches!(register(), Err(ShellError::NotWindows)));
        assert!(matches!(unregister(), Err(ShellError::NotWindows)));
        assert!(matches!(
            print_document(Path::new("/tmp/x.pdf")),
            Err(ShellError::NotWindows)
        ));
    }
}

#[cfg(all(test, windows))]
mod credential_tests {
    use super::*;

    /// A name no real install uses, so a failed run cannot clobber a real key.
    const TEST_TARGET: &str = "quark/test-credential-roundtrip";

    #[test]
    fn a_secret_round_trips_through_the_credential_manager() {
        // This is the whole point of the mechanism: if it does not come back
        // byte-for-byte, keys silently stop working after a restart.
        let secret = "sk-ant-test-\u{00e9}\u{4e2d}-0123456789";
        store_secret(TEST_TARGET, secret).expect("store");
        let back = load_secret(TEST_TARGET).expect("load");
        assert_eq!(back.as_deref(), Some(secret));

        delete_secret(TEST_TARGET).expect("delete");
        assert_eq!(
            load_secret(TEST_TARGET).expect("load after delete"),
            None,
            "the credential outlived its deletion"
        );
    }

    #[test]
    fn an_absent_credential_reads_as_none_rather_than_an_error() {
        // First run has no key stored. Treating that as a failure would show
        // an error dialog to every new user.
        let missing = load_secret("quark/test-definitely-not-stored").expect("load");
        assert_eq!(missing, None);
    }

    #[test]
    fn deleting_something_that_was_never_stored_is_not_an_error() {
        // Callers clear a key that may or may not be set.
        delete_secret("quark/test-definitely-not-stored").expect("delete");
    }
}
