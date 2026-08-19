//! Error types for Varn.
//!
//! All operations return [`Result<T, VarnError>`]. Errors are actionable and
//! include context about what failed and why.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, VarnError>;

/// The unified error type for all Varn operations.
#[derive(Debug)]
pub enum VarnError {
    /// A filesystem I/O error.
    Io(io::Error),
    /// A serialization or deserialization error.
    Json(serde_json::Error),
    /// A Varn repository already exists at the given path.
    AlreadyInitialized { path: PathBuf },
    /// No Varn repository was found at or above the given path.
    NotInitialized { searched: PathBuf },
    /// A path was invalid or unusable for the requested operation.
    InvalidPath(String),
    /// The requested feature is not yet implemented.
    NotImplemented(&'static str),
    /// A catch-all for operational errors that don't fit another variant.
    Other(String),
}

impl fmt::Display for VarnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "serialization error: {e}"),
            Self::AlreadyInitialized { path } => {
                write!(f, "Varn is already initialized at {}", path.display())
            }
            Self::NotInitialized { searched } => {
                write!(
                    f,
                    "Varn is not initialized (searched: {})",
                    searched.display()
                )
            }
            Self::InvalidPath(msg) => write!(f, "invalid path: {msg}"),
            Self::NotImplemented(what) => {
                write!(f, "{what} is not yet implemented in this version")
            }
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for VarnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for VarnError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for VarnError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_initialized_display() {
        let err = VarnError::AlreadyInitialized {
            path: PathBuf::from("/tmp/test/.varn"),
        };
        assert_eq!(
            err.to_string(),
            "Varn is already initialized at /tmp/test/.varn"
        );
    }

    #[test]
    fn not_initialized_display() {
        let err = VarnError::NotInitialized {
            searched: PathBuf::from("/tmp/test"),
        };
        assert_eq!(
            err.to_string(),
            "Varn is not initialized (searched: /tmp/test)"
        );
    }

    #[test]
    fn not_implemented_display() {
        let err = VarnError::NotImplemented("checkpoint");
        assert_eq!(
            err.to_string(),
            "checkpoint is not yet implemented in this version"
        );
    }

    #[test]
    fn io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "missing");
        let varn_err: VarnError = io_err.into();
        assert!(matches!(varn_err, VarnError::Io(_)));
    }
}
