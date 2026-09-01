//! Linux-specific regression suite.
//!
//! Compiled and run only on Linux targets. Covers mode bits, uid/gid,
//! symlink semantics unique to POSIX, and permission-based lock behavior.

mod mode_bits;
mod ownership;
mod posix_locks;
mod posix_symlinks;
