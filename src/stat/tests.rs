use super::*;

// ──────────────────────────────────────────────────
// mode_to_human tests
// ──────────────────────────────────────────────────

#[test]
fn test_mode_to_human_regular_644() {
    // Regular file 0644: -rw-r--r--
    let mode = libc::S_IFREG as u32 | 0o644;
    assert_eq!(mode_to_human(mode), "-rw-r--r--");
}

#[test]
fn test_mode_to_human_regular_755() {
    // Regular file 0755: -rwxr-xr-x
    let mode = libc::S_IFREG as u32 | 0o755;
    assert_eq!(mode_to_human(mode), "-rwxr-xr-x");
}

#[test]
fn test_mode_to_human_directory() {
    let mode = libc::S_IFDIR as u32 | 0o755;
    assert_eq!(mode_to_human(mode), "drwxr-xr-x");
}

#[test]
fn test_mode_to_human_symlink() {
    let mode = libc::S_IFLNK as u32 | 0o777;
    assert_eq!(mode_to_human(mode), "lrwxrwxrwx");
}

#[test]
fn test_mode_to_human_setuid() {
    let mode = libc::S_IFREG as u32 | 0o4755;
    assert_eq!(mode_to_human(mode), "-rwsr-xr-x");
}

#[test]
fn test_mode_to_human_setgid() {
    let mode = libc::S_IFREG as u32 | 0o2755;
    assert_eq!(mode_to_human(mode), "-rwxr-sr-x");
}

#[test]
fn test_mode_to_human_sticky() {
    let mode = libc::S_IFDIR as u32 | 0o1755;
    assert_eq!(mode_to_human(mode), "drwxr-xr-t");
}

#[test]
fn test_mode_to_human_setuid_no_exec() {
    // setuid but no owner execute => 'S'
    let mode = libc::S_IFREG as u32 | 0o4644;
    assert_eq!(mode_to_human(mode), "-rwSr--r--");
}

#[test]
fn test_mode_to_human_sticky_no_exec() {
    // sticky but no other execute => 'T'
    let mode = libc::S_IFDIR as u32 | 0o1754;
    assert_eq!(mode_to_human(mode), "drwxr-xr-T");
}

// ──────────────────────────────────────────────────
// file_type_label tests
// ──────────────────────────────────────────────────

#[test]
fn test_file_type_label_regular() {
    assert_eq!(file_type_label(libc::S_IFREG as u32), "regular file");
}

#[test]
fn test_file_type_label_directory() {
    assert_eq!(file_type_label(libc::S_IFDIR as u32), "directory");
}

#[test]
fn test_file_type_label_symlink() {
    assert_eq!(file_type_label(libc::S_IFLNK as u32), "symbolic link");
}

#[test]
fn test_file_type_label_block() {
    assert_eq!(file_type_label(libc::S_IFBLK as u32), "block special file");
}

#[test]
fn test_file_type_label_char() {
    assert_eq!(
        file_type_label(libc::S_IFCHR as u32),
        "character special file"
    );
}

#[test]
fn test_file_type_label_fifo() {
    assert_eq!(file_type_label(libc::S_IFIFO as u32), "fifo");
}

#[test]
fn test_file_type_label_socket() {
    assert_eq!(file_type_label(libc::S_IFSOCK as u32), "socket");
}

// ──────────────────────────────────────────────────
// expand_backslash_escapes tests
// ──────────────────────────────────────────────────

#[test]
fn test_expand_newline() {
    assert_eq!(expand_backslash_escapes("a\\nb"), "a\nb");
}

#[test]
fn test_expand_tab() {
    assert_eq!(expand_backslash_escapes("a\\tb"), "a\tb");
}

#[test]
fn test_expand_backslash() {
    assert_eq!(expand_backslash_escapes("a\\\\b"), "a\\b");
}

#[test]
fn test_expand_octal() {
    assert_eq!(expand_backslash_escapes("\\0101"), "A"); // 0101 octal = 65 = 'A'
}

#[test]
fn test_expand_no_escapes() {
    assert_eq!(expand_backslash_escapes("hello"), "hello");
}

