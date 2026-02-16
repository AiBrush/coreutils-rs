use super::*;
use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fstdbuf");
    path
}

fn cmd() -> Command {
    Command::new(bin_path())
}

#[test]
fn test_stdbuf_runs_command() {
    let output = cmd()
        .args(["-o", "L", "echo", "hello"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fstdbuf should exit with code 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "hello");
}

#[test]
fn test_stdbuf_exit_code() {
    let output = cmd()
        .args(["-o", "0", "false"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "fstdbuf should propagate child exit code"
    );
}

#[test]
fn test_stdbuf_no_command() {
    let output = cmd().output().unwrap();
    assert!(
        !output.status.success(),
        "fstdbuf without a command should fail"
    );
}

#[test]
fn test_stdbuf_help() {
    let output = cmd().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("stdbuf"),
        "Help should mention stdbuf"
    );
}

#[test]
fn test_stdbuf_version() {
    let output = cmd().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("fcoreutils"),
        "Version should mention fcoreutils"
    );
}

#[test]
fn test_stdbuf_matches_gnu_args() {
    // Verify that the same flags are accepted
    let output = cmd()
        .args(["-i", "L", "-o", "0", "-e", "4096", "echo", "test"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fstdbuf should accept -i, -o, -e flags, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "test");
}

// ---- Unit tests for parse_buffer_mode ----

#[test]
fn test_parse_buffer_mode_line() {
    let mode = parse_buffer_mode("L").unwrap();
    assert!(matches!(mode, BufferMode::Line));
}

#[test]
fn test_parse_buffer_mode_line_lowercase() {
    let mode = parse_buffer_mode("l").unwrap();
    assert!(matches!(mode, BufferMode::Line));
}

#[test]
fn test_parse_buffer_mode_unbuffered() {
    let mode = parse_buffer_mode("0").unwrap();
    assert!(matches!(mode, BufferMode::Unbuffered));
}

#[test]
fn test_parse_buffer_mode_size() {
    let mode = parse_buffer_mode("4096").unwrap();
    match mode {
        BufferMode::Size(n) => assert_eq!(n, 4096),
        _ => panic!("expected Size mode"),
    }
}

#[test]
fn test_parse_buffer_mode_size_k() {
    let mode = parse_buffer_mode("4K").unwrap();
    match mode {
        BufferMode::Size(n) => assert_eq!(n, 4096),
        _ => panic!("expected Size mode"),
    }
}

#[test]
fn test_parse_buffer_mode_size_m() {
    let mode = parse_buffer_mode("1M").unwrap();
    match mode {
        BufferMode::Size(n) => assert_eq!(n, 1024 * 1024),
        _ => panic!("expected Size mode"),
    }
}

#[test]
fn test_parse_buffer_mode_invalid() {
    let result = parse_buffer_mode("abc");
    assert!(result.is_err());
}

#[test]
fn test_buffer_mode_to_env_value() {
    assert_eq!(BufferMode::Line.to_env_value(), "L");
    assert_eq!(BufferMode::Unbuffered.to_env_value(), "0");
    assert_eq!(BufferMode::Size(8192).to_env_value(), "8192");
}
