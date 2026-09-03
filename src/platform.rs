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

/// Return whether `path` is an NTFS junction (a mount-point reparse point).
///
/// Rust's `FileType` exposes both junctions and symbolic links as symlinks.
/// Windows' reparse tag is the authoritative distinction. Failure to inspect
/// a reparse point is intentionally treated as "not a junction" so scanning
/// remains non-fatal for inaccessible entries.
pub fn is_junction(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FileAttributeTagInfo, GetFileInformationByHandleEx, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;

        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        unsafe { CloseHandle(handle) };
        ok != 0 && tag.ReparseTag == IO_REPARSE_TAG_MOUNT_POINT
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Create an NTFS junction at `link` pointing to `target`.
///
/// A junction is a mount-point reparse point, not a directory symlink. The
/// reparse buffer format is documented by `FSCTL_SET_REPARSE_POINT`; using
/// it directly avoids invoking `cmd.exe` and keeps paths with spaces or
/// shell metacharacters safe.
#[cfg(windows)]
pub fn create_junction(target: &Path, link: &Path) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    use windows_sys::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;

    // Junctions require an absolute target. Canonicalization also removes a
    // potentially ambiguous `.`/`..` component from a captured target.
    let target = normalize_junction_target(target);
    let target = std::fs::canonicalize(target)?;
    let raw_print_name: Vec<u16> = target.as_os_str().encode_wide().collect();
    // `canonicalize` can return an extended `\\?\\` path. The print name
    // should remain a normal Win32 path, while the substitute name below
    // receives its required NT namespace prefix.
    let print_name: Vec<u16> =
        if raw_print_name.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]) {
            raw_print_name[4..].to_vec()
        } else {
            raw_print_name
        };
    let substitute_name = junction_substitute_name(&print_name)?;
    let substitute_bytes = substitute_name
        .len()
        .checked_mul(2)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "junction target is too long",
            )
        })?;
    let print_bytes = print_name
        .len()
        .checked_mul(2)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "junction target is too long",
            )
        })?;
    // REPARSE_DATA_BUFFER's mount-point payload starts with four u16 fields
    // (offset/length pairs), then the two names. ReparseDataLength excludes
    // the 8-byte common header.
    let names_bytes = substitute_name
        .len()
        .checked_add(print_name.len())
        .and_then(|n| n.checked_mul(2))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "junction target is too long",
            )
        })?;
    let data_len = 8usize
        .checked_add(names_bytes)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "junction target is too long",
            )
        })?;
    let mut buffer: Vec<u16> = vec![
        (IO_REPARSE_TAG_MOUNT_POINT & 0xffff) as u16,
        (IO_REPARSE_TAG_MOUNT_POINT >> 16) as u16,
        data_len,
        0,
        0,
        substitute_bytes,
        substitute_bytes,
        print_bytes,
    ];
    buffer.extend_from_slice(&substitute_name);
    buffer.extend_from_slice(&print_name);

    std::fs::create_dir(link)?;
    let wide_link: Vec<u16> = OsStr::new(link)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide_link.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let err = std::io::Error::last_os_error();
        let _ = std::fs::remove_dir(link);
        return Err(err);
    }
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            (buffer.len() * std::mem::size_of::<u16>()) as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        let _ = std::fs::remove_dir(link);
        return Err(err);
    }
    Ok(())
}

/// Translate the NT-native spelling returned by some junction APIs
/// (`\\??\\C:\\...`) to the Win32 extended-path spelling accepted by the
/// standard library (`\\\\?\\C:\\...`). `read_link` is allowed to return
/// either spelling, so restore normalizes before canonicalizing the target.
#[cfg(windows)]
fn normalize_junction_target(target: &Path) -> std::path::PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    const BACKSLASH: u16 = b'\\' as u16;
    const QUESTION: u16 = b'?' as u16;
    let raw: Vec<u16> = target.as_os_str().encode_wide().collect();
    if raw.starts_with(&[BACKSLASH, QUESTION, QUESTION, BACKSLASH]) {
        let mut win32 = vec![BACKSLASH, BACKSLASH, QUESTION, BACKSLASH];
        win32.extend_from_slice(&raw[4..]);
        std::path::PathBuf::from(OsString::from_wide(&win32))
    } else {
        target.to_path_buf()
    }
}

/// Convert an absolute Win32 target path to a junction substitute name.
///
/// Local paths use `\\??\\C:\\...`; UNC paths use
/// `\\??\\UNC\\server\\share\\...`. The input from `canonicalize` may
/// carry a `\\?\\` extended-path prefix, which is removed for the printable
/// name and converted for the substitute name.
#[cfg(windows)]
fn junction_substitute_name(print_name: &[u16]) -> std::io::Result<Vec<u16>> {
    const BACKSLASH: u16 = b'\\' as u16;
    const QUESTION: u16 = b'?' as u16;
    const U: u16 = b'U' as u16;
    const N: u16 = b'N' as u16;
    const C: u16 = b'C' as u16;

    let name = if print_name.starts_with(&[BACKSLASH, BACKSLASH, QUESTION, BACKSLASH]) {
        &print_name[4..]
    } else {
        print_name
    };
    if name.len() >= 2 && name[0] == BACKSLASH && name[1] == BACKSLASH {
        let mut result = vec![BACKSLASH, QUESTION, QUESTION, BACKSLASH, U, N, C, BACKSLASH];
        result.extend_from_slice(&name[2..]);
        return Ok(result);
    }
    // An absolute DOS path starts with a drive designator, for example C:\\.
    if name.len() >= 3 && name[1] == b':' as u16 && name[2] == BACKSLASH {
        let mut result = vec![BACKSLASH, QUESTION, QUESTION, BACKSLASH];
        result.extend_from_slice(name);
        return Ok(result);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "junction target must be an absolute local or UNC path",
    ))
}

#[cfg(not(windows))]
pub fn create_junction(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "junctions are only supported on Windows",
    ))
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
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

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
    let mut dacl: *mut windows_sys::Win32::Security::ACL = std::ptr::null_mut();
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
    // Only the DACL is applied: the owner/group pointers are null, so the
    // corresponding flags must NOT be set (a set flag with a null pointer
    // is ERROR_INVALID_PARAMETER). Owner/group restore via SDDL is future
    // work once the SDDL round-trip is proven end to end.
    let rc = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
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