// ──────────────────────────────────────────────────
// Integration tests using stat_file
// ──────────────────────────────────────────────────

#[test]
fn test_stat_default_format() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: None,
        printf_format: None,
        terse: false,
    };

    let path_str = file_path.to_str().unwrap();
    let result = stat_file(path_str, &config).unwrap();

    // Check that all expected sections are present
    assert!(result.contains("File:"), "should contain File: header");
    assert!(result.contains("Size:"), "should contain Size:");
    assert!(result.contains("Blocks:"), "should contain Blocks:");
    assert!(result.contains("IO Block:"), "should contain IO Block:");
    assert!(
        result.contains("regular file"),
        "should identify as regular file"
    );
    assert!(result.contains("Inode:"), "should contain Inode:");
    assert!(result.contains("Links:"), "should contain Links:");
    assert!(result.contains("Access:"), "should contain Access:");
    assert!(result.contains("Modify:"), "should contain Modify:");
    assert!(result.contains("Change:"), "should contain Change:");
    assert!(result.contains("Birth:"), "should contain Birth:");
    assert!(result.contains("Uid:"), "should contain Uid:");
    assert!(result.contains("Gid:"), "should contain Gid:");
}

#[test]
fn test_stat_terse() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("terse.txt");
    std::fs::write(&file_path, "hello").unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: None,
        printf_format: None,
        terse: true,
    };

    let path_str = file_path.to_str().unwrap();
    let result = stat_file(path_str, &config).unwrap();

    // Terse format: name size blocks mode uid gid dev ino nlink rmaj rmin atime mtime ctime atime blksize
    let fields: Vec<&str> = result.trim().split_whitespace().collect();
    assert_eq!(
        fields.len(),
        16,
        "terse format should have 16 fields, got {}: {:?}",
        fields.len(),
        fields
    );
    // First field is the file name
    assert_eq!(fields[0], path_str);
    // Second field is size (5 bytes for "hello")
    assert_eq!(fields[1], "5");
}

#[test]
fn test_stat_custom_format() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("custom.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%s %n".to_string()),
        printf_format: None,
        terse: false,
    };

    let path_str = file_path.to_str().unwrap();
    let result = stat_file(path_str, &config).unwrap();

    // Should be "11 <path>\n"
    let expected = format!("11 {}\n", path_str);
    assert_eq!(result, expected);
}

#[test]
fn test_stat_filesystem() {
    let config = StatConfig {
        dereference: false,
        filesystem: true,
        format: None,
        printf_format: None,
        terse: false,
    };

    let result = stat_file("/", &config).unwrap();

    assert!(result.contains("File:"), "should contain File:");
    assert!(result.contains("Namelen:"), "should contain Namelen:");
    assert!(result.contains("Block size:"), "should contain Block size:");
    assert!(result.contains("Blocks:"), "should contain Blocks:");
    assert!(result.contains("Inodes:"), "should contain Inodes:");
}

