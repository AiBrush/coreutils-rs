use super::*;

fn bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // deps
    path.pop(); // debug
    path.push(name);
    path
}

// ---- unit / library tests ----

#[test]
fn test_cp_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "hello world\n").unwrap();

    let config = CpConfig::default();
    copy_file(&src, &dst, &config).unwrap();

    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello world\n");
}

#[test]
fn test_cp_multiple_to_dir() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let target = dir.path().join("target_dir");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(&a, "aaa\n").unwrap();
    std::fs::write(&b, "bbb\n").unwrap();

    let config = CpConfig {
        target_directory: Some(target.to_str().unwrap().to_string()),
        ..CpConfig::default()
    };

    let sources = vec![
        a.to_str().unwrap().to_string(),
        b.to_str().unwrap().to_string(),
    ];
    let (errors, had_error) = run_cp(&sources, None, &config);
    assert!(!had_error, "errors: {:?}", errors);
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).unwrap(),
        "aaa\n"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("b.txt")).unwrap(),
        "bbb\n"
    );
}

#[test]
fn test_cp_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let src_dir = dir.path().join("src_dir");
    std::fs::create_dir(&src_dir).unwrap();
    std::fs::write(src_dir.join("file1.txt"), "one\n").unwrap();
    let sub = src_dir.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("file2.txt"), "two\n").unwrap();

    let dst_dir = dir.path().join("dst_dir");

    let config = CpConfig {
        recursive: true,
        ..CpConfig::default()
    };
    let sources = vec![src_dir.to_str().unwrap().to_string()];
    let (errors, had_error) = run_cp(&sources, Some(dst_dir.to_str().unwrap()), &config);
    assert!(!had_error, "errors: {:?}", errors);
    assert_eq!(
        std::fs::read_to_string(dst_dir.join("file1.txt")).unwrap(),
        "one\n"
    );
    assert_eq!(
        std::fs::read_to_string(dst_dir.join("sub").join("file2.txt")).unwrap(),
        "two\n"
    );
}

#[cfg(unix)]
#[test]
fn test_cp_preserve_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.sh");
    std::fs::write(&src, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();

    let dst = dir.path().join("dst.sh");
    let config = CpConfig {
        preserve_mode: true,
        ..CpConfig::default()
    };
    copy_file(&src, &dst, &config).unwrap();

    let dst_mode = std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
    assert_eq!(dst_mode, 0o755);
}

#[test]
fn test_cp_force() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "new content\n").unwrap();
    std::fs::write(&dst, "old content\n").unwrap();

    // Make destination read-only.
    let mut perms = std::fs::metadata(&dst).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&dst, perms).unwrap();

    let config = CpConfig {
        force: true,
        ..CpConfig::default()
    };
    let sources = vec![src.to_str().unwrap().to_string()];
    let (errors, had_error) = run_cp(&sources, Some(dst.to_str().unwrap()), &config);
    assert!(!had_error, "errors: {:?}", errors);
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new content\n");
}

#[test]
fn test_cp_no_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "new content\n").unwrap();
    std::fs::write(&dst, "old content\n").unwrap();

    let config = CpConfig {
        no_clobber: true,
        ..CpConfig::default()
    };
    let sources = vec![src.to_str().unwrap().to_string()];
    let (errors, had_error) = run_cp(&sources, Some(dst.to_str().unwrap()), &config);
    assert!(!had_error, "errors: {:?}", errors);
    // Destination should NOT have been overwritten.
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old content\n");
}

#[test]
fn test_cp_verbose() {
    // Verbose flag should not cause errors; we just verify the copy succeeds.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "hello\n").unwrap();

    let config = CpConfig {
        verbose: true,
        ..CpConfig::default()
    };
    let sources = vec![src.to_str().unwrap().to_string()];
    let (errors, had_error) = run_cp(&sources, Some(dst.to_str().unwrap()), &config);
    assert!(!had_error, "errors: {:?}", errors);
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello\n");
}

#[cfg(unix)]
#[test]
fn test_cp_symbolic_link() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("link.txt");
    std::fs::write(&src, "real content\n").unwrap();

    let config = CpConfig {
        symbolic_link: true,
        ..CpConfig::default()
    };
    copy_file(&src, &dst, &config).unwrap();

    let meta = std::fs::symlink_metadata(&dst).unwrap();
    assert!(meta.file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "real content\n");
}

