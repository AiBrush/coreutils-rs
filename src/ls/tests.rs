use super::*;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("alpha.txt"), "hello\n").unwrap();
    fs::write(dir.path().join("beta.txt"), "world\n").unwrap();
    fs::write(dir.path().join("gamma.rs"), "fn main() {}\n").unwrap();
    fs::create_dir(dir.path().join("subdir")).unwrap();
    dir
}

fn default_config() -> LsConfig {
    let mut cfg = LsConfig::default();
    cfg.format = OutputFormat::SingleColumn;
    cfg.width = 80;
    cfg
}

/// Get the path to a built binary. Works in both lib tests and integration tests.
fn bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove 'deps'
    path.push(name);
    path
}

// ---------------------------------------------------------------------------
// test_ls_default
// ---------------------------------------------------------------------------

#[test]
fn test_ls_default() {
    let dir = setup_dir();
    let config = default_config();
    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"alpha.txt"));
    assert!(names.contains(&"beta.txt"));
    assert!(names.contains(&"gamma.rs"));
    assert!(names.contains(&"subdir"));
    assert_eq!(names.len(), 4);
}

// ---------------------------------------------------------------------------
// test_ls_all (-a)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_all() {
    let dir = setup_dir();
    fs::write(dir.path().join(".hidden"), "secret").unwrap();

    let mut config = default_config();
    config.all = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"."));
    assert!(names.contains(&".."));
    assert!(names.contains(&".hidden"));
    assert!(names.contains(&"alpha.txt"));
}

// ---------------------------------------------------------------------------
// test_ls_almost_all (-A)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_almost_all() {
    let dir = setup_dir();
    fs::write(dir.path().join(".hidden"), "secret").unwrap();

    let mut config = default_config();
    config.almost_all = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    // Should include .hidden but NOT . and ..
    assert!(names.contains(&".hidden"));
    assert!(!names.contains(&"."));
    assert!(!names.contains(&".."));
}

// ---------------------------------------------------------------------------
// test_ls_long (-l)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_long() {
    let dir = setup_dir();
    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_long(&entries, &config).unwrap();

    // Each line should have permission string at start
    for line in output.lines() {
        let first_char = line.chars().next().unwrap();
        assert!(
            "dlcbps-".contains(first_char),
            "Long format line should start with file type char, got: {}",
            line
        );
    }

    // Should contain the filenames
    assert!(output.contains("alpha.txt"));
    assert!(output.contains("subdir"));
}

// ---------------------------------------------------------------------------
// test_ls_long_human (-lh)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_long_human() {
    let dir = TempDir::new().unwrap();
    // Write a larger file
    let data = vec![b'A'; 2048];
    fs::write(dir.path().join("big.txt"), &data).unwrap();

    let mut config = default_config();
    config.long_format = true;
    config.human_readable = true;
    config.format = OutputFormat::Long;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_long(&entries, &config).unwrap();
    // 2048 bytes should show as 2.0K
    assert!(
        output.contains("2.0K") || output.contains("2048"),
        "Expected human readable size for 2048 bytes, got: {}",
        output
    );
}

// ---------------------------------------------------------------------------
// test_ls_recursive (-R)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_recursive() {
    let dir = setup_dir();
    fs::write(dir.path().join("subdir").join("nested.txt"), "inner\n").unwrap();

    let mut config = default_config();
    config.recursive = true;

    let output = render_dir(dir.path(), &config).unwrap();
    // Should contain the nested file
    assert!(output.contains("nested.txt"), "Recursive should show nested files");
    // Should contain the subdir header
    assert!(output.contains("subdir:"), "Recursive should show subdir header");
}

// ---------------------------------------------------------------------------
// test_ls_sort_size (-S)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_sort_size() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("small"), "a").unwrap();
    fs::write(dir.path().join("medium"), "abcdefghij").unwrap();
    fs::write(dir.path().join("large"), "a".repeat(1000)).unwrap();

    let mut config = default_config();
    config.sort_by = SortBy::Size;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names[0], "large", "Largest should be first with -S");
    assert_eq!(names[1], "medium");
    assert_eq!(names[2], "small");
}

// ---------------------------------------------------------------------------
// test_ls_sort_time (-t)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_sort_time() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("older"), "first").unwrap();
    // Sleep briefly so mtime differs
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(dir.path().join("newer"), "second").unwrap();

    let mut config = default_config();
    config.sort_by = SortBy::Time;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names[0], "newer", "Newest should be first with -t");
    assert_eq!(names[1], "older");
}

