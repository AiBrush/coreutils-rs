use super::core::*;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

/// Helper: create a file with the given mode.
fn create_file_with_mode(dir: &TempDir, name: &str, mode: u32) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, "test content").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

/// Helper: get the mode of a file (lower 12 bits).
fn get_mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().mode() & 0o7777
}

/// Helper: default config with no verbose/changes output.
fn default_config() -> ChmodConfig {
    ChmodConfig::default()
}

// ──────────────────────────────────────────────────
// Octal mode tests
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_octal() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "octal.txt", 0o644);

    let new_mode = parse_mode("755", 0o644).unwrap();
    assert_eq!(new_mode, 0o755);

    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o755);
}

#[test]
fn test_chmod_octal_leading_zero() {
    let new_mode = parse_mode("0644", 0o000).unwrap();
    assert_eq!(new_mode, 0o644);
}

#[test]
fn test_chmod_octal_000() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "zero.txt", 0o755);

    let new_mode = parse_mode("000", 0o755).unwrap();
    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o000);
}

// ──────────────────────────────────────────────────
// Symbolic mode tests
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_symbolic_add() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "add.txt", 0o644);

    let new_mode = parse_mode("u+x", 0o644).unwrap();
    assert_eq!(new_mode, 0o744);

    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o744);
}

#[test]
fn test_chmod_symbolic_remove() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "remove.txt", 0o775);

    let new_mode = parse_mode("g-w", 0o775).unwrap();
    assert_eq!(new_mode, 0o755);

    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o755);
}

#[test]
fn test_chmod_symbolic_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "set.txt", 0o755);

    let new_mode = parse_mode("o=r", 0o755).unwrap();
    assert_eq!(new_mode, 0o754);

    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o754);
}

#[test]
fn test_chmod_symbolic_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "all.txt", 0o000);

    let new_mode = parse_mode("a+r", 0o000).unwrap();
    assert_eq!(new_mode, 0o444);

    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o444);
}

#[test]
fn test_chmod_combined() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "combined.txt", 0o000);

    let new_mode = parse_mode("u+rw,g+r,o-rwx", 0o000).unwrap();
    assert_eq!(new_mode, 0o640);

    chmod_file(&path, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&path), 0o640);
}

#[test]
fn test_chmod_symbolic_set_exact() {
    // Test u=rw,g=r,o= pattern
    let new_mode = parse_mode("u=rw,g=r,o=", 0o777).unwrap();
    assert_eq!(new_mode, 0o640);
}

#[test]
fn test_chmod_symbolic_add_multiple() {
    // u+rwx
    let new_mode = parse_mode("u+rwx", 0o000).unwrap();
    assert_eq!(new_mode, 0o700);
}

#[test]
fn test_chmod_symbolic_remove_multiple() {
    // go-rwx
    let new_mode = parse_mode("go-rwx", 0o777).unwrap();
    assert_eq!(new_mode, 0o700);
}

// ──────────────────────────────────────────────────
// Capital X tests
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_capital_x() {
    // X adds execute only if the file is a directory or already has execute
    // For a regular file with no execute bit, X should not add execute
    let new_mode = parse_mode("a+X", 0o644).unwrap();
    assert_eq!(new_mode, 0o644, "X should not add execute to non-exec regular file");

    // For a regular file that already has an execute bit, X should add execute
    let new_mode = parse_mode("a+X", 0o744).unwrap();
    assert_eq!(new_mode, 0o755, "X should add execute when file already has execute");

    // For a directory (mode includes S_IFDIR bit 0o40000)
    let dir_mode = 0o040644; // directory with mode 644
    let new_mode = parse_mode("a+X", dir_mode).unwrap();
    assert_eq!(new_mode, 0o755, "X should add execute for directory");
}

