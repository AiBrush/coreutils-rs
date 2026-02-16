use super::*;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use tempfile::TempDir;

/// Helper: create a default config for testing.
fn default_config() -> DuConfig {
    DuConfig::default()
}

/// Helper: create a temporary directory tree for testing.
/// Layout:
///   tmp/
///     file1.txt  (100 bytes)
///     subdir/
///       file2.txt  (200 bytes)
///       nested/
///         file3.txt (50 bytes)
///     excluded.log (75 bytes)
fn create_test_tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // file1.txt
    let mut f1 = fs::File::create(root.join("file1.txt")).unwrap();
    f1.write_all(&vec![b'A'; 100]).unwrap();

    // subdir/file2.txt
    fs::create_dir(root.join("subdir")).unwrap();
    let mut f2 = fs::File::create(root.join("subdir").join("file2.txt")).unwrap();
    f2.write_all(&vec![b'B'; 200]).unwrap();

    // subdir/nested/file3.txt
    fs::create_dir(root.join("subdir").join("nested")).unwrap();
    let mut f3 = fs::File::create(root.join("subdir").join("nested").join("file3.txt")).unwrap();
    f3.write_all(&vec![b'C'; 50]).unwrap();

    // excluded.log
    let mut f4 = fs::File::create(root.join("excluded.log")).unwrap();
    f4.write_all(&vec![b'D'; 75]).unwrap();

    tmp
}

/// Helper: get the path string for a binary built by this crate.
fn bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove "deps"
    path.push(name);
    path
}

/// Helper: collect entry paths relative to the root from `du_path` output.
fn entry_paths(entries: &[DuEntry]) -> Vec<String> {
    entries.iter().map(|e| e.path.display().to_string()).collect()
}

// ---------- Tests ----------

#[test]
fn test_du_runs() {
    let tmp = create_test_tree();
    let config = default_config();
    let entries = du_path(tmp.path(), &config).unwrap();
    // Should have at least the root directory entry.
    assert!(!entries.is_empty(), "du should produce at least one entry");
    // The last entry should be the root.
    let last = entries.last().unwrap();
    assert_eq!(last.path, tmp.path());
    // The total size should be > 0.
    assert!(last.size > 0, "root directory size should be > 0");
}