// ---------------------------------------------------------------------------
// test_ls_reverse (-r)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_reverse() {
    let dir = setup_dir();
    let mut config = default_config();
    config.reverse = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    // Default sort is by name, reversed: subdir, gamma.rs, beta.txt, alpha.txt
    // The actual order depends on case handling; just check it IS reversed
    // relative to non-reversed.
    let mut config2 = default_config();
    config2.reverse = false;
    let entries2 = collect_entries(dir.path(), &config2).unwrap();
    let names2: Vec<&str> = entries2.iter().map(|e| e.name.as_str()).collect();

    let mut reversed_names2 = names2.clone();
    reversed_names2.reverse();
    assert_eq!(names, reversed_names2, "Reverse should reverse the sort");
}

// ---------------------------------------------------------------------------
// test_ls_inode (-i)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_inode() {
    let dir = setup_dir();
    let mut config = default_config();
    config.show_inode = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_single_column(&entries, &config).unwrap();

    // Each line should start with an inode number
    for line in output.lines() {
        let first_token = line.split_whitespace().next().unwrap();
        assert!(
            first_token.parse::<u64>().is_ok(),
            "Line should start with inode number: {}",
            line
        );
    }
}

// ---------------------------------------------------------------------------
// test_ls_classify (-F)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_classify() {
    let dir = setup_dir();
    // Make a file executable
    let exec_path = dir.path().join("script.sh");
    fs::write(&exec_path, "#!/bin/sh\n").unwrap();
    let mut perms = fs::metadata(&exec_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&exec_path, perms).unwrap();

    let mut config = default_config();
    config.indicator_style = IndicatorStyle::Classify;
    config.classify = ClassifyMode::Always;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_single_column(&entries, &config).unwrap();

    // Directories should end with /
    assert!(
        output.contains("subdir/"),
        "Directory should have / indicator: {}",
        output
    );
    // Executable should end with *
    assert!(
        output.contains("script.sh*"),
        "Executable should have * indicator: {}",
        output
    );
}

// ---------------------------------------------------------------------------
// test_ls_one_per_line (-1)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_one_per_line() {
    let dir = setup_dir();
    let config = default_config(); // already SingleColumn
    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_single_column(&entries, &config).unwrap();

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 4);
}

// ---------------------------------------------------------------------------
// test_ls_directory (-d)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_directory() {
    let dir = setup_dir();
    let mut config = default_config();
    config.directory = true;

    // When -d is set, ls_main treats the path as a file entry
    let path_str = dir.path().to_string_lossy().to_string();
    let entry =
        FileEntry::from_path_with_name(path_str.clone(), dir.path(), &config).unwrap();
    assert!(entry.is_dir, "Directory entry should be a directory");
    assert_eq!(entry.name, path_str);
}

