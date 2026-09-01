//! Windows-specific regression suite.
//!
//! Compiled and run only on Windows targets. Covers file attributes, the
//! SDDL security descriptor path, NTFS hard links, junctions, and Windows
//! path forms.

mod acl;
mod attributes;
mod hardlinks;
mod junctions;
mod win_paths;
