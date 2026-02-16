use super::core::*;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Helper: build the path to the `frm` binary built by cargo.
fn frm_cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove `deps`
    path.push("frm");
    Command::new(path)
}

// ── Unit tests (library API) ────────────────────────────────────────────────

#[test]
fn test_rm_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.txt");
    fs::write(&file, "hello").unwrap();
    assert!(file.exists());

    let config = RmConfig::default();
    let ok = rm_path(&file, &config).unwrap();
    assert!(ok);
    assert!(!file.exists());
}

#[test]
fn test_rm_multiple() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let c = dir.path().join("c.txt");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    fs::write(&c, "c").unwrap();

    let config = RmConfig::default();
    for f in &[&a, &b, &c] {
        let ok = rm_path(f.as_path(), &config).unwrap();
        assert!(ok);
    }
    assert!(!a.exists());
    assert!(!b.exists());
    assert!(!c.exists());
}

#[test]
fn test_rm_force_missing() {
    let config = RmConfig {
        force: true,
        ..RmConfig::default()
    };
    let missing = Path::new("/tmp/nonexistent_rm_test_file_999888777");
    let ok = rm_path(missing, &config).unwrap();
    // -f with a nonexistent file is not an error.
    assert!(ok);
}

#[test]
fn test_rm_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    let deep = sub.join("deep");
    fs::create_dir_all(&deep).unwrap();
    fs::write(sub.join("file1.txt"), "1").unwrap();
    fs::write(deep.join("file2.txt"), "2").unwrap();

    let config = RmConfig {
        recursive: true,
        ..RmConfig::default()
    };
    let ok = rm_path(&sub, &config).unwrap();
    assert!(ok);
    assert!(!sub.exists());
}

#[test]
fn test_rm_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty_dir");
    fs::create_dir(&empty).unwrap();

    let config = RmConfig {
        dir: true,
        ..RmConfig::default()
    };
    let ok = rm_path(&empty, &config).unwrap();
    assert!(ok);
    assert!(!empty.exists());
}

#[test]
fn test_rm_preserve_root() {
    let config = RmConfig {
        recursive: true,
        preserve_root: PreserveRoot::Yes,
        ..RmConfig::default()
    };
    let ok = rm_path(Path::new("/"), &config).unwrap();
    // Should refuse to remove '/'.
    assert!(!ok);
}

#[test]
fn test_rm_verbose() {
    // We test verbose through the binary so we can capture stderr.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("verbose_file.txt");
    fs::write(&file, "data").unwrap();

    let output = frm_cmd()
        .args(["-v", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("removed"),
        "verbose output should contain 'removed', got: {}",
        stderr
    );
    assert!(!file.exists());
}

#[test]
fn test_rm_nonexistent_error() {
    let config = RmConfig::default();
    let missing = Path::new("/tmp/nonexistent_rm_test_file_123456");
    let ok = rm_path(missing, &config).unwrap();
    // Without -f, a nonexistent file should report failure.
    assert!(!ok);
}

#[test]
fn test_rm_dir_without_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("norecurse");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("file.txt"), "x").unwrap();

    let config = RmConfig::default();
    let ok = rm_path(&sub, &config).unwrap();
    // Should fail: it's a directory and neither -r nor -d was given.
    assert!(!ok);
    assert!(sub.exists());
}

#[test]
fn test_rm_matches_gnu() {
    // Compare exit code with GNU rm for a nonexistent file.
    let bogus = "/tmp/nonexistent_rm_match_gnu_test_42";

    let gnu = Command::new("rm").arg(bogus).output();
    if let Ok(gnu_out) = gnu {
        let ours = frm_cmd().arg(bogus).output().unwrap();
        assert_eq!(
            ours.status.code(),
            gnu_out.status.code(),
            "Exit code mismatch with GNU rm"
        );
    }
}