// ---------------------------------------------------------------------------
// test_ls_numeric (-n)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_numeric() {
    let dir = setup_dir();
    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;
    config.numeric_ids = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_long(&entries, &config).unwrap();

    // UID and GID should be numeric
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        // tokens[2] = owner (uid), tokens[3] = group (gid)
        if tokens.len() >= 4 {
            assert!(
                tokens[2].parse::<u32>().is_ok(),
                "Owner should be numeric: {}",
                tokens[2]
            );
            assert!(
                tokens[3].parse::<u32>().is_ok(),
                "Group should be numeric: {}",
                tokens[3]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// test_ls_permission_format
// ---------------------------------------------------------------------------

#[test]
fn test_ls_permission_format() {
    // Regular file: -rw-r--r--
    let perm = format_permissions(libc::S_IFREG | 0o644);
    assert_eq!(perm, "-rw-r--r--");

    // Directory: drwxr-xr-x
    let perm = format_permissions(libc::S_IFDIR | 0o755);
    assert_eq!(perm, "drwxr-xr-x");

    // Symlink: lrwxrwxrwx
    let perm = format_permissions(libc::S_IFLNK | 0o777);
    assert_eq!(perm, "lrwxrwxrwx");

    // Setuid: -rwsr-xr-x
    let perm = format_permissions(libc::S_IFREG | 0o4755);
    assert_eq!(perm, "-rwsr-xr-x");

    // Setgid: -rwxr-sr-x
    let perm = format_permissions(libc::S_IFREG | 0o2755);
    assert_eq!(perm, "-rwxr-sr-x");

    // Sticky: drwxrwxrwt
    let perm = format_permissions(libc::S_IFDIR | 0o1777);
    assert_eq!(perm, "drwxrwxrwt");

    // Setuid without exec: -rwSr--r--
    let perm = format_permissions(libc::S_IFREG | 0o4644);
    assert_eq!(perm, "-rwSr--r--");

    // Sticky without other-exec: drwxrwxrwT
    let perm = format_permissions(libc::S_IFDIR | 0o1776);
    assert_eq!(perm, "drwxrwxrwT");

    // Pipe: prw-r--r--
    let perm = format_permissions(libc::S_IFIFO | 0o644);
    assert_eq!(perm, "prw-r--r--");

    // Socket: srwxrwxrwx
    let perm = format_permissions(libc::S_IFSOCK | 0o777);
    assert_eq!(perm, "srwxrwxrwx");

    // Block device
    let perm = format_permissions(libc::S_IFBLK | 0o660);
    assert_eq!(perm, "brw-rw----");

    // Char device
    let perm = format_permissions(libc::S_IFCHR | 0o666);
    assert_eq!(perm, "crw-rw-rw-");
}

// ---------------------------------------------------------------------------
// test_ls_hidden_files
// ---------------------------------------------------------------------------

#[test]
fn test_ls_hidden_files() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(".hidden"), "secret").unwrap();
    fs::write(dir.path().join("visible"), "public").unwrap();

    // Default: hidden files excluded
    let config = default_config();
    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&".hidden"));
    assert!(names.contains(&"visible"));

    // -a: hidden files included
    let mut config_a = default_config();
    config_a.all = true;
    let entries_a = collect_entries(dir.path(), &config_a).unwrap();
    let names_a: Vec<&str> = entries_a.iter().map(|e| e.name.as_str()).collect();
    assert!(names_a.contains(&".hidden"));
}

// ---------------------------------------------------------------------------
// test_ls_symlink_display
// ---------------------------------------------------------------------------

#[test]
fn test_ls_symlink_display() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("target.txt");
    let link = dir.path().join("link.txt");
    fs::write(&target, "content").unwrap();
    unix_fs::symlink(&target, &link).unwrap();

    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_long(&entries, &config).unwrap();

    // Should show "link.txt -> target.txt" (or full path)
    assert!(
        output.contains("link.txt -> "),
        "Symlink should show target: {}",
        output
    );
}

// ---------------------------------------------------------------------------
// test_ls_sort_extension (-X)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_sort_extension() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.c"), "").unwrap();
    fs::write(dir.path().join("file.a"), "").unwrap();
    fs::write(dir.path().join("file.b"), "").unwrap();

    let mut config = default_config();
    config.sort_by = SortBy::Extension;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["file.a", "file.b", "file.c"]);
}

// ---------------------------------------------------------------------------
// test_ls_sort_version (-v)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_sort_version() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file10"), "").unwrap();
    fs::write(dir.path().join("file2"), "").unwrap();
    fs::write(dir.path().join("file1"), "").unwrap();

    let mut config = default_config();
    config.sort_by = SortBy::Version;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["file1", "file2", "file10"]);
}

// ---------------------------------------------------------------------------
// test_ls_unsorted (-U)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_unsorted() {
    let dir = setup_dir();
    let mut config = default_config();
    config.sort_by = SortBy::None;

    let entries = collect_entries(dir.path(), &config).unwrap();
    // Just verify we get all 4 entries
    assert_eq!(entries.len(), 4);
}

// ---------------------------------------------------------------------------
// test_ls_group_directories_first
// ---------------------------------------------------------------------------

#[test]
fn test_ls_group_directories_first() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("afile"), "").unwrap();
    fs::create_dir(dir.path().join("zdir")).unwrap();
    fs::write(dir.path().join("bfile"), "").unwrap();

    let mut config = default_config();
    config.group_directories_first = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names[0], "zdir", "Directory should come first");
}

// ---------------------------------------------------------------------------
// test_ls_ignore_backups (-B)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_ignore_backups() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.txt"), "").unwrap();
    fs::write(dir.path().join("file.txt~"), "").unwrap();

    let mut config = default_config();
    config.ignore_backups = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"file.txt"));
    assert!(!names.contains(&"file.txt~"));
}

