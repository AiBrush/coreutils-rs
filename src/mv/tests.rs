use std::fs;
use std::process::Command;

fn cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("fmv");
    Command::new(path)
}

#[test]
fn test_mv_rename() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("source.txt");
    let dst = dir.path().join("dest.txt");
    fs::write(&src, "hello").unwrap();

    let output = cmd()
        .args([src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "mv should succeed");
    assert!(!src.exists(), "source should no longer exist");
    assert!(dst.exists(), "destination should exist");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "hello");
}

#[test]
fn test_mv_to_directory() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("file.txt");
    let dest_dir = dir.path().join("dest");
    fs::create_dir(&dest_dir).unwrap();
    fs::write(&src, "content").unwrap();

    let output = cmd()
        .args([src.to_str().unwrap(), dest_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "mv to directory should succeed");
    assert!(!src.exists(), "source should no longer exist");
    assert!(
        dest_dir.join("file.txt").exists(),
        "file should be in dest dir"
    );
    assert_eq!(
        fs::read_to_string(dest_dir.join("file.txt")).unwrap(),
        "content"
    );
}

#[test]
fn test_mv_force() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("new.txt");
    let dst = dir.path().join("existing.txt");
    fs::write(&src, "new content").unwrap();
    fs::write(&dst, "old content").unwrap();

    // Without --force, mv still overwrites by default (unlike cp).
    // But with -f it should never prompt.
    let output = cmd()
        .args(["-f", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "mv -f should succeed");
    assert!(!src.exists(), "source should be removed");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new content");
}

#[test]
fn test_mv_no_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    fs::write(&src, "new").unwrap();
    fs::write(&dst, "old").unwrap();

    let output = cmd()
        .args(["-n", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "mv -n should succeed (no error)");
    // Source should still exist because dest was not overwritten
    assert!(src.exists(), "source should still exist with -n");
    assert_eq!(
        fs::read_to_string(&dst).unwrap(),
        "old",
        "destination should not be overwritten with -n"
    );
}

#[test]
fn test_mv_verbose() {
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
        stderr.contains("->") || stderr.contains("renamed"),
        "verbose output should contain rename info: {}",
        stderr
    );
}

#[test]
fn test_mv_backup() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("new.txt");
    let dst = dir.path().join("existing.txt");
    fs::write(&src, "new content").unwrap();
    fs::write(&dst, "old content").unwrap();

    let output = cmd()
        .args(["-b", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "mv -b should succeed");

    // The old file should be backed up
    let backup = dir.path().join("existing.txt~");
    assert!(backup.exists(), "backup file should exist");
    assert_eq!(fs::read_to_string(&backup).unwrap(), "old content");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new content");
    assert!(!src.exists(), "source should be removed");
}

#[test]
fn test_mv_matches_gnu() {
    // Compare our mv behavior with GNU mv on basic rename
    let dir = tempfile::tempdir().unwrap();

    let gnu_src = dir.path().join("gnu_src.txt");
    let gnu_dst = dir.path().join("gnu_dst.txt");
    let our_src = dir.path().join("our_src.txt");
    let our_dst = dir.path().join("our_dst.txt");

    fs::write(&gnu_src, "test").unwrap();
    fs::write(&our_src, "test").unwrap();

    let gnu = Command::new("mv")
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
            assert!(!gnu_src.exists(), "GNU mv should have moved source");
            assert!(!our_src.exists(), "our mv should have moved source");
            assert_eq!(
                fs::read_to_string(&gnu_dst).unwrap(),
                fs::read_to_string(&our_dst).unwrap(),
                "File contents should match"
            );
        }
    }
}

#[test]
fn test_mv_help() {
    let output = cmd().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("mv"));
}

#[test]
fn test_mv_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mv"));
    assert!(stdout.contains("fcoreutils"));
}

#[test]
fn test_mv_missing_operand() {
    let output = cmd().output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing file operand"));
}

#[test]
fn test_mv_missing_dest() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("only.txt");
    fs::write(&src, "data").unwrap();

    let output = cmd().arg(src.to_str().unwrap()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing destination"));
}

#[test]
fn test_mv_target_directory() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("td_file.txt");
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
    assert!(dest_dir.join("td_file.txt").exists());
    assert!(!src.exists());
}

#[test]
fn test_mv_no_target_directory() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("nt_src.txt");
    let dst = dir.path().join("nt_dst.txt");
    fs::write(&src, "data").unwrap();

    let output = cmd()
        .args(["-T", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!src.exists());
    assert!(dst.exists());
    assert_eq!(fs::read_to_string(&dst).unwrap(), "data");
}

#[test]
fn test_mv_update_newer() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("newer.txt");
    let dst = dir.path().join("older.txt");

    // Create dst first (older)
    fs::write(&dst, "old").unwrap();
    // Small delay to ensure different timestamps
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Create src after (newer)
    fs::write(&src, "new").unwrap();

    let output = cmd()
        .args(["-u", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    // Source is newer, so it should be moved
    assert!(!src.exists());
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
}

#[test]
fn test_mv_update_older() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("newer_dst.txt");
    let src = dir.path().join("older_src.txt");

    // Create src first (older)
    fs::write(&src, "old").unwrap();
    // Small delay to ensure different timestamps
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Create dst after (newer)
    fs::write(&dst, "new").unwrap();

    let output = cmd()
        .args(["-u", src.to_str().unwrap(), dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    // Source is older, so nothing should happen
    assert!(src.exists(), "source should still exist (older than dest)");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
}

#[test]
fn test_mv_multiple_to_directory() {
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
    assert!(!f1.exists());
    assert!(!f2.exists());
    assert!(dest.join("a.txt").exists());
    assert!(dest.join("b.txt").exists());
}

#[test]
fn test_mv_nonexistent_source() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("dst.txt");

    let output = cmd()
        .args(["/nonexistent_mv_test_12345", dst.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot stat") || stderr.contains("No such file"));
}

#[test]
fn test_mv_backup_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    fs::write(&src, "new").unwrap();
    fs::write(&dst, "old").unwrap();

    let output = cmd()
        .args([
            "--suffix=.bak",
            "-b",
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let backup = dir.path().join("dst.txt.bak");
    assert!(backup.exists(), "backup with custom suffix should exist");
    assert_eq!(fs::read_to_string(&backup).unwrap(), "old");
}

#[test]
fn test_mv_directory() {
    let dir = tempfile::tempdir().unwrap();
    let src_dir = dir.path().join("src_dir");
    let dst_dir = dir.path().join("dst_dir");
    fs::create_dir(&src_dir).unwrap();
    fs::write(src_dir.join("inner.txt"), "inside").unwrap();

    let output = cmd()
        .args([src_dir.to_str().unwrap(), dst_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!src_dir.exists());
    assert!(dst_dir.join("inner.txt").exists());
    assert_eq!(
        fs::read_to_string(dst_dir.join("inner.txt")).unwrap(),
        "inside"
    );
}
