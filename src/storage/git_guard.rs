//! Git coexistence guard for the `.varn/` store.
//!
//! Varn stores tens of thousands of content objects inside `.varn/`. If the
//! managed directory is also a Git repository and `.varn/` is not excluded,
//! a blind `git add -A` stages the entire object store. This module prevents
//! that in two layers:
//!
//! 1. **Store-level guard** (automatic): `varn init` creates
//!    `.varn/.gitignore` containing `*`. Git applies `.gitignore` files to
//!    their own directory and below, so this alone makes Git ignore the whole
//!    store — without Varn ever touching anything outside `.varn/`.
//! 2. **Detection and advice** (advisory): for stores created before the
//!    guard existed, [`assess`] reports whether the store is effectively
//!    excluded, and commands surface a warning with a copy-pasteable fix.
//!
//! [`append_to_gitignore`] supports the explicit `varn init --gitignore`
//! flag, which adds `.varn/` to the enclosing repository's root `.gitignore`.
//!
//! ## Conservatism
//!
//! The assessment reads only the root `.gitignore` and `.git/info/exclude`.
//! It does not evaluate nested `.gitignore` files in intermediate directories
//! or full gitignore glob semantics. A store that is in fact excluded by such
//! an exotic setup may still produce a warning; the warning is advisory and
//! never blocks a command.

use crate::error::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Name of the guard file inside `.varn/`.
pub const GUARD_FILE: &str = ".gitignore";

/// Content written to the guard file. `*` ignores everything in `.varn/`.
pub const GUARD_CONTENT: &str = "*\n";

/// Spellings of a root-level ignore entry that exclude `.varn/`.
const ROOT_ENTRY_SPELLINGS: &[&str] = &[".varn", ".varn/", "/.varn", "/.varn/"];

/// Result of [`append_to_gitignore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitignoreUpdate {
    /// The `.gitignore` file did not exist and was created.
    Created,
    /// `.varn/` was appended to an existing `.gitignore`.
    Appended,
    /// `.varn/` (in some recognized spelling) was already present.
    AlreadyPresent,
}

/// The git-coexistence status of a Varn store.
#[derive(Debug, Clone)]
pub struct GitCoexistence {
    /// The enclosing git work tree root, if the store is inside one.
    ///
    /// `.git` may be a directory (normal repository) or a file (worktrees
    /// and submodules); both are recognized.
    pub git_root: Option<PathBuf>,
    /// Whether Git would ignore the store's contents.
    ///
    /// `true` when the store-level guard is present, or the root `.gitignore`
    /// / `.git/info/exclude` excludes `.varn/`.
    pub store_ignored: bool,
}

/// Walk upward from `start` looking for a `.git` entry (directory or file).
///
/// The returned root is fully resolved (symlinks and `.` components removed)
/// so callers can display it directly.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let start = fs::canonicalize(start).unwrap_or_else(|_| {
        if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(start))
                .unwrap_or_else(|_| start.to_path_buf())
        }
    });

    let mut current: Option<&Path> = Some(&start);
    while let Some(dir) = current {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() || dot_git.is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Whether the store-level guard file exists inside `varn_dir`.
pub fn guard_present(varn_dir: &Path) -> bool {
    varn_dir.join(GUARD_FILE).is_file()
}

/// Create the store-level git guard (`.varn/.gitignore` containing `*`) if
/// it does not already exist.
///
/// Returns `true` if the file was created, `false` if it was already present.
/// Existing guard files are never overwritten, so a user-customized guard is
/// preserved.
pub fn ensure_guard(varn_dir: &Path) -> Result<bool> {
    let path = varn_dir.join(GUARD_FILE);
    if path.is_file() {
        return Ok(false);
    }
    fs::write(&path, GUARD_CONTENT)?;
    Ok(true)
}

/// Whether a `.gitignore`-style file excludes `.varn/`.
///
/// Patterns are evaluated in order with last-match-wins semantics, so a
/// negation after a positive match correctly un-excludes the store.
fn file_excludes_varn(path: &Path) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        // An unreadable or non-UTF-8 file contributes no patterns. The
        // assessment is advisory and must not fail the calling command.
        Err(_) => return false,
    };

    let mut ignored = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (negated, pattern) = match line.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, line),
        };
        if ROOT_ENTRY_SPELLINGS.contains(&pattern) {
            ignored = !negated;
        }
    }
    ignored
}