// ---------------------------------------------------------------------------
// test_ls_ignore_pattern (-I)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_ignore_pattern() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.txt"), "").unwrap();
    fs::write(dir.path().join("file.log"), "").unwrap();
    fs::write(dir.path().join("data.csv"), "").unwrap();

    let mut config = default_config();
    config.ignore_patterns = vec!["*.log".to_string()];

    let entries = collect_entries(dir.path(), &config).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"file.txt"));
    assert!(!names.contains(&"file.log"));
    assert!(names.contains(&"data.csv"));
}

// ---------------------------------------------------------------------------
// test_ls_comma_format (-m)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_comma_format() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a"), "").unwrap();
    fs::write(dir.path().join("b"), "").unwrap();
    fs::write(dir.path().join("c"), "").unwrap();

    let mut config = default_config();
    config.format = OutputFormat::Comma;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let mut buf = Vec::new();
    print_comma(&mut buf, &entries, &config, None).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(output.contains(", "), "Comma format should have ', ': {}", output);
    assert!(output.ends_with('\n'));
}

// ---------------------------------------------------------------------------
// test_ls_no_group (-G / -o)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_no_group() {
    let dir = setup_dir();
    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;
    config.show_group = false;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output_no_group = render_long(&entries, &config).unwrap();

    config.show_group = true;
    let output_with_group = render_long(&entries, &config).unwrap();

    // Without group, lines should be shorter
    let line_ng = output_no_group.lines().next().unwrap();
    let line_wg = output_with_group.lines().next().unwrap();
    assert!(
        line_ng.len() <= line_wg.len(),
        "No-group output should be shorter or equal"
    );
}

// ---------------------------------------------------------------------------
// test_ls_no_owner (-g)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_no_owner() {
    let dir = setup_dir();
    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;
    config.show_owner = false;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_long(&entries, &config).unwrap();
    assert!(
        !output.is_empty(),
        "Output without owner should still produce output"
    );
}

// ---------------------------------------------------------------------------
// test_format_size
// ---------------------------------------------------------------------------

#[test]
fn test_format_size_units() {
    assert_eq!(format_size(0, true, false, false), "0");
    assert_eq!(format_size(500, true, false, false), "500");
    assert_eq!(format_size(1024, true, false, false), "1.0K");
    assert_eq!(format_size(1536, true, false, false), "1.5K");
    assert_eq!(format_size(1048576, true, false, false), "1.0M");
    assert_eq!(format_size(1073741824, true, false, false), "1.0G");

    // SI units (powers of 1000)
    assert_eq!(format_size(1000, false, true, false), "1.0K");
    assert_eq!(format_size(1500, false, true, false), "1.5K");
    assert_eq!(format_size(1000000, false, true, false), "1.0M");
}

// ---------------------------------------------------------------------------
// test_quoting_styles
// ---------------------------------------------------------------------------

#[test]
fn test_quoting_styles() {
    let mut config = default_config();

    // Literal
    config.quoting_style = QuotingStyle::Literal;
    assert_eq!(quote_name("hello", &config), "hello");

    // C-style
    config.quoting_style = QuotingStyle::C;
    assert_eq!(quote_name("hello", &config), "\"hello\"");
    assert_eq!(quote_name("hel\"lo", &config), "\"hel\\\"lo\"");

    // Escape
    config.quoting_style = QuotingStyle::Escape;
    assert_eq!(quote_name("hello", &config), "hello");
    assert_eq!(quote_name("hel\\lo", &config), "hel\\\\lo");

    // Shell
    config.quoting_style = QuotingStyle::Shell;
    assert_eq!(quote_name("hello", &config), "hello");
    assert_eq!(quote_name("hello world", &config), "'hello world'");

    // Shell-always
    config.quoting_style = QuotingStyle::ShellAlways;
    assert_eq!(quote_name("hello", &config), "'hello'");
}

// ---------------------------------------------------------------------------
// test_version_sort
// ---------------------------------------------------------------------------