#[test]
fn test_stat_symlink_deref() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("target.txt");
    let link_path = dir.path().join("link.txt");
    std::fs::write(&file_path, "content").unwrap();
    std::os::unix::fs::symlink(&file_path, &link_path).unwrap();

    // Without dereference, should show symlink info
    let config_no_deref = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%F".to_string()),
        printf_format: None,
        terse: false,
    };
    let link_str = link_path.to_str().unwrap();
    let result = stat_file(link_str, &config_no_deref).unwrap();
    assert_eq!(result.trim(), "symbolic link");

    // With dereference, should show regular file info
    let config_deref = StatConfig {
        dereference: true,
        filesystem: false,
        format: Some("%F".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(link_str, &config_deref).unwrap();
    assert_eq!(result.trim(), "regular file");
}

#[test]
fn test_stat_format_specifiers() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("specs.txt");
    std::fs::write(&file_path, "test data here").unwrap();

    let path_str = file_path.to_str().unwrap();

    // Test %s (size)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%s".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result.trim(), "14", "size should be 14 bytes");

    // Test %n (file name)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%n".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result.trim(), path_str);

    // Test %F (file type)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%F".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result.trim(), "regular file");

    // Test %a (access rights in octal)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%a".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    // Should be a valid octal number
    let octal_val = result.trim();
    assert!(
        u32::from_str_radix(octal_val, 8).is_ok(),
        "access rights should be valid octal: {}",
        octal_val
    );

    // Test %A (human-readable permissions)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%A".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let perms = result.trim();
    assert_eq!(perms.len(), 10, "permission string should be 10 chars");
    assert!(perms.starts_with('-'), "regular file should start with '-'");

    // Test %h (hard links)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%h".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let nlinks: u64 = result.trim().parse().unwrap();
    assert!(nlinks >= 1, "should have at least 1 hard link");

    // Test %i (inode)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%i".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let ino: u64 = result.trim().parse().unwrap();
    assert!(ino > 0, "inode number should be positive");

    // Test %u and %U (uid and username)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%u".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let _uid: u32 = result.trim().parse().unwrap();

    // Test %N (quoted name)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%N".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let trimmed = result.trim();
    assert!(
        trimmed.starts_with('\'') && trimmed.ends_with('\''),
        "%%N should produce quoted name: {}",
        trimmed
    );

    // Test %B (block size constant)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%B".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result.trim(), "512");

    // Test %d and %D (device number decimal and hex)
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%d".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let _dev: u64 = result.trim().parse().unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%D".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    let _dev_hex = u64::from_str_radix(result.trim(), 16).unwrap();
}

#[test]
fn test_stat_matches_gnu() {
    // Compare our output with GNU stat for a known file
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("gnu_compare.txt");
    std::fs::write(&file_path, "gnu test data").unwrap();

    let path_str = file_path.to_str().unwrap();

    // Compare size
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%s".to_string()),
        printf_format: None,
        terse: false,
    };
    let our_result = stat_file(path_str, &config).unwrap();

    let gnu = std::process::Command::new("stat")
        .args(["-c", "%s", path_str])
        .output();
    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_size = String::from_utf8_lossy(&gnu_out.stdout);
            assert_eq!(
                our_result.trim(),
                gnu_size.trim(),
                "size should match GNU stat"
            );
        }
    }

    // Compare inode
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%i".to_string()),
        printf_format: None,
        terse: false,
    };
    let our_result = stat_file(path_str, &config).unwrap();

    let gnu = std::process::Command::new("stat")
        .args(["-c", "%i", path_str])
        .output();
    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_ino = String::from_utf8_lossy(&gnu_out.stdout);
            assert_eq!(
                our_result.trim(),
                gnu_ino.trim(),
                "inode should match GNU stat"
            );
        }
    }

    // Compare file type
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%F".to_string()),
        printf_format: None,
        terse: false,
    };
    let our_result = stat_file(path_str, &config).unwrap();

    let gnu = std::process::Command::new("stat")
        .args(["-c", "%F", path_str])
        .output();
    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_type = String::from_utf8_lossy(&gnu_out.stdout);
            assert_eq!(
                our_result.trim(),
                gnu_type.trim(),
                "file type should match GNU stat"
            );
        }
    }

    // Compare permissions in octal
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%a".to_string()),
        printf_format: None,
        terse: false,
    };
    let our_result = stat_file(path_str, &config).unwrap();

    let gnu = std::process::Command::new("stat")
        .args(["-c", "%a", path_str])
        .output();
    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_perms = String::from_utf8_lossy(&gnu_out.stdout);
            assert_eq!(
                our_result.trim(),
                gnu_perms.trim(),
                "permissions should match GNU stat"
            );
        }
    }

    // Compare human-readable permissions
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%A".to_string()),
        printf_format: None,
        terse: false,
    };
    let our_result = stat_file(path_str, &config).unwrap();

    let gnu = std::process::Command::new("stat")
        .args(["-c", "%A", path_str])
        .output();
    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_human = String::from_utf8_lossy(&gnu_out.stdout);
            assert_eq!(
                our_result.trim(),
                gnu_human.trim(),
                "human-readable permissions should match GNU stat"
            );
        }
    }

    // Compare number of blocks
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%b".to_string()),
        printf_format: None,
        terse: false,
    };
    let our_result = stat_file(path_str, &config).unwrap();

    let gnu = std::process::Command::new("stat")
        .args(["-c", "%b", path_str])
        .output();
    if let Ok(gnu_out) = gnu {
        if gnu_out.status.success() {
            let gnu_blocks = String::from_utf8_lossy(&gnu_out.stdout);
            assert_eq!(
                our_result.trim(),
                gnu_blocks.trim(),
                "block count should match GNU stat"
            );
        }
    }
}

