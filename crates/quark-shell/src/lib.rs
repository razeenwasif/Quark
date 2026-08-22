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

    pub(super) fn register() -> Result<(), ShellError> {
        let exe = exe_path()?;
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
}

/// Registers Quark as a PDF handler for the current user.
///
/// Writes only under `HKEY_CURRENT_USER`, so no elevation is needed and the
/// registration cannot affect other accounts on the machine.
pub fn register() -> Result<(), ShellError> {
    #[cfg(windows)]
    {
        win::register()
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

#[cfg(test)]
mod tests {
    use super::*;

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