#[test]
fn test_version_sort_cmp() {
    assert_eq!(version_cmp("file1", "file2"), Ordering::Less);
    assert_eq!(version_cmp("file2", "file10"), Ordering::Less);
    assert_eq!(version_cmp("file10", "file2"), Ordering::Greater);
    assert_eq!(version_cmp("file1", "file1"), Ordering::Equal);
    assert_eq!(version_cmp("abc", "def"), Ordering::Less);
}

// ---------------------------------------------------------------------------
// test_glob_match
// ---------------------------------------------------------------------------

#[test]
fn test_glob_match_patterns() {
    assert!(glob_match("*.txt", "file.txt"));
    assert!(!glob_match("*.txt", "file.rs"));
    assert!(glob_match("*", "anything"));
    assert!(glob_match("file?", "file1"));
    assert!(!glob_match("file?", "file10"));
    assert!(glob_match("*.tar.gz", "archive.tar.gz"));
}

// ---------------------------------------------------------------------------
// test_ls_dereference (-L)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_dereference() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("target.txt");
    let link = dir.path().join("link.txt");
    fs::write(&target, "content").unwrap();
    unix_fs::symlink(&target, &link).unwrap();

    let mut config = default_config();
    config.dereference = true;
    config.long_format = true;
    config.format = OutputFormat::Long;

    let entries = collect_entries(dir.path(), &config).unwrap();
    // With dereference, the symlink should show as a regular file (no 'l' prefix)
    let link_entry = entries.iter().find(|e| e.name == "link.txt").unwrap();
    let perms = format_permissions(link_entry.mode);
    assert!(
        perms.starts_with('-'),
        "Dereferenced symlink should show as regular file: {}",
        perms
    );
}

// ---------------------------------------------------------------------------
// test_ls_kibibytes (-k)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_kibibytes() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file.txt"), "data").unwrap();

    let mut config = default_config();
    config.show_size = true;
    config.kibibytes = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_single_column(&entries, &config).unwrap();
    // Each line should start with a number (the block count in K)
    for line in output.lines() {
        let first = line.split_whitespace().next().unwrap();
        assert!(
            first.parse::<u64>().is_ok(),
            "Block size should be numeric: {}",
            first
        );
    }
}

// ---------------------------------------------------------------------------
// test_ls_indicator_slash (-p)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_indicator_slash() {
    let dir = setup_dir();
    let mut config = default_config();
    config.indicator_style = IndicatorStyle::Slash;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_single_column(&entries, &config).unwrap();
    assert!(
        output.contains("subdir/"),
        "Slash indicator should append / to dirs: {}",
        output
    );
    // Regular files should NOT have /
    assert!(
        !output.contains("alpha.txt/"),
        "Regular files should not have /: {}",
        output
    );
}

// ---------------------------------------------------------------------------
// test_ls_hide_control_chars (-q)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_hide_control_chars() {
    let mut config = default_config();
    config.hide_control_chars = true;
    config.quoting_style = QuotingStyle::Literal;

    let result = quote_name("hello\x01world", &config);
    assert_eq!(result, "hello?world");
}

// ---------------------------------------------------------------------------
// test_ls_show_size (-s)
// ---------------------------------------------------------------------------

#[test]
fn test_ls_show_size() {
    let dir = setup_dir();
    let mut config = default_config();
    config.show_size = true;

    let entries = collect_entries(dir.path(), &config).unwrap();
    let output = render_single_column(&entries, &config).unwrap();
    for line in output.lines() {
        let first = line.split_whitespace().next().unwrap();
        assert!(
            first.parse::<u64>().is_ok(),
            "Should start with block size: {}",
            first
        );
    }
}

// ---------------------------------------------------------------------------
// test_ls_time_field
// ---------------------------------------------------------------------------

#[test]
fn test_ls_time_field() {
    let dir = setup_dir();
    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;

    // Mtime (default)
    config.time_field = TimeField::Mtime;
    let entries = collect_entries(dir.path(), &config).unwrap();
    let output_m = render_long(&entries, &config).unwrap();
    assert!(!output_m.is_empty());

    // Atime
    config.time_field = TimeField::Atime;
    let output_a = render_long(&entries, &config).unwrap();
    assert!(!output_a.is_empty());
}

// ---------------------------------------------------------------------------
// test_ls_time_styles
// ---------------------------------------------------------------------------

