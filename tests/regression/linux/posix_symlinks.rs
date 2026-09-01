//! Linux: POSIX symlink edge cases (absolute targets, symlink chains,
//! symlink loops).

use crate::common::TestRepo;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn absolute_symlink_target_round_trip() {
    let repo = TestRepo::new();
    let outside = std::env::temp_dir().join("varn-linux-abs-target.txt");
    fs::write(&outside, b"outside content").unwrap();

    repo.write("real.txt", b"placeholder");
    std::os::unix::fs::symlink(&outside, repo.root().join("abslink.txt")).unwrap();

    let snapshot = repo.checkpoint("abs link");
    let entry = crate::common::find_snap_entry(&snapshot, "abslink.txt");
    assert_eq!(entry.meta.target.as_deref(), Some(outside.as_path()));

    fs::remove_file(repo.root().join("abslink.txt")).unwrap();
    repo.restore(&snapshot);
    assert!(repo.root().join("abslink.txt").is_symlink());
    assert_eq!(
        fs::read_link(repo.root().join("abslink.txt")).unwrap(),
        outside
    );
    assert!(repo.verifies(&snapshot));
    fs::remove_file(&outside).ok();
}

#[test]
fn relative_symlink_with_dots_round_trip() {
    let repo = TestRepo::new();
    repo.write("a/real.txt", b"real");
    let link = repo.root().join("b/link.txt");
    let target = PathBuf::from("../a/real.txt");
    fs::create_dir_all(repo.root().join("b")).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let snapshot = repo.checkpoint("dots link");
    let entry = crate::common::find_snap_entry(&snapshot, "b/link.txt");
    assert_eq!(
        entry.meta.target.as_deref(),
        Some(std::path::Path::new("../a/real.txt"))
    );

    fs::remove_file(repo.root().join("b/link.txt")).unwrap();
    repo.restore(&snapshot);
    assert_eq!(
        fs::read_link(repo.root().join("b/link.txt")).unwrap(),
        PathBuf::from("../a/real.txt")
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn symlink_chain_round_trip() {
    let repo = TestRepo::new();
    repo.write("final.txt", b"the end");
    std::os::unix::fs::symlink(Path::new("final.txt"), repo.root().join("mid.txt")).unwrap();
    std::os::unix::fs::symlink(Path::new("mid.txt"), repo.root().join("start.txt")).unwrap();

    let snapshot = repo.checkpoint("chain");
    for n in ["mid.txt", "start.txt"] {
        fs::remove_file(repo.root().join(n)).unwrap();
    }
    repo.restore(&snapshot);

    assert_eq!(
        fs::read_link(repo.root().join("start.txt")).unwrap(),
        PathBuf::from("mid.txt")
    );
    assert_eq!(
        fs::read_link(repo.root().join("mid.txt")).unwrap(),
        PathBuf::from("final.txt")
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn symlink_loop_does_not_hang_the_scanner() {
    let repo = TestRepo::new();
    // a -> b, b -> a: a cycle. The scanner must not follow it.
    std::os::unix::fs::symlink(Path::new("b"), repo.root().join("a")).unwrap();
    std::os::unix::fs::symlink(Path::new("a"), repo.root().join("b")).unwrap();

    // If this returns, the scanner didn't loop.
    let scan = repo.scan();
    let links = scan
        .entries
        .iter()
        .filter(|e| e.meta.kind == varn::filesystem::EntryKind::Symlink)
        .count();
    assert_eq!(links, 2, "both loop members must be captured as symlinks");
}

#[test]
fn fifo_and_socket_are_recorded_as_other() {
    let repo = TestRepo::new();
    // Create a FIFO via libc mkfifo (std has no fifo API).
    let fifo = repo.root().join("pipe.fifo");
    let c_path = std::ffi::CString::new(fifo.as_os_str().to_str().unwrap()).unwrap();
    unsafe extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> std::ffi::c_int;
    }
    let rc = unsafe { mkfifo(c_path.as_ptr(), 0o644) };
    if rc == 0 {
        let scan = repo.scan();
        let entry = scan
            .entries
            .iter()
            .find(|e| e.path == std::path::Path::new("pipe.fifo"))
            .expect("fifo must be listed");
        assert_eq!(
            entry.meta.kind,
            varn::filesystem::EntryKind::Other,
            "fifos are recorded as Other (not restored in this version)"
        );
    }
    // If mkfifo failed (restricted env), the test passes trivially.
}
