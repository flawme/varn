//! Platform identification and OS-specific abstractions.
//!
//! Platform-specific code lives here so it does not leak into core logic.
//! As platform-specific behavior grows, submodules (`unix`, `windows`) will
//! be added behind `#[cfg]` gates.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