/// Assess how a Varn store coexists with an enclosing git repository.
///
/// `root` is the managed directory (the parent of `varn_dir`).
pub fn assess(root: &Path, varn_dir: &Path) -> GitCoexistence {
    let git_root = find_git_root(root);

    // The store-level guard alone makes Git ignore the store contents.
    let guard_effective = guard_present(varn_dir) && {
        let content = fs::read_to_string(varn_dir.join(GUARD_FILE)).unwrap_or_default();
        content.lines().any(|l| l.trim() == "*")
    };

    let store_ignored = if guard_effective {
        true
    } else if let Some(ref git_root) = git_root {
        file_excludes_varn(&git_root.join(".gitignore"))
            || file_excludes_varn(&git_root.join(".git").join("info").join("exclude"))
    } else {
        false
    };

    GitCoexistence {
        git_root,
        store_ignored,
    }
}

/// The advisory warning shown when the store is not excluded from Git.
///
/// The suggested fix works in every state: fresh or legacy store, guard
/// present or missing.
pub fn unignored_warning() -> &'static str {
    ".varn/ is not excluded from git; a bare `git add -A` would stage the \
     object store. Fix: echo '.varn/' >> .gitignore"
}

/// Decision helper for commands: the warning to surface, if any.
pub fn coexistence_warning(root: &Path, varn_dir: &Path) -> Option<&'static str> {
    let coexist = assess(root, varn_dir);
    if coexist.git_root.is_some() && !coexist.store_ignored {
        Some(unignored_warning())
    } else {
        None
    }
}

/// Add `.varn/` to the root `.gitignore` of the repository at `git_root`.
///
/// Creates `.gitignore` if it is missing. Idempotent: recognized spellings of
/// the entry (`.varn`, `.varn/`, `/.varn`, `/.varn/`) are not duplicated.
pub fn append_to_gitignore(git_root: &Path) -> Result<GitignoreUpdate> {
    let path = git_root.join(".gitignore");

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::write(&path, format!("{GITIGNORE_ENTRY}\n"))?;
            return Ok(GitignoreUpdate::Created);
        }
        Err(e) => return Err(e.into()),
    };

    let already = content.lines().any(|line| {
        let line = line.trim();
        ROOT_ENTRY_SPELLINGS.contains(&line)
    });
    if already {
        return Ok(GitignoreUpdate::AlreadyPresent);
    }

    let mut updated = content.clone();
    if !updated.ends_with('\n') && !updated.is_empty() {
        updated.push('\n');
    }
    updated.push_str(GITIGNORE_ENTRY);
    updated.push('\n');
    fs::write(&path, updated)?;
    Ok(GitignoreUpdate::Appended)
}

