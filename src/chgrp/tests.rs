#[cfg(unix)]
use std::process::Command;

#[cfg(unix)]
fn cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("fchgrp");
    Command::new(path)
}

#[cfg(unix)]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

// ---- Basic chgrp_file tests ----

#[test]
#[cfg(unix)]
fn test_chgrp_basic_noop() {
    // Changing to the current group should be a no-op (no root needed)
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    let current_gid = meta.gid();

    let config = crate::chgrp::ChgrpConfig::default();
    let changed = crate::chgrp::chgrp_file(tmp.path(), current_gid, &config).unwrap();
    assert!(!changed, "Setting same group should be a no-op");
}

#[test]
#[cfg(unix)]
fn test_chgrp_basic_actual_change() {
    if !is_root() {
        return;
    }
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("testfile");
    std::fs::write(&file_path, "hello").unwrap();

    // Find a target group different from current
    let meta = std::fs::metadata(&file_path).unwrap();
    let current_gid = meta.gid();
    let target_gid = if current_gid == 0 { 1 } else { 0 };

    let config = crate::chgrp::ChgrpConfig::default();
    let changed = crate::chgrp::chgrp_file(&file_path, target_gid, &config).unwrap();
    assert!(changed, "Group should have been changed");

    let new_meta = std::fs::metadata(&file_path).unwrap();
    assert_eq!(new_meta.gid(), target_gid);
}

#[test]
#[cfg(unix)]
fn test_chgrp_reference() {
    // get_reference_ids should work for chgrp too
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let (uid, gid) = crate::chown::get_reference_ids(tmp.path()).unwrap();
    let euid = unsafe { libc::geteuid() };
    let egid = unsafe { libc::getegid() };
    assert_eq!(uid, euid);
    assert_eq!(gid, egid);
}

#[test]
#[cfg(unix)]
fn test_chgrp_from_filter_skip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();

    let config = crate::chgrp::ChgrpConfig {
        from_group: Some(99999), // unlikely to match
        ..Default::default()
    };
    let changed = crate::chgrp::chgrp_file(tmp.path(), 0, &config).unwrap();
    assert!(!changed, "--from filter should cause skip");
}

#[test]
#[cfg(unix)]
fn test_chgrp_recursive() {
    if !is_root() {
        return;
    }
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = sub.join("b.txt");
    std::fs::write(&file1, "a").unwrap();
    std::fs::write(&file2, "b").unwrap();

    let meta = std::fs::metadata(&file1).unwrap();
    let current_gid = meta.gid();
    let target_gid = if current_gid == 0 { 1 } else { 0 };

    let config = crate::chgrp::ChgrpConfig {
        recursive: true,
        ..Default::default()
    };

    let errors =
        crate::chgrp::chgrp_recursive(dir.path(), target_gid, &config, true, "chgrp");
    assert_eq!(errors, 0);

    let m1 = std::fs::metadata(&file1).unwrap();
    let m2 = std::fs::metadata(&file2).unwrap();
    assert_eq!(m1.gid(), target_gid);
    assert_eq!(m2.gid(), target_gid);
}

// ---- CLI error tests (no root needed) ----

#[test]
#[cfg(unix)]
fn test_chgrp_matches_gnu_errors_missing_operand() {
    let output = cmd().output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing operand"));

    let gnu = Command::new("chgrp").output();
    if let Ok(gnu) = gnu {
        assert_ne!(gnu.status.code(), Some(0));
    }
}

#[test]
#[cfg(unix)]
fn test_chgrp_matches_gnu_errors_missing_file() {
    let output = cmd().arg("root").output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing operand"),
        "stderr was: {}",
        stderr
    );
}

#[test]
#[cfg(unix)]
fn test_chgrp_matches_gnu_errors_invalid_group() {
    let output = cmd()
        .args(["nonexistent_group_xyz_99999", "/tmp/nofile"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid group"), "stderr was: {}", stderr);
}

#[test]
#[cfg(unix)]
fn test_chgrp_help() {
    let output = cmd().arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--recursive"));
}

#[test]
#[cfg(unix)]
fn test_chgrp_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("(fcoreutils)"));
}

#[test]
#[cfg(unix)]
fn test_chgrp_preserve_root() {
    let output = cmd()
        .args(["--preserve-root", "-R", "root", "/"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dangerous to operate recursively on '/'"),
        "stderr was: {}",
        stderr
    );
}

#[test]
#[cfg(unix)]
fn test_chgrp_nonexistent_file() {
    let output = cmd()
        .args(["root", "/nonexistent_file_xyz_99999"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
}
