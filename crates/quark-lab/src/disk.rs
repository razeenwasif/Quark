//! How much room there is on a disk.

use std::path::Path;

/// The free space where `path` is, or where its nearest folder that exists is.
pub fn free_bytes(path: &Path) -> u64 {
    let mut at = path;
    while !at.exists() {
        match at.parent() {
            Some(parent) => at = parent,
            None => return 0,
        }
    }
    free_at(at)
}

#[cfg(unix)]
fn free_at(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return 0;
    };
    // SAFETY: statvfs fills the struct it is given from a NUL-terminated path.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut stat) != 0 {
            return 0;
        }
        (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64)
    }
}

#[cfg(not(unix))]
fn free_at(_: &Path) -> u64 {
    0
}