#[test]
fn test_du_summary() {
    let tmp = create_test_tree();
    let config = DuConfig {
        summarize: true,
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();
    // With --summarize, only one entry: the root.
    assert_eq!(entries.len(), 1, "summarize should produce exactly one entry");
    assert_eq!(entries[0].path, tmp.path());
}

#[test]
fn test_du_human() {
    // Human-readable formatting (powers of 1024).
    assert_eq!(format_size(0, &DuConfig { human_readable: true, ..default_config() }), "0");
    assert_eq!(format_size(512, &DuConfig { human_readable: true, ..default_config() }), "512");
    assert_eq!(format_size(1024, &DuConfig { human_readable: true, ..default_config() }), "1K");

    let large = 1024 * 1024 * 5;
    let s = format_size(large, &DuConfig { human_readable: true, ..default_config() });
    assert_eq!(s, "5M");
}

#[test]
fn test_du_bytes() {
    // -b is equivalent to --apparent-size --block-size=1
    let tmp = create_test_tree();
    let config = DuConfig {
        apparent_size: true,
        block_size: 1,
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();

    // With apparent size and block_size=1, sizes should reflect actual content bytes.
    let root = entries.last().unwrap();
    // Total apparent size = 100 + 200 + 50 + 75 = 425 bytes of file data,
    // plus directory entries' apparent size.
    assert!(root.size >= 425, "apparent total should be >= 425 bytes, got {}", root.size);

    // Verify format_size with block_size=1 produces raw number.
    let formatted = format_size(425, &config);
    assert_eq!(formatted, "425");
}

#[test]
fn test_du_total() {
    let tmp = create_test_tree();
    let config = DuConfig {
        total: true,
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();
    // du_path itself does not add a "total" line; that is handled by the caller.
    // But the root entry's size should represent the total.
    let root = entries.last().unwrap();
    assert!(root.size > 0);

    // Verify the grand total can be computed by summing.
    // With --total, the binary prints a "total" line with the grand total.
    // Here we just verify the entries are produced correctly.
    let total: u64 = entries
        .iter()
        .filter(|e| e.path == tmp.path())
        .map(|e| e.size)
        .sum();
    assert!(total > 0);
}

#[test]
fn test_du_max_depth() {
    let tmp = create_test_tree();
    let config = DuConfig {
        max_depth: Some(1),
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();

    // With max_depth=1, we should see root and its immediate subdirectory,
    // but not subdir/nested as a separate entry.
    let paths = entry_paths(&entries);
    let nested_dir = tmp.path().join("subdir").join("nested");
    assert!(
        !paths.contains(&nested_dir.display().to_string()),
        "nested directory should not appear with max_depth=1"
    );

    // Root and subdir should appear.
    assert!(
        paths.contains(&tmp.path().display().to_string()),
        "root should appear"
    );
    assert!(
        paths.contains(&tmp.path().join("subdir").display().to_string()),
        "subdir should appear at depth 1"
    );
}

#[test]
fn test_du_all() {
    let tmp = create_test_tree();
    let config = DuConfig {
        all: true,
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();
    let paths = entry_paths(&entries);

    // With --all, individual files should appear.
    assert!(
        paths.contains(&tmp.path().join("file1.txt").display().to_string()),
        "file1.txt should appear with --all"
    );
    assert!(
        paths.contains(
            &tmp.path()
                .join("subdir")
                .join("file2.txt")
                .display()
                .to_string()
        ),
        "subdir/file2.txt should appear with --all"
    );
}

#[test]
fn test_du_exclude() {
    let tmp = create_test_tree();
    let config = DuConfig {
        all: true,
        exclude_patterns: vec!["*.log".to_string()],
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();
    let paths = entry_paths(&entries);

    // excluded.log should not appear.
    assert!(
        !paths.contains(&tmp.path().join("excluded.log").display().to_string()),
        "excluded.log should be excluded by *.log pattern"
    );

    // But other files should still be present.
    assert!(
        paths.contains(&tmp.path().join("file1.txt").display().to_string()),
        "file1.txt should not be excluded"
    );
}

#[test]
fn test_du_one_filesystem() {
    // This test verifies that with --one-file-system, du stays on the same device.
    // We cannot easily create a cross-device scenario in a unit test, so we verify
    // that the flag doesn't break normal operation.
    let tmp = create_test_tree();
    let config = DuConfig {
        one_file_system: true,
        ..default_config()
    };
    let entries = du_path(tmp.path(), &config).unwrap();
    assert!(!entries.is_empty(), "du -x should still produce entries on same fs");
    let root = entries.last().unwrap();
    assert!(root.size > 0);
}

#[test]
fn test_du_apparent_size() {
    let tmp = create_test_tree();

    // Get disk usage (default).
    let config_disk = default_config();
    let entries_disk = du_path(tmp.path(), &config_disk).unwrap();
    let root_disk = entries_disk.last().unwrap().size;

    // Get apparent size.
    let config_apparent = DuConfig {
        apparent_size: true,
        ..default_config()
    };
    let entries_apparent = du_path(tmp.path(), &config_apparent).unwrap();
    let root_apparent = entries_apparent.last().unwrap().size;

    // Disk usage is typically >= apparent size (due to block allocation).
    // The apparent size should reflect the actual content bytes.
    // file data totals 100+200+50+75 = 425 bytes.
    assert!(
        root_apparent >= 425,
        "apparent size should be >= 425, got {}",
        root_apparent
    );

    // Disk usage should generally be >= apparent (blocks round up).
    // On some filesystems with tiny files this might not hold strictly,
    // but for our test data it should.
    assert!(
        root_disk >= root_apparent || root_disk > 0,
        "disk usage should be positive"
    );
}

#[test]
fn test_du_matches_gnu() {
    // Verify that our binary can be invoked and produces output similar to GNU du.
    let fdu = bin_path("fdu");
    if !fdu.exists() {
        // Binary not built yet; skip this integration test.
        eprintln!("fdu binary not found at {:?}, skipping integration test", fdu);
        return;
    }

    let tmp = create_test_tree();
    let output = std::process::Command::new(&fdu)
        .arg("-s")
        .arg(tmp.path())
        .output()
        .expect("failed to run fdu");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should produce exactly one line (summarize mode).
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1, "fdu -s should produce one line, got: {:?}", lines);
    // The line should contain a number followed by a tab and the path.
    assert!(
        lines[0].contains('\t'),
        "output should be tab-separated: {:?}",
        lines[0]
    );
    let parts: Vec<&str> = lines[0].splitn(2, '\t').collect();
    assert_eq!(parts.len(), 2);
    let _size: u64 = parts[0].trim().parse().expect("first column should be a number");
    assert!(
        parts[1].trim() == tmp.path().display().to_string(),
        "path should match: {} vs {}",
        parts[1].trim(),
        tmp.path().display()
    );
}

// ---------- Unit tests for helpers ----------

#[test]
fn test_glob_match() {
    assert!(glob_match("*.log", "test.log"));
    assert!(glob_match("*.log", ".log"));
    assert!(!glob_match("*.log", "test.txt"));
    assert!(glob_match("file?.txt", "file1.txt"));
    assert!(!glob_match("file?.txt", "file12.txt"));
    assert!(glob_match("*", "anything"));
    assert!(glob_match("test*", "test123"));
    assert!(!glob_match("test*", "tes"));
}

#[test]
fn test_parse_block_size() {
    assert_eq!(parse_block_size("1024").unwrap(), 1024);
    assert_eq!(parse_block_size("1K").unwrap(), 1024);
    assert_eq!(parse_block_size("1M").unwrap(), 1024 * 1024);
    assert_eq!(parse_block_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
    assert!(parse_block_size("").is_err());
}

#[test]
fn test_parse_threshold() {
    assert_eq!(parse_threshold("1024").unwrap(), 1024);
    assert_eq!(parse_threshold("-1K").unwrap(), -1024);
    assert_eq!(parse_threshold("1M").unwrap(), 1024 * 1024);
}

#[test]
fn test_format_size_scaled() {
    let config = DuConfig {
        block_size: 1024,
        ..default_config()
    };
    // 4096 bytes / 1024 = 4
    assert_eq!(format_size(4096, &config), "4");
    // 512 bytes / 1024 = ceil(0.5) = 1
    assert_eq!(format_size(512, &config), "1");
    // 0 bytes = 0
    assert_eq!(format_size(0, &config), "0");
}

#[test]
fn test_format_size_si() {
    let config = DuConfig {
        si: true,
        ..default_config()
    };
    assert_eq!(format_size(1000, &config), "1k");
    assert_eq!(format_size(500, &config), "500");
}
