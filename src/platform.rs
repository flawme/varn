//! Platform identification and OS-specific abstractions.
//!
//! Platform-specific code lives here so it does not leak into core logic.
//! As platform-specific behavior grows, submodules (`unix`, `windows`) will
//! be added behind `#[cfg]` gates.

use std::fs;
use std::path::Path;

/// Name of the current operating system.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

/// Whether the current platform is POSIX-compliant (Unix-like).
pub fn is_posix() -> bool {
    cfg!(unix)
}

/// Check whether a file or directory is read-only (owner lacks write
/// permission).
///
/// On Unix this inspects the permission mode bits. On Windows it checks the
/// read-only attribute. If metadata cannot be read, defaults to `false`.
pub fn is_readonly(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(meta) => is_readonly_meta(&meta),
        Err(_) => false,
    }
}

/// Check whether an entry is read-only given its [`fs::Metadata`].
pub fn is_readonly_meta(meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o200 == 0
    }
    #[cfg(not(unix))]
    {
        meta.permissions().readonly()
    }
}

/// Create a symbolic link at `link` pointing to `target`.
///
/// On Unix, this uses `symlink` which works for both files and directories.
/// On Windows, creating symlinks requires Developer Mode or administrator
/// privileges. File links use `symlink_file`; directory links use
/// `symlink_dir`.
pub fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks are not supported on this platform",
        ))
    }
}

/// Set macOS BSD file flags (`st_flags`) on a path.
///
/// Uses `lchflags(2)`. Best-effort by design: the caller treats
/// `PermissionDenied` as a warning, not a failure, because privileged flags
/// (e.g. `schg`, system immutable) require root.
#[cfg(target_os = "macos")]
pub fn set_bsd_flags(path: &Path, flags: u32) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // `lchflags` is not exposed by the libc crate on apple targets, so we
    // declare it directly. Signature from the Darwin man page:
    // int lchflags(const char *path, u_int flags);
    unsafe extern "C" {
        fn lchflags(path: *const std::ffi::c_char, flags: u32) -> std::ffi::c_int;
    }

    let c_path = CString::new(path.as_os_str().as_bytes())?;
    // Apply to the link itself, not the target.
    let rc = unsafe { lchflags(c_path.as_ptr(), flags) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Set Windows file attributes (READONLY, HIDDEN, SYSTEM, ARCHIVE, ...).
///
/// Uses `SetFileAttributesW`. Note that this replaces the attribute set
/// wholesale, matching how the attributes were captured.
#[cfg(windows)]
pub fn set_file_attributes(path: &Path, attributes: u32) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let rc = unsafe { SetFileAttributesW(wide.as_ptr(), attributes) };
    if rc == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Get the number of hard links to a file on Windows.
///
/// Opens the file and queries `GetFileInformationByHandle`. Returns `None`
/// if the file cannot be opened or the filesystem does not report a link
/// count (FAT/exFAT report 1). Directories always report 1.
#[cfg(windows)]
pub fn windows_link_count(path: &Path) -> Option<u32> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };

    if path.is_dir() {
        return Some(1);
    }

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(info.nNumberOfLinks)
}

/// Get the Windows security descriptor (owner, group, DACL) in SDDL form.
///
/// Uses `GetNamedSecurityInfoW` with `ConvertSecurityDescriptorToStringSecurityDescriptorW`.
/// Returns `None` if the descriptor cannot be read (never fails the scan).
#[cfg(windows)]
pub fn get_security_descriptor_sddl(path: &Path) -> Option<String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    };

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut psd: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let rc = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut psd,
        )
    };
    if rc != 0 || psd.is_null() {
        return None;
    }

    let mut sddl_ptr: *mut u16 = std::ptr::null_mut();
    let mut sddl_len: u32 = 0;
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            psd,
            windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut sddl_ptr,
            &mut sddl_len,
        )
    };
    let result = if ok != 0 && !sddl_ptr.is_null() {
        let slice = unsafe { std::slice::from_raw_parts(sddl_ptr, sddl_len as usize) };
        // sddl_len INCLUDES the null terminator. Trim trailing NULs so the
        // stored SDDL string is clean; a padded string made every
        // SetNamedSecurityInfoW call fail with ERROR_INVALID_PARAMETER
        // (os error 87) and ACLs were never restored.
        let trimmed: &[u16] = match slice.iter().rposition(|&c| c != 0) {
            Some(last) => &slice[..=last],
            None => &[],
        };
        Some(String::from_utf16_lossy(trimmed))
    } else {
        None
    };
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(sddl_ptr as _);
        windows_sys::Win32::Foundation::LocalFree(psd as _);
    }
    result
}

/// Apply a security descriptor in SDDL form to a path.
///
/// Uses `ConvertStringSecurityDescriptorToSecurityDescriptorW` +
/// `SetNamedSecurityInfoW`. Best-effort by design; the caller warns on
/// failure.
#[cfg(windows)]
pub fn set_security_descriptor(path: &Path, sddl: &str) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    };

    let sddl_wide: Vec<u16> = OsStr::new(sddl)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let path_wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut psd: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let mut len: u32 = 0;
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_wide.as_ptr(),
            windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION,
            &mut psd,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SetNamedSecurityInfoW takes the OWNER PSID, GROUP PSID, and DACL
    // pointer — NOT the whole security descriptor. Extract the DACL from
    // the converted descriptor via GetSecurityDescriptorDacl; passing the
    // SD itself as the PACL fails with ERROR_INVALID_PARAMETER (os 87).
    let mut dacl: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut dacl_present: i32 = 0;
    let mut dacl_defaulted: i32 = 0;
    let ok_dacl = unsafe {
        windows_sys::Win32::Security::GetSecurityDescriptorDacl(
            psd,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    };
    if ok_dacl == 0 {
        unsafe { windows_sys::Win32::Foundation::LocalFree(psd as _) };
        return Err(std::io::Error::last_os_error());
    }
    let rc = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            std::ptr::null_mut(),
        )
    };
    unsafe { windows_sys::Win32::Foundation::LocalFree(psd as _) };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn os_name_is_known() {
        let name = os_name();
        assert!(
            name == "linux" || name == "macos" || name == "windows" || name == "unknown",
            "unexpected os name: {name}"
        );
    }

    #[test]
    fn is_posix_matches_cfg() {
        assert_eq!(is_posix(), cfg!(unix));
    }

    #[test]
    fn is_readonly_writable_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("writable.txt");
        std::fs::write(&f, b"data").unwrap();
        assert!(!is_readonly(&f));
    }

    #[cfg(unix)]
    #[test]
    fn is_readonly_readonly_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("readonly.txt");
        std::fs::write(&f, b"data").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&f, perms).unwrap();
        assert!(is_readonly(&f));
    }

    #[test]
    fn is_readonly_missing_file_defaults_false() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does_not_exist");
        assert!(!is_readonly(&missing));
    }

    #[cfg(unix)]
    #[test]
    fn create_symlink_creates_link() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target.txt");
        std::fs::write(&target, b"hello").unwrap();
        let link = tmp.path().join("link.txt");

        create_symlink(&target, &link).unwrap();

        assert!(link.is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
    }

    #[cfg(unix)]
    #[test]
    fn create_symlink_creates_dir_link() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target_dir");
        std::fs::create_dir(&target).unwrap();
        let link = tmp.path().join("link_dir");

        create_symlink(&target, &link).unwrap();

        assert!(link.is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
    }
}
