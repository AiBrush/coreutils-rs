use super::*;
use std::collections::HashSet;

// ──────────────────────────────────────────────────
// Unit tests for formatting helpers
// ──────────────────────────────────────────────────

#[test]
fn test_human_readable_1024_zero() {
    assert_eq!(human_readable_1024(0), "0");
}

#[test]
fn test_human_readable_1024_bytes() {
    assert_eq!(human_readable_1024(512), "512");
}

#[test]
fn test_human_readable_1024_kilo() {
    // 10240 bytes = 10.0K -> "10K"
    assert_eq!(human_readable_1024(10240), "10K");
}

#[test]
fn test_human_readable_1024_mega() {
    // 5 * 1024 * 1024 = 5242880
    assert_eq!(human_readable_1024(5 * 1024 * 1024), "5.0M");
}

#[test]
fn test_human_readable_1024_giga() {
    assert_eq!(human_readable_1024(2u64 * 1024 * 1024 * 1024), "2.0G");
}

#[test]
fn test_human_readable_1000_zero() {
    assert_eq!(human_readable_1000(0), "0");
}

#[test]
fn test_human_readable_1000_bytes() {
    assert_eq!(human_readable_1000(999), "999");
}

#[test]
fn test_human_readable_1000_kilo() {
    assert_eq!(human_readable_1000(10000), "10k");
}

#[test]
fn test_human_readable_1000_mega() {
    assert_eq!(human_readable_1000(5_000_000), "5.0M");
}

#[test]
fn test_format_size_human() {
    let config = DfConfig {
        human_readable: true,
        ..DfConfig::default()
    };
    let result = format_size(1024 * 1024, &config);
    assert_eq!(result, "1.0M");
}

#[test]
fn test_format_size_si() {
    let config = DfConfig {
        si: true,
        ..DfConfig::default()
    };
    let result = format_size(1_000_000, &config);
    assert_eq!(result, "1.0M");
}

#[test]
fn test_format_size_block_1k() {
    let config = DfConfig {
        block_size: 1024,
        ..DfConfig::default()
    };
    let result = format_size(2048, &config);
    assert_eq!(result, "2");
}

#[test]
fn test_parse_block_size_numeric() {
    assert_eq!(parse_block_size("512").unwrap(), 512);
    assert_eq!(parse_block_size("1024").unwrap(), 1024);
}

#[test]
fn test_parse_block_size_suffix() {
    assert_eq!(parse_block_size("1K").unwrap(), 1024);
    assert_eq!(parse_block_size("1M").unwrap(), 1024 * 1024);
    assert_eq!(parse_block_size("1G").unwrap(), 1024 * 1024 * 1024);
}

#[test]
fn test_parse_block_size_bare_suffix() {
    assert_eq!(parse_block_size("K").unwrap(), 1024);
    assert_eq!(parse_block_size("M").unwrap(), 1024 * 1024);
}

#[test]
fn test_parse_block_size_invalid() {
    assert!(parse_block_size("").is_err());
    assert!(parse_block_size("abc").is_err());
}

#[test]
fn test_parse_output_fields_valid() {
    let fields = parse_output_fields("source,size,used,avail,pcent,target").unwrap();
    assert_eq!(fields.len(), 6);
    assert_eq!(fields[0], "source");
    assert_eq!(fields[5], "target");
}