#[test]
fn test_cp_hard_link() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("hard.txt");
    std::fs::write(&src, "link me\n").unwrap();

    let config = CpConfig {
        link: true,
        ..CpConfig::default()
    };
    copy_file(&src, &dst, &config).unwrap();

    // Both should refer to the same inode.
    #[cfg(unix)]
    {
        let src_ino = std::fs::metadata(&src).unwrap().ino();
        let dst_ino = std::fs::metadata(&dst).unwrap().ino();
        assert_eq!(src_ino, dst_ino);
    }

    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "link me\n");
}

#[test]
fn test_cp_update() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");

    // Write destination first so it is older.
    std::fs::write(&dst, "old content\n").unwrap();
    // Small sleep to ensure mtime differs.
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&src, "new content\n").unwrap();

    let config = CpConfig {
        update: true,
        ..CpConfig::default()
    };
    let sources = vec![src.to_str().unwrap().to_string()];
    let (errors, had_error) = run_cp(&sources, Some(dst.to_str().unwrap()), &config);
    assert!(!had_error, "errors: {:?}", errors);
    // Source is newer, so destination should be updated.
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new content\n");
}

#[test]
fn test_cp_update_skip_older() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");

    // Write source first so it is older.
    std::fs::write(&src, "old content\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&dst, "new content\n").unwrap();

    let config = CpConfig {
        update: true,
        ..CpConfig::default()
    };
    let sources = vec![src.to_str().unwrap().to_string()];
    let (errors, had_error) = run_cp(&sources, Some(dst.to_str().unwrap()), &config);
    assert!(!had_error, "errors: {:?}", errors);
    // Source is older, so destination should NOT be overwritten.
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new content\n");
}

#[test]
fn test_cp_roundtrip() {
    // Verify that a copied file is byte-identical to the original.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("data.bin");
    let dst = dir.path().join("copy.bin");

    // Write a non-trivial binary pattern.
    let data: Vec<u8> = (0..=255).cycle().take(100_000).collect();
    std::fs::write(&src, &data).unwrap();

    let config = CpConfig::default();
    copy_file(&src, &dst, &config).unwrap();

    let copied = std::fs::read(&dst).unwrap();
    assert_eq!(data, copied);
}

// ---- integration tests with binary ----

#[test]
fn test_cp_binary_basic() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "hello from fcp\n").unwrap();

    let output = std::process::Command::new(bin_path("fcp"))
        .arg(src.to_str().unwrap())
        .arg(dst.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello from fcp\n");
}

#[test]
fn test_cp_binary_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let src_dir = dir.path().join("orig");
    std::fs::create_dir(&src_dir).unwrap();
    std::fs::write(src_dir.join("a.txt"), "aaa\n").unwrap();
    let sub = src_dir.join("nested");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("b.txt"), "bbb\n").unwrap();

    let dst_dir = dir.path().join("clone");

    let output = std::process::Command::new(bin_path("fcp"))
        .arg("-R")
        .arg(src_dir.to_str().unwrap())
        .arg(dst_dir.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        std::fs::read_to_string(dst_dir.join("a.txt")).unwrap(),
        "aaa\n"
    );
    assert_eq!(
        std::fs::read_to_string(dst_dir.join("nested").join("b.txt")).unwrap(),
        "bbb\n"
    );
}

#[test]
fn test_cp_binary_version() {
    let output = std::process::Command::new(bin_path("fcp"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cp (fcoreutils)"));
}

#[test]
fn test_cp_binary_no_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "new\n").unwrap();
    std::fs::write(&dst, "old\n").unwrap();

    let output = std::process::Command::new(bin_path("fcp"))
        .arg("-n")
        .arg(src.to_str().unwrap())
        .arg(dst.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old\n");
}

#[test]
#[cfg(target_os = "linux")]
fn test_cp_matches_gnu() {
    // Compare our fcp output with system cp for a simple copy.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let our_dst = dir.path().join("our.txt");
    let gnu_dst = dir.path().join("gnu.txt");
    std::fs::write(&src, "GNU compat test\nline 2\n").unwrap();

    let our_output = std::process::Command::new(bin_path("fcp"))
        .arg(src.to_str().unwrap())
        .arg(our_dst.to_str().unwrap())
        .output()
        .unwrap();
    assert!(our_output.status.success());

    let gnu_output = std::process::Command::new("cp")
        .arg(src.to_str().unwrap())
        .arg(gnu_dst.to_str().unwrap())
        .output()
        .unwrap();
    assert!(gnu_output.status.success());

    let our_content = std::fs::read(&our_dst).unwrap();
    let gnu_content = std::fs::read(&gnu_dst).unwrap();
    assert_eq!(our_content, gnu_content);
}
