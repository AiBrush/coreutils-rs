use std::process::Command;

fn cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("fshred");
    Command::new(path)
}

#[test]
fn test_shred_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("overwrite.txt");
    let original = b"This is secret data that should be overwritten";
    std::fs::write(&file, original).unwrap();

    let output = cmd().arg(file.to_str().unwrap()).output().unwrap();
    assert!(output.status.success(), "shred failed: {:?}", output);

    // File should still exist (no -u flag)
    assert!(file.exists());

    // Content should be different from original
    let content = std::fs::read(&file).unwrap();
    assert_ne!(&content[..original.len()], original);
}

#[test]
fn test_shred_removes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("remove.txt");
    std::fs::write(&file, "secret data").unwrap();

    let output = cmd()
        .args(["-u", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "shred -u failed: {:?}", output);

    // File should be removed
    assert!(!file.exists());
}

#[test]
fn test_shred_zero_pass() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("zero.txt");
    std::fs::write(&file, "secret data!").unwrap();

    let output = cmd()
        .args(["-z", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "shred -z failed: {:?}", output);

    // File should still exist
    assert!(file.exists());

    // After zero pass, the file data within the original size should be all zeros
    // (if exact mode; otherwise it is rounded up)
    let content = std::fs::read(&file).unwrap();
    // The last pass was zeros, so the content should be all zeros
    assert!(
        content.iter().all(|&b| b == 0),
        "Expected all zeros after -z pass, got non-zero bytes"
    );
}

#[test]
fn test_shred_iterations() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("iters.txt");
    std::fs::write(&file, "some data here").unwrap();

    let output = cmd()
        .args(["-n", "5", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "shred -n 5 failed: {:?}",
        output
    );
    assert!(file.exists());
}

#[test]
fn test_shred_verbose() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("verbose.txt");
    std::fs::write(&file, "some data").unwrap();

    let output = cmd()
        .args(["-v", "-n", "2", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "shred -v failed: {:?}",
        output
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pass 1/2"),
        "Expected pass 1/2 in verbose output, got: {}",
        stderr
    );
    assert!(
        stderr.contains("pass 2/2"),
        "Expected pass 2/2 in verbose output, got: {}",
        stderr
    );
}

#[test]
fn test_shred_size() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sized.txt");
    std::fs::write(&file, "hello").unwrap();

    let output = cmd()
        .args(["-s", "1024", "-z", "-x", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "shred -s 1024 failed: {:?}",
        output
    );

    // File should have been written with the specified size
    let content = std::fs::read(&file).unwrap();
    assert_eq!(content.len(), 1024);
    // With -z, content should be all zeros
    assert!(content.iter().all(|&b| b == 0));
}

#[test]
fn test_shred_file_removed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("gone.txt");
    std::fs::write(&file, "will be removed").unwrap();
    assert!(file.exists());

    let output = cmd()
        .args(["-u", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!file.exists(), "File should have been removed with -u");
}

#[test]
fn test_shred_force() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("readonly.txt");
    std::fs::write(&file, "readonly data").unwrap();

    // Make the file read-only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o444);
        std::fs::set_permissions(&file, perms).unwrap();
    }

    let output = cmd()
        .args(["-f", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "shred -f failed on read-only file: {:?}",
        output
    );
}

#[test]
fn test_shred_help() {
    let output = cmd().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("shred"));
}

#[test]
fn test_shred_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("shred"));
    assert!(stdout.contains("fcoreutils"));
}

#[test]
fn test_shred_matches_gnu_behavior() {
    // Both should fail on nonexistent files
    let gnu = Command::new("shred").arg("/nonexistent_file_xyz").output();
    if let Ok(gnu) = gnu {
        let ours = cmd().arg("/nonexistent_file_xyz").output().unwrap();
        assert_eq!(
            ours.status.success(),
            gnu.status.success(),
            "Exit status mismatch with GNU shred on nonexistent file"
        );
    }
}

#[test]
fn test_shred_missing_file() {
    let output = cmd().output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing file operand") || stderr.contains("Usage"),
        "Expected usage error, got: {}",
        stderr
    );
}

#[test]
fn test_shred_exact() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("exact.txt");
    let data = b"hello world"; // 11 bytes, not block-aligned
    std::fs::write(&file, data).unwrap();

    let output = cmd()
        .args(["-x", "-z", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    // With -x, the file should be exactly the original size
    let content = std::fs::read(&file).unwrap();
    assert_eq!(content.len(), data.len());
}

// Unit tests for parse functions
use crate::shred::{parse_remove_mode, parse_size, RemoveMode, ShredConfig};

#[test]
fn test_parse_size_plain() {
    assert_eq!(parse_size("1024").unwrap(), 1024);
}

#[test]
fn test_parse_size_k() {
    assert_eq!(parse_size("1K").unwrap(), 1024);
}

#[test]
fn test_parse_size_m() {
    assert_eq!(parse_size("1M").unwrap(), 1_048_576);
}

#[test]
fn test_parse_size_g() {
    assert_eq!(parse_size("1G").unwrap(), 1_073_741_824);
}

#[test]
fn test_parse_size_kb() {
    assert_eq!(parse_size("1KB").unwrap(), 1_000);
}

#[test]
fn test_parse_size_invalid() {
    assert!(parse_size("abc").is_err());
    assert!(parse_size("").is_err());
}

#[test]
fn test_parse_remove_mode_u() {
    assert_eq!(parse_remove_mode("-u").unwrap(), RemoveMode::WipeSync);
}

#[test]
fn test_parse_remove_mode_unlink() {
    assert_eq!(
        parse_remove_mode("--remove=unlink").unwrap(),
        RemoveMode::Unlink
    );
}

#[test]
fn test_parse_remove_mode_wipe() {
    assert_eq!(
        parse_remove_mode("--remove=wipe").unwrap(),
        RemoveMode::Wipe
    );
}

#[test]
fn test_parse_remove_mode_wipesync() {
    assert_eq!(
        parse_remove_mode("--remove=wipesync").unwrap(),
        RemoveMode::WipeSync
    );
}

#[test]
fn test_parse_remove_mode_invalid() {
    assert!(parse_remove_mode("--remove=bogus").is_err());
}

#[test]
fn test_shred_config_default() {
    let config = ShredConfig::default();
    assert_eq!(config.iterations, 3);
    assert!(!config.zero_pass);
    assert!(config.remove.is_none());
    assert!(!config.force);
    assert!(!config.verbose);
    assert!(!config.exact);
    assert!(config.size.is_none());
}