#[test]
fn test_chmod_capital_x_on_directory() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    fs::set_permissions(&subdir, fs::Permissions::from_mode(0o644)).unwrap();

    // Directories have execute bits for X
    let meta = fs::metadata(&subdir).unwrap();
    let current_mode = meta.mode();
    let new_mode = parse_mode("a+X", current_mode).unwrap();
    chmod_file(&subdir, new_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&subdir), 0o755, "X should add execute for directory");
}

// ──────────────────────────────────────────────────
// Sticky bit and setuid/setgid tests
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_sticky_bit() {
    let new_mode = parse_mode("o+t", 0o755).unwrap();
    assert_eq!(new_mode, 0o1755);
}

#[test]
fn test_chmod_setuid() {
    let new_mode = parse_mode("u+s", 0o755).unwrap();
    assert_eq!(new_mode, 0o4755);
}

#[test]
fn test_chmod_setgid() {
    let new_mode = parse_mode("g+s", 0o755).unwrap();
    assert_eq!(new_mode, 0o2755);
}

// ──────────────────────────────────────────────────
// Recursive tests
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_recursive() {
    let dir = tempfile::tempdir().unwrap();

    // Create a directory tree
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let file1 = dir.path().join("file1.txt");
    let file2 = sub.join("file2.txt");
    fs::write(&file1, "a").unwrap();
    fs::write(&file2, "b").unwrap();

    // Set initial modes
    fs::set_permissions(&file1, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&file2, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();

    let config = ChmodConfig {
        recursive: true,
        ..Default::default()
    };

    chmod_recursive(dir.path(), "a+x", &config).unwrap();

    // All files and directories should have execute bit added
    // Directories: 755 + x = 755 (already has x) or
    // Files: 644 + execute = 755
    assert_eq!(get_mode(&file1), 0o755, "file1 should have execute");
    assert_eq!(get_mode(&file2), 0o755, "file2 should have execute");
    // sub directory already had execute, it should still have it
    assert_eq!(get_mode(&sub), 0o755, "sub should have execute");
}

#[test]
fn test_chmod_recursive_octal() {
    let dir = tempfile::tempdir().unwrap();

    let sub = dir.path().join("rsub");
    fs::create_dir(&sub).unwrap();
    let file1 = dir.path().join("rfile.txt");
    fs::write(&file1, "data").unwrap();

    let config = ChmodConfig {
        recursive: true,
        ..Default::default()
    };

    chmod_recursive(dir.path(), "700", &config).unwrap();

    assert_eq!(get_mode(&file1), 0o700);
    assert_eq!(get_mode(&sub), 0o700);
}

// ──────────────────────────────────────────────────
// Reference mode test
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_reference() {
    let dir = tempfile::tempdir().unwrap();
    let ref_file = create_file_with_mode(&dir, "ref.txt", 0o751);
    let target = create_file_with_mode(&dir, "target.txt", 0o644);

    // Read reference file mode
    let ref_mode = fs::metadata(&ref_file).unwrap().mode() & 0o7777;
    assert_eq!(ref_mode, 0o751);

    // Apply reference mode to target
    chmod_file(&target, ref_mode, &default_config()).unwrap();
    assert_eq!(get_mode(&target), 0o751);
}

// ──────────────────────────────────────────────────
// Verbose output test
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_verbose() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "verbose.txt", 0o644);

    let config = ChmodConfig {
        verbose: true,
        ..Default::default()
    };

    // This should print to stderr but not error
    let changed = chmod_file(&path, 0o755, &config).unwrap();
    assert!(changed, "should report change");
}

#[test]
fn test_chmod_verbose_no_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "nochange.txt", 0o644);

    let config = ChmodConfig {
        verbose: true,
        ..Default::default()
    };

    let changed = chmod_file(&path, 0o644, &config).unwrap();
    assert!(!changed, "should report no change");
}

