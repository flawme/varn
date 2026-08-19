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
}
