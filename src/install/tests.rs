use std::fs;
use std::process::Command;

fn cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("finstall");
    Command::new(path)
}

#[test]
fn test_install_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.txt");
    let dst = dir.path().join("dest.txt");
    fs::write(&src, "hello install").unwrap();

    let output = cmd()
        .args([src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dst.exists(), "destination should exist");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "hello install");

    // Default mode should be 0755
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "default install mode should be 0755, got {:o}", mode);
    }
}

#[test]
fn test_install_mode() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("mode_src.txt");
    let dst = dir.path().join("mode_dst.txt");
    fs::write(&src, "content").unwrap();

    let output = cmd()
        .args(["-m", "0644", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -m 0644 should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "mode should be 0644, got {:o}", mode);
    }
}

#[test]
fn test_install_directory() {
    let dir = tempfile::tempdir().unwrap();
    let new_dir = dir.path().join("new_dir");

    let output = cmd()
        .args(["-d", new_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -d should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(new_dir.is_dir(), "directory should be created");
}

#[test]
fn test_install_d_creates_parents() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("a").join("b").join("c").join("dest.txt");
    fs::write(&src, "deep").unwrap();

    let output = cmd()
        .args(["-D", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -D should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dst.exists(), "destination should exist");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "deep");
}

#[test]
fn test_install_compare() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("cmp_src.txt");
    let dst = dir.path().join("cmp_dst.txt");
    fs::write(&src, "same content").unwrap();
    fs::write(&dst, "same content").unwrap();

    // Set a specific mtime on dst
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dst, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let old_meta = fs::metadata(&dst).unwrap();
    let old_mtime = old_meta.modified().unwrap();

    // Small delay
    std::thread::sleep(std::time::Duration::from_millis(50));

    let output = cmd()
        .args(["-C", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -C should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // With -C and identical content, mtime should NOT change
    let new_meta = fs::metadata(&dst).unwrap();
    let new_mtime = new_meta.modified().unwrap();
    assert_eq!(
        old_mtime, new_mtime,
        "mtime should not change when files are identical with -C"
    );
}

#[test]
fn test_install_compare_different() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("cmp_src2.txt");
    let dst = dir.path().join("cmp_dst2.txt");
    fs::write(&src, "new content").unwrap();
    fs::write(&dst, "old content").unwrap();

    let output = cmd()
        .args(["-C", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -C with different content should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // File should be updated since contents differ
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new content");
}

#[test]
fn test_install_verbose() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("verbose_src.txt");
    let dst = dir.path().join("verbose_dst.txt");
    fs::write(&src, "data").unwrap();

    let output = cmd()
        .args(["-v", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("->"),
        "verbose output should contain '->': {}",
        stderr
    );
}

#[test]
fn test_install_matches_gnu() {
    // Compare with GNU install on basic file copy
    let dir = tempfile::tempdir().unwrap();

    let gnu_src = dir.path().join("gnu_src.txt");
    let gnu_dst = dir.path().join("gnu_dst.txt");
    let our_src = dir.path().join("our_src.txt");
    let our_dst = dir.path().join("our_dst.txt");

    fs::write(&gnu_src, "test data").unwrap();
    fs::write(&our_src, "test data").unwrap();

    let gnu = Command::new("install")
        .args([gnu_src.to_str().unwrap(), gnu_dst.to_str().unwrap()])
        .output();

    if let Ok(gnu_output) = gnu {
        let our_output = cmd()
            .args([our_src.to_str().unwrap(), our_dst.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(
            our_output.status.code(),
            gnu_output.status.code(),
            "Exit codes should match"
        );
        if gnu_output.status.success() {
            assert_eq!(
                fs::read_to_string(&gnu_dst).unwrap(),
                fs::read_to_string(&our_dst).unwrap(),
                "File contents should match"
            );

            // Check that both set 0755
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let gnu_mode = fs::metadata(&gnu_dst).unwrap().permissions().mode() & 0o777;
                let our_mode = fs::metadata(&our_dst).unwrap().permissions().mode() & 0o777;
                assert_eq!(
                    gnu_mode, our_mode,
                    "Modes should match: gnu={:o} ours={:o}",
                    gnu_mode, our_mode
                );
            }
        }
    }
}

#[test]
fn test_install_help() {
    let output = cmd().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("install"));
}

#[test]
fn test_install_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("install"));
    assert!(stdout.contains("fcoreutils"));
}

#[test]
fn test_install_missing_operand() {
    let output = cmd().output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing file operand"));
}

#[test]
fn test_install_target_directory() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("td_src.txt");
    let dest_dir = dir.path().join("td_dest");
    fs::write(&src, "content").unwrap();
    fs::create_dir(&dest_dir).unwrap();

    let output = cmd()
        .args([
            "-t",
            dest_dir.to_str().unwrap(),
            src.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(dest_dir.join("td_src.txt").exists());
}

#[test]
fn test_install_preserve_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("ts_src.txt");
    let dst = dir.path().join("ts_dst.txt");
    fs::write(&src, "timestamps").unwrap();

    // Wait a bit so the install time would differ
    std::thread::sleep(std::time::Duration::from_millis(50));

    let output = cmd()
        .args(["-p", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -p should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let src_mtime = fs::metadata(&src).unwrap().modified().unwrap();
    let dst_mtime = fs::metadata(&dst).unwrap().modified().unwrap();
    assert_eq!(
        src_mtime, dst_mtime,
        "modification times should match with -p"
    );
}

#[test]
fn test_install_multiple_to_directory() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("a.txt");
    let f2 = dir.path().join("b.txt");
    let dest = dir.path().join("dest");
    fs::write(&f1, "aaa").unwrap();
    fs::write(&f2, "bbb").unwrap();
    fs::create_dir(&dest).unwrap();

    let output = cmd()
        .args([
            f1.to_str().unwrap(),
            f2.to_str().unwrap(),
            dest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(dest.join("a.txt").exists());
    assert!(dest.join("b.txt").exists());
}

#[test]
fn test_install_backup() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bak_src.txt");
    let dst = dir.path().join("bak_dst.txt");
    fs::write(&src, "new").unwrap();
    fs::write(&dst, "old").unwrap();

    let output = cmd()
        .args(["-b", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    let backup = dir.path().join("bak_dst.txt~");
    assert!(backup.exists(), "backup file should exist");
    assert_eq!(fs::read_to_string(&backup).unwrap(), "old");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
}

#[test]
fn test_install_directory_nested() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("x").join("y").join("z");

    let output = cmd()
        .args(["-d", nested.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install -d with nested dirs should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(nested.is_dir());
}