#[test]
fn test_ls_time_styles() {
    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // full-iso
    let s = format_time(now_secs, 123456789, &TimeStyle::FullIso);
    assert!(s.contains('.'), "full-iso should have nanoseconds: {}", s);
    assert!(s.contains('+') || s.contains('-'), "full-iso should have tz offset: {}", s);

    // long-iso
    let s = format_time(now_secs, 0, &TimeStyle::LongIso);
    assert!(s.contains('-'), "long-iso should have date: {}", s);

    // iso (recent file)
    let s = format_time(now_secs, 0, &TimeStyle::Iso);
    assert!(s.contains(':'), "iso for recent should have time: {}", s);

    // iso (old file)
    let old = now_secs - 365 * 24 * 3600;
    let s = format_time(old, 0, &TimeStyle::Iso);
    assert!(!s.contains(':'), "iso for old file should show year: {}", s);

    // locale (recent)
    let s = format_time(now_secs, 0, &TimeStyle::Locale);
    assert!(s.contains(':'), "locale for recent should have time: {}", s);

    // locale (old)
    let s = format_time(old, 0, &TimeStyle::Locale);
    assert!(!s.contains(':'), "locale for old file should show year: {}", s);
}

// ---------------------------------------------------------------------------
// test_ls_color_db
// ---------------------------------------------------------------------------

#[test]
fn test_ls_color_db_default() {
    let db = ColorDb::default();
    assert!(db.dir.contains("[01;34m"), "Default dir should be bold blue");
    assert!(db.link.contains("[01;36m"), "Default link should be bold cyan");
    assert!(db.exec.contains("[01;32m"), "Default exec should be bold green");
}

// ---------------------------------------------------------------------------
// test_ls_matches_gnu (integration test)
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn test_ls_matches_gnu() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("alpha"), "aaa\n").unwrap();
    fs::write(dir.path().join("beta"), "bbb\n").unwrap();
    fs::create_dir(dir.path().join("gamma")).unwrap();

    let fls = bin_path("fls");
    if !fls.exists() {
        // Binary may not be built yet in CI
        return;
    }

    // Compare our output (-1 mode) with GNU ls
    let our_output = std::process::Command::new(&fls)
        .arg("-1")
        .arg("--color=never")
        .arg(dir.path())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("ls")
        .arg("-1")
        .arg("--color=never")
        .arg(dir.path())
        .output()
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&our_output.stdout),
        String::from_utf8_lossy(&gnu_output.stdout),
        "fls -1 should match GNU ls -1"
    );
}

// ---------------------------------------------------------------------------
// test_binary_help
// ---------------------------------------------------------------------------

#[test]
fn test_binary_help() {
    let fls = bin_path("fls");
    if !fls.exists() {
        return;
    }

    let output = std::process::Command::new(&fls)
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "Help should contain Usage");
    assert!(stdout.contains("--all"), "Help should mention --all");
}

// ---------------------------------------------------------------------------
// test_binary_version
// ---------------------------------------------------------------------------

#[test]
fn test_binary_version() {
    let fls = bin_path("fls");
    if !fls.exists() {
        return;
    }

    let output = std::process::Command::new(&fls)
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ls (fcoreutils)"),
        "Version should identify as fcoreutils"
    );
}

// ---------------------------------------------------------------------------
// test_binary_nonexistent
// ---------------------------------------------------------------------------

#[test]
fn test_binary_nonexistent() {
    let fls = bin_path("fls");
    if !fls.exists() {
        return;
    }

    let output = std::process::Command::new(&fls)
        .arg("/nonexistent/path")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ls:"), "Error should mention ls:");
    assert!(
        stderr.contains("No such file or directory"),
        "Error should explain the problem"
    );
}

// ---------------------------------------------------------------------------
// test_ls_empty_dir
// ---------------------------------------------------------------------------

#[test]
fn test_ls_empty_dir() {
    let dir = TempDir::new().unwrap();
    let config = default_config();
    let entries = collect_entries(dir.path(), &config).unwrap();
    assert!(entries.is_empty(), "Empty dir should have no entries");
}

// ---------------------------------------------------------------------------
// test_ls_total_blocks
// ---------------------------------------------------------------------------

#[test]
fn test_ls_total_blocks() {
    let dir = setup_dir();
    let mut config = default_config();
    config.long_format = true;
    config.format = OutputFormat::Long;

    let output = render_dir(dir.path(), &config).unwrap();
    assert!(
        output.starts_with("total "),
        "Long listing should start with total: {}",
        output
    );
}