#[test]
fn test_parse_output_fields_invalid() {
    let result = parse_output_fields("source,invalid_field");
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────
// Mount reading tests (Linux-specific)
// ──────────────────────────────────────────────────

#[cfg(target_os = "linux")]
#[test]
fn test_read_mounts_nonempty() {
    let mounts = read_mounts();
    assert!(!mounts.is_empty(), "Should find at least one mount");
}

#[cfg(target_os = "linux")]
#[test]
fn test_read_mounts_has_root() {
    let mounts = read_mounts();
    let has_root = mounts.iter().any(|m| m.target == "/");
    assert!(has_root, "Should have root filesystem mounted");
}

// ──────────────────────────────────────────────────
// Integration tests using the binary (Linux-specific)
// ──────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn cmd() -> std::process::Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("fdf");
    std::process::Command::new(path)
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_runs() {
    let output = cmd().output().unwrap();
    assert_eq!(output.status.code(), Some(0), "df should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Filesystem"),
        "Should have Filesystem header"
    );
    assert!(
        stdout.contains("Mounted on"),
        "Should have 'Mounted on' header"
    );
    // Should have at least one filesystem line beyond the header.
    assert!(stdout.lines().count() >= 2, "Should have at least header + 1 line");
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_human() {
    let output = cmd().arg("-h").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Size"),
        "Human-readable should use 'Size' header"
    );
    // Human-readable output should contain unit suffixes.
    let has_suffix = stdout.contains('K')
        || stdout.contains('M')
        || stdout.contains('G')
        || stdout.contains('T');
    assert!(has_suffix, "Human-readable output should contain size suffixes");
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_inodes() {
    let output = cmd().arg("-i").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Inodes"), "Should have Inodes header");
    assert!(stdout.contains("IUsed"), "Should have IUsed header");
    assert!(stdout.contains("IFree"), "Should have IFree header");
    assert!(stdout.contains("IUse%"), "Should have IUse% header");
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_type_filter() {
    // Filter for tmpfs which should exist on most Linux systems.
    let output = cmd().args(["-t", "tmpfs"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // All non-header lines should be tmpfs mounts. If tmpfs not present, just header.
    for line in stdout.lines().skip(1) {
        // When filtering by type, result set is restricted to that type.
        // We don't verify the type column here since -T isn't passed,
        // but the filter should not crash.
        assert!(!line.is_empty());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_exclude() {
    let output = cmd().args(["-x", "tmpfs"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // With -T we could verify no tmpfs line appears, but without -T we just
    // check it runs successfully.
    assert!(stdout.contains("Filesystem"));
    // Additionally verify with -T to ensure tmpfs is excluded.
    let output2 = cmd().args(["-x", "tmpfs", "-T"]).output().unwrap();
    assert_eq!(output2.status.code(), Some(0));
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    for line in stdout2.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            assert_ne!(parts[1], "tmpfs", "tmpfs should be excluded");
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_total() {
    let output = cmd().arg("--total").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout.lines().last().unwrap_or("");
    assert!(
        last_line.starts_with("total"),
        "Last line should start with 'total', got: '{}'",
        last_line
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_print_type() {
    let output = cmd().arg("-T").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Type"),
        "Should have 'Type' column header"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_specific_file() {
    let output = cmd().arg("/").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should show exactly header + 1 filesystem line.
    let line_count = stdout.lines().count();
    assert_eq!(
        line_count, 2,
        "df / should show header + 1 line, got {} lines",
        line_count
    );
    // The filesystem should have / as the mount point.
    let fs_line = stdout.lines().nth(1).unwrap();
    assert!(
        fs_line.contains('/'),
        "Filesystem line should contain '/' mount point"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_portability() {
    let output = cmd().arg("-P").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // POSIX format uses "Capacity" instead of "Use%".
    assert!(
        stdout.contains("Capacity"),
        "Portability mode should use 'Capacity' header"
    );
    assert!(
        stdout.contains("Available"),
        "Portability mode should use 'Available' header"
    );
    // Each filesystem entry should be on a single line (POSIX requirement).
    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert!(
            parts.len() >= 6,
            "POSIX format line should have at least 6 fields, got {}: '{}'",
            parts.len(),
            line
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn test_df_matches_gnu_format() {
    // Compare column count with GNU df.
    let gnu = std::process::Command::new("df").arg("/").output();
    if let Ok(gnu) = gnu {
        let ours = cmd().arg("/").output().unwrap();
        let gnu_stdout = String::from_utf8_lossy(&gnu.stdout);
        let our_stdout = String::from_utf8_lossy(&ours.stdout);

        // Both should have the same number of output lines.
        assert_eq!(
            gnu_stdout.lines().count(),
            our_stdout.lines().count(),
            "Line count should match GNU df"
        );

        // Headers should have the same column names.
        let gnu_header = gnu_stdout.lines().next().unwrap_or("");
        let our_header = our_stdout.lines().next().unwrap_or("");
        assert!(
            our_header.contains("Filesystem"),
            "Our header should contain 'Filesystem'"
        );
        assert!(
            gnu_header.contains("Filesystem"),
            "GNU header should contain 'Filesystem'"
        );

        // Both should show the same number of columns.
        let gnu_cols = gnu_header.split_whitespace().count();
        let our_cols = our_header.split_whitespace().count();
        // GNU uses "Mounted on" (2 words) and "1K-blocks" (1 word): 6 tokens total.
        // We should match this.
        assert_eq!(
            our_cols, gnu_cols,
            "Column count should match: ours={}, gnu={}",
            our_cols, gnu_cols
        );
    }
}

// ──────────────────────────────────────────────────
// Header/output formatting unit tests
// ──────────────────────────────────────────────────

#[test]
fn test_header_default() {
    let config = DfConfig::default();
    let mut buf = Vec::new();
    print_header(&config, &mut buf).unwrap();
    let header = String::from_utf8(buf).unwrap();
    assert!(header.contains("Filesystem"));
    assert!(header.contains("1K-blocks"));
    assert!(header.contains("Used"));
    assert!(header.contains("Avail"));
    assert!(header.contains("Use%"));
    assert!(header.contains("Mounted on"));
}

#[test]
fn test_header_inodes() {
    let config = DfConfig {
        inodes: true,
        ..DfConfig::default()
    };
    let mut buf = Vec::new();
    print_header(&config, &mut buf).unwrap();
    let header = String::from_utf8(buf).unwrap();
    assert!(header.contains("Inodes"));
    assert!(header.contains("IUsed"));
    assert!(header.contains("IFree"));
    assert!(header.contains("IUse%"));
}

#[test]
fn test_header_with_type() {
    let config = DfConfig {
        print_type: true,
        ..DfConfig::default()
    };
    let mut buf = Vec::new();
    print_header(&config, &mut buf).unwrap();
    let header = String::from_utf8(buf).unwrap();
    assert!(header.contains("Type"));
}

#[test]
fn test_header_portability() {
    let config = DfConfig {
        portability: true,
        ..DfConfig::default()
    };
    let mut buf = Vec::new();
    print_header(&config, &mut buf).unwrap();
    let header = String::from_utf8(buf).unwrap();
    assert!(header.contains("Capacity"));
    assert!(header.contains("Available"));
}

#[test]
fn test_print_fs_line_output() {
    let config = DfConfig::default();
    let info = FsInfo {
        source: "/dev/sda1".to_string(),
        fstype: "ext4".to_string(),
        target: "/".to_string(),
        total: 100 * 1024 * 1024,
        used: 60 * 1024 * 1024,
        available: 40 * 1024 * 1024,
        use_percent: 60.0,
        itotal: 1000000,
        iused: 100000,
        iavail: 900000,
        iuse_percent: 10.0,
    };
    let mut buf = Vec::new();
    print_fs_line(&info, &config, &mut buf).unwrap();
    let line = String::from_utf8(buf).unwrap();
    assert!(line.contains("/dev/sda1"));
    assert!(line.contains("/"));
    assert!(line.contains("60%")); // use_percent=60.0 -> ceil -> 60%
}

#[test]
fn test_total_line() {
    let config = DfConfig::default();
    let filesystems = vec![
        FsInfo {
            source: "/dev/sda1".to_string(),
            fstype: "ext4".to_string(),
            target: "/".to_string(),
            total: 100 * 1024,
            used: 60 * 1024,
            available: 40 * 1024,
            use_percent: 60.0,
            itotal: 1000,
            iused: 100,
            iavail: 900,
            iuse_percent: 10.0,
        },
        FsInfo {
            source: "/dev/sda2".to_string(),
            fstype: "ext4".to_string(),
            target: "/home".to_string(),
            total: 200 * 1024,
            used: 80 * 1024,
            available: 120 * 1024,
            use_percent: 40.0,
            itotal: 2000,
            iused: 200,
            iavail: 1800,
            iuse_percent: 10.0,
        },
    ];
    let mut buf = Vec::new();
    print_total_line(&filesystems, &config, &mut buf).unwrap();
    let line = String::from_utf8(buf).unwrap();
    assert!(line.starts_with("total"));
    // Total size: (100+200)*1024 bytes / 1024 block_size = 300
    assert!(line.contains("300"));
}