#[test]
fn test_chmod_changes_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file_with_mode(&dir, "changes.txt", 0o644);

    let config = ChmodConfig {
        changes: true,
        ..Default::default()
    };

    // This should print to stderr
    let changed = chmod_file(&path, 0o755, &config).unwrap();
    assert!(changed);

    // This should not print
    let changed = chmod_file(&path, 0o755, &config).unwrap();
    assert!(!changed);
}

// ──────────────────────────────────────────────────
// Preserve root test
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_preserve_root() {
    let config = ChmodConfig {
        recursive: true,
        preserve_root: true,
        ..Default::default()
    };

    let result = chmod_recursive(Path::new("/"), "755", &config);
    assert!(result.is_err(), "should fail on / with --preserve-root");
}

// ──────────────────────────────────────────────────
// Edge cases
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_invalid_mode() {
    let result = parse_mode("xyz", 0o644);
    assert!(result.is_err(), "should reject invalid mode");
}

#[test]
fn test_chmod_empty_set() {
    // o= with no permissions should clear other bits
    let new_mode = parse_mode("o=", 0o777).unwrap();
    assert_eq!(new_mode, 0o770);
}

#[test]
fn test_chmod_symbolic_u_copy() {
    // g=u means copy user bits to group
    let new_mode = parse_mode("g=u", 0o754).unwrap();
    assert_eq!(new_mode, 0o774, "group should get user's rwx bits");
}

// ──────────────────────────────────────────────────
// GNU compatibility
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_matches_gnu() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();

    // Test with GNU chmod if available
    let gnu_file = create_file_with_mode(&dir, "gnu.txt", 0o644);
    let our_file = create_file_with_mode(&dir, "our.txt", 0o644);

    let gnu = Command::new("chmod")
        .args(["755", gnu_file.to_str().unwrap()])
        .output();

    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_mode = get_mode(&gnu_file);

            let new_mode = parse_mode("755", 0o644).unwrap();
            chmod_file(&our_file, new_mode, &default_config()).unwrap();
            let our_mode = get_mode(&our_file);

            assert_eq!(our_mode, gnu_mode, "mode should match GNU chmod");
        }
    }
}

#[test]
fn test_chmod_matches_gnu_symbolic() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();

    let test_cases = [
        ("u+x", 0o644),
        ("g-w", 0o755),
        ("o=r", 0o755),
        ("a+r", 0o000),
        ("u=rw,g=r,o=", 0o777),
    ];

    for (mode_str, initial_mode) in test_cases {
        let gnu_file = create_file_with_mode(&dir, &format!("gnu_{}.txt", mode_str.replace(',', "_")), initial_mode);
        let our_file = create_file_with_mode(&dir, &format!("our_{}.txt", mode_str.replace(',', "_")), initial_mode);

        let gnu = Command::new("chmod")
            .args([mode_str, gnu_file.to_str().unwrap()])
            .output();

        if let Ok(gnu_out) = gnu {
            if gnu_out.status.success() {
                let gnu_mode = get_mode(&gnu_file);

                let new_mode = parse_mode(mode_str, initial_mode).unwrap();
                chmod_file(&our_file, new_mode, &default_config()).unwrap();
                let our_mode = get_mode(&our_file);

                assert_eq!(
                    our_mode, gnu_mode,
                    "mode mismatch for '{}' on {:o}: ours={:o}, gnu={:o}",
                    mode_str, initial_mode, our_mode, gnu_mode
                );
            }
        }
    }
}

// ──────────────────────────────────────────────────
// Symlink handling
// ──────────────────────────────────────────────────

#[test]
fn test_chmod_skips_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let target = create_file_with_mode(&dir, "real.txt", 0o644);
    let link = dir.path().join("link.txt");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();

    // chmod on symlink should skip it (not follow)
    let changed = chmod_file(&link, 0o755, &default_config()).unwrap();
    assert!(!changed, "should skip symlink");

    // Original file should be unchanged
    assert_eq!(get_mode(&target), 0o644, "original should be unchanged");
}
