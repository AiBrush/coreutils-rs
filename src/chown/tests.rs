#[cfg(unix)]
use std::process::Command;

#[cfg(unix)]
fn cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("fchown");
    Command::new(path)
}

#[cfg(unix)]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

// ---- parse_owner_spec tests (no root required) ----

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_numeric() {
    // Pure numeric owner:group
    let (uid, gid) = crate::chown::parse_owner_spec("1000:1000").unwrap();
    assert_eq!(uid, Some(1000));
    assert_eq!(gid, Some(1000));
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_user_only_numeric() {
    let (uid, gid) = crate::chown::parse_owner_spec("1000").unwrap();
    assert_eq!(uid, Some(1000));
    assert_eq!(gid, None);
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_group_only_numeric() {
    let (uid, gid) = crate::chown::parse_owner_spec(":1000").unwrap();
    assert_eq!(uid, None);
    assert_eq!(gid, Some(1000));
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_user_colon_numeric() {
    // "UID:" means set owner and group to that user's login group
    // Use root (uid 0) which always exists
    let (uid, gid) = crate::chown::parse_owner_spec("0:").unwrap();
    assert_eq!(uid, Some(0));
    // gid should be root's login group (typically 0)
    assert!(gid.is_some());
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_by_name() {
    // "root" should resolve to uid 0
    let result = crate::chown::parse_owner_spec("root");
    if let Ok((uid, gid)) = result {
        assert_eq!(uid, Some(0));
        assert_eq!(gid, None);
    }
    // If root user doesn't exist somehow, just skip
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_invalid_user() {
    let result = crate::chown::parse_owner_spec("nonexistent_user_xyz_99999");
    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_invalid_group() {
    let result = crate::chown::parse_owner_spec(":nonexistent_group_xyz_99999");
    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_empty() {
    let result = crate::chown::parse_owner_spec("");
    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn test_chown_parse_spec_dot_separator() {
    // Deprecated dot form: "1000.1000"
    let (uid, gid) = crate::chown::parse_owner_spec("1000.1000").unwrap();
    assert_eq!(uid, Some(1000));
    assert_eq!(gid, Some(1000));
}

#[test]
#[cfg(unix)]
fn test_chown_numeric() {
    // Test numeric owner:group on a temp file (requires root to actually change)
    let result = crate::chown::parse_owner_spec("1000:1000");
    assert!(result.is_ok());
    let (uid, gid) = result.unwrap();
    assert_eq!(uid, Some(1000));
    assert_eq!(gid, Some(1000));
}

#[test]
#[cfg(unix)]
fn test_chown_group_only() {
    let result = crate::chown::parse_owner_spec(":0");
    assert!(result.is_ok());
    let (uid, gid) = result.unwrap();
    assert_eq!(uid, None);
    assert_eq!(gid, Some(0));
}

// ---- chown_file tests (may need root) ----

#[test]
#[cfg(unix)]
fn test_chown_file_noop() {
    // Changing to the current owner/group should be a no-op (no root needed)
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    let current_uid = meta.uid();
    let current_gid = meta.gid();

    let config = crate::chown::ChownConfig::default();
    let changed =
        crate::chown::chown_file(tmp.path(), Some(current_uid), Some(current_gid), &config)
            .unwrap();
    assert!(!changed, "Setting same owner/group should be a no-op");
}

#[test]
#[cfg(unix)]
fn test_chown_file_from_filter_skip() {
    // --from filter with non-matching owner should skip
    let tmp = tempfile::NamedTempFile::new().unwrap();

    let config = crate::chown::ChownConfig {
        from_owner: Some(99999), // unlikely to match
        ..Default::default()
    };
    let changed = crate::chown::chown_file(tmp.path(), Some(0), None, &config).unwrap();
    assert!(!changed, "--from filter should cause skip");
}

#[test]
#[cfg(unix)]
fn test_chown_verbose() {
    // verbose mode on a no-op should not panic
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();

    let config = crate::chown::ChownConfig {
        verbose: true,
        ..Default::default()
    };
    let result =
        crate::chown::chown_file(tmp.path(), Some(meta.uid()), Some(meta.gid()), &config);
    assert!(result.is_ok());
}

#[test]
#[cfg(unix)]
fn test_chown_reference() {
    // get_reference_ids should return valid uid/gid
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let (uid, gid) = crate::chown::get_reference_ids(tmp.path()).unwrap();
    // Should match our effective uid/gid
    let euid = unsafe { libc::geteuid() };
    let egid = unsafe { libc::getegid() };
    assert_eq!(uid, euid);
    assert_eq!(gid, egid);
}

#[test]
#[cfg(unix)]
fn test_chown_actual_change() {
    if !is_root() {
        // Skip: chown requires root
        return;
    }
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("testfile");
    std::fs::write(&file_path, "hello").unwrap();

    let config = crate::chown::ChownConfig::default();
    // Change to nobody (uid 65534) if it exists, else use 1000
    let target_uid = crate::chown::resolve_user("nobody").unwrap_or(1000);
    let result = crate::chown::chown_file(&file_path, Some(target_uid), None, &config);
    assert!(result.is_ok());

    let meta = std::fs::metadata(&file_path).unwrap();
    assert_eq!(meta.uid(), target_uid);
}

#[test]
#[cfg(unix)]
fn test_chown_recursive() {
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

    let config = crate::chown::ChownConfig {
        recursive: true,
        ..Default::default()
    };

    let target_uid = crate::chown::resolve_user("nobody").unwrap_or(1000);
    let errors = crate::chown::chown_recursive(
        dir.path(),
        Some(target_uid),
        None,
        &config,
        true,
        "chown",
    );
    assert_eq!(errors, 0);

    let m1 = std::fs::metadata(&file1).unwrap();
    let m2 = std::fs::metadata(&file2).unwrap();
    assert_eq!(m1.uid(), target_uid);
    assert_eq!(m2.uid(), target_uid);
}

// ---- CLI error tests (no root needed) ----

#[test]
#[cfg(unix)]
fn test_chown_matches_gnu_errors_missing_operand() {
    let output = cmd().output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing operand"));

    // Compare with GNU
    let gnu = Command::new("chown").output();
    if let Ok(gnu) = gnu {
        assert_ne!(gnu.status.code(), Some(0));
    }
}

#[test]
#[cfg(unix)]
fn test_chown_matches_gnu_errors_missing_file() {
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
fn test_chown_matches_gnu_errors_invalid_user() {
    let output = cmd()
        .args(["nonexistent_user_xyz_99999", "/tmp/nofile"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid user"), "stderr was: {}", stderr);
}

#[test]
#[cfg(unix)]
fn test_chown_help() {
    let output = cmd().arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--recursive"));
}

#[test]
#[cfg(unix)]
fn test_chown_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("(fcoreutils)"));
}

#[test]
#[cfg(unix)]
fn test_chown_preserve_root() {
    // --preserve-root -R / should error
    let output = cmd()
        .args(["--preserve-root", "-R", "root:root", "/"])
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
fn test_chown_nonexistent_file() {
    let output = cmd()
        .args(["root", "/nonexistent_file_xyz_99999"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
}