/// The entry appended to a root `.gitignore`.
const GITIGNORE_ENTRY: &str = ".varn/";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::VARN_DIR;
    use tempfile::TempDir;

    fn init_store(root: &Path) -> PathBuf {
        let varn_dir = root.join(VARN_DIR);
        fs::create_dir_all(&varn_dir).unwrap();
        varn_dir
    }

    #[test]
    fn find_git_root_finds_enclosing_repo() {
        let tmp = TempDir::new().unwrap();
        let git_root = tmp.path().join("repo");
        fs::create_dir_all(git_root.join(".git")).unwrap();
        let deep = git_root.join("a/b");
        fs::create_dir_all(&deep).unwrap();

        assert_eq!(find_git_root(&deep), Some(git_root));
    }

    #[test]
    fn find_git_root_handles_git_file() {
        let tmp = TempDir::new().unwrap();
        let git_root = tmp.path().join("worktree");
        fs::create_dir_all(&git_root).unwrap();
        fs::write(git_root.join(".git"), b"gitdir: /somewhere/else\n").unwrap();

        assert_eq!(find_git_root(&git_root), Some(git_root));
    }

    #[test]
    fn find_git_root_returns_none_outside_repo() {
        let tmp = TempDir::new().unwrap();
        // Skip when an enclosing repo exists above the temp dir (e.g. a git
        // repo at /tmp); find_git_root would then correctly return it.
        if tmp
            .path()
            .ancestors()
            .skip(1)
            .any(|p| p.join(".git").is_dir() || p.join(".git").is_file())
        {
            return;
        }
        assert_eq!(find_git_root(tmp.path()), None);
    }

    #[test]
    fn ensure_guard_creates_file() {
        let tmp = TempDir::new().unwrap();
        let varn_dir = init_store(tmp.path());

        assert!(ensure_guard(&varn_dir).unwrap());
        let content = fs::read_to_string(varn_dir.join(GUARD_FILE)).unwrap();
        assert_eq!(content, "*\n");
    }

    #[test]
    fn ensure_guard_is_idempotent_and_preserves_custom_content() {
        let tmp = TempDir::new().unwrap();
        let varn_dir = init_store(tmp.path());
        fs::write(varn_dir.join(GUARD_FILE), b"*\n!keep/\n").unwrap();

        assert!(!ensure_guard(&varn_dir).unwrap());
        let content = fs::read_to_string(varn_dir.join(GUARD_FILE)).unwrap();
        assert_eq!(content, "*\n!keep/\n");
    }

    #[test]
    fn assess_guard_makes_store_ignored() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let varn_dir = init_store(root);
        fs::create_dir_all(root.join(".git")).unwrap();
        ensure_guard(&varn_dir).unwrap();

        let coexist = assess(root, &varn_dir);
        assert!(coexist.git_root.is_some());
        assert!(coexist.store_ignored);
    }

    #[test]
    fn assess_root_gitignore_entry_is_recognized() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let varn_dir = init_store(root);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), b"target/\n.varn/\n").unwrap();

        assert!(assess(root, &varn_dir).store_ignored);
    }

    #[test]
    fn assess_recognizes_spelling_variants() {
        for entry in [".varn", "/.varn", "/.varn/"] {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            let varn_dir = init_store(root);
            fs::create_dir_all(root.join(".git")).unwrap();
            fs::write(root.join(".gitignore"), entry).unwrap();
            assert!(
                assess(root, &varn_dir).store_ignored,
                "spelling {entry} should be recognized"
            );
        }
    }

    #[test]
    fn assess_negation_after_positive_un_excludes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let varn_dir = init_store(root);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), b".varn/\n!.varn/\n").unwrap();

        assert!(!assess(root, &varn_dir).store_ignored);
    }

    #[test]
    fn assess_info_exclude_is_honored() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let varn_dir = init_store(root);
        fs::create_dir_all(root.join(".git/info")).unwrap();
        fs::write(root.join(".git/info/exclude"), b".varn/\n").unwrap();

        assert!(assess(root, &varn_dir).store_ignored);
    }

    #[test]
    fn assess_unignored_store_outside_git_has_no_warning() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let varn_dir = init_store(root);
        // Skip when an enclosing repo exists above the temp dir (e.g. a git
        // repo at /tmp); the warning would then correctly fire.
        if find_git_root(root).is_some() {
            return;
        }

        assert!(coexistence_warning(root, &varn_dir).is_none());
    }

    #[test]
    fn coexistence_warning_fires_for_unignored_store_in_git_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let varn_dir = init_store(root);
        fs::create_dir_all(root.join(".git")).unwrap();

        let warning = coexistence_warning(root, &varn_dir).expect("warning expected");
        assert!(warning.contains(".varn/"));
        assert!(warning.contains("gitignore"));
    }

    #[test]
    fn append_to_gitignore_creates_missing_file() {
        let tmp = TempDir::new().unwrap();
        let git_root = tmp.path();
        fs::create_dir_all(git_root.join(".git")).unwrap();

        assert_eq!(
            append_to_gitignore(git_root).unwrap(),
            GitignoreUpdate::Created
        );
        let content = fs::read_to_string(git_root.join(".gitignore")).unwrap();
        assert_eq!(content, ".varn/\n");
    }

    #[test]
    fn append_to_gitignore_appends_without_newline() {
        let tmp = TempDir::new().unwrap();
        let git_root = tmp.path();
        fs::write(git_root.join(".gitignore"), b"target/").unwrap();

        assert_eq!(
            append_to_gitignore(git_root).unwrap(),
            GitignoreUpdate::Appended
        );
        let content = fs::read_to_string(git_root.join(".gitignore")).unwrap();
        assert_eq!(content, "target/\n.varn/\n");
    }

    #[test]
    fn append_to_gitignore_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let git_root = tmp.path();
        fs::write(git_root.join(".gitignore"), b"target/\n.varn/\n").unwrap();

        assert_eq!(
            append_to_gitignore(git_root).unwrap(),
            GitignoreUpdate::AlreadyPresent
        );
        let content = fs::read_to_string(git_root.join(".gitignore")).unwrap();
        assert_eq!(content, "target/\n.varn/\n");
    }

    #[test]
    fn append_to_gitignore_preserves_comments_and_other_entries() {
        let tmp = TempDir::new().unwrap();
        let git_root = tmp.path();
        fs::write(git_root.join(".gitignore"), b"# build output\ntarget/\n").unwrap();

        append_to_gitignore(git_root).unwrap();
        let content = fs::read_to_string(git_root.join(".gitignore")).unwrap();
        assert_eq!(content, "# build output\ntarget/\n.varn/\n");
    }
}