// ──────────────────────────────────────────────────
// Filesystem format specifier tests
// ──────────────────────────────────────────────────

#[test]
fn test_stat_filesystem_format_specifiers() {
    // Test filesystem specifiers on root
    let config = StatConfig {
        dereference: false,
        filesystem: true,
        format: Some("%n".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file("/", &config).unwrap();
    assert_eq!(result.trim(), "/");

    // Test %b (total blocks)
    let config = StatConfig {
        dereference: false,
        filesystem: true,
        format: Some("%b".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file("/", &config).unwrap();
    let blocks: u64 = result.trim().parse().unwrap();
    assert!(blocks > 0, "total blocks should be positive");

    // Test %s (block size)
    let config = StatConfig {
        dereference: false,
        filesystem: true,
        format: Some("%s".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file("/", &config).unwrap();
    let bsize: u64 = result.trim().parse().unwrap();
    assert!(bsize > 0, "block size should be positive");
}

// ──────────────────────────────────────────────────
// Printf format tests
// ──────────────────────────────────────────────────

#[test]
fn test_stat_printf_format() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("printf.txt");
    std::fs::write(&file_path, "12345").unwrap();

    let path_str = file_path.to_str().unwrap();

    // --printf should interpret backslash escapes and NOT add trailing newline
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: None,
        printf_format: Some("size=%s\\n".to_string()),
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result, "size=5\n");
}

// ──────────────────────────────────────────────────
// Directory stat test
// ──────────────────────────────────────────────────

#[test]
fn test_stat_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path_str = dir.path().to_str().unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%F".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result.trim(), "directory");
}

// ──────────────────────────────────────────────────
// Error handling test
// ──────────────────────────────────────────────────

#[test]
fn test_stat_nonexistent() {
    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: None,
        printf_format: None,
        terse: false,
    };
    let result = stat_file("/nonexistent_stat_test_file_12345", &config);
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────
// Symlink %N specifier test
// ──────────────────────────────────────────────────

#[test]
fn test_stat_symlink_name_specifier() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.txt");
    let link = dir.path().join("sym.txt");
    std::fs::write(&target, "data").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let link_str = link.to_str().unwrap();
    let target_str = target.to_str().unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("%N".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(link_str, &config).unwrap();
    let trimmed = result.trim();

    // Should show 'link' -> 'target'
    assert!(
        trimmed.contains("->"),
        "symlink %%N should contain '->': {}",
        trimmed
    );
    assert!(
        trimmed.contains(target_str),
        "symlink %%N should contain target path: {}",
        trimmed
    );
}

// ──────────────────────────────────────────────────
// Filesystem terse format test
// ──────────────────────────────────────────────────

#[test]
fn test_stat_filesystem_terse() {
    let config = StatConfig {
        dereference: false,
        filesystem: true,
        format: None,
        printf_format: None,
        terse: true,
    };
    let result = stat_file("/", &config).unwrap();

    let fields: Vec<&str> = result.trim().split_whitespace().collect();
    assert_eq!(
        fields.len(),
        12,
        "filesystem terse format should have 12 fields, got {}: {:?}",
        fields.len(),
        fields
    );
    assert_eq!(fields[0], "/");
}

// ──────────────────────────────────────────────────
// Escaped percent test
// ──────────────────────────────────────────────────

#[test]
fn test_stat_escaped_percent() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("pct.txt");
    std::fs::write(&file_path, "x").unwrap();

    let path_str = file_path.to_str().unwrap();

    let config = StatConfig {
        dereference: false,
        filesystem: false,
        format: Some("100%%".to_string()),
        printf_format: None,
        terse: false,
    };
    let result = stat_file(path_str, &config).unwrap();
    assert_eq!(result.trim(), "100%");
}
