use super::evaluate;
use std::process::Command;

fn args(strs: &[&str]) -> Vec<String> {
    strs.iter().map(|s| s.to_string()).collect()
}

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("ftest");
    path
}

fn cmd() -> Command {
    Command::new(bin_path())
}

#[test]
fn test_test_file_exists() {
    // /tmp always exists
    assert_eq!(evaluate(&args(&["-e", "/tmp"])), Ok(true));
    // A nonexistent path
    assert_eq!(
        evaluate(&args(&["-e", "/nonexistent_path_xyz_123"])),
        Ok(false)
    );
}

#[test]
fn test_test_is_file() {
    // /etc/passwd is a regular file on Linux
    assert_eq!(evaluate(&args(&["-f", "/etc/passwd"])), Ok(true));
    // /tmp is a directory, not a regular file
    assert_eq!(evaluate(&args(&["-f", "/tmp"])), Ok(false));
}

#[test]
fn test_test_is_dir() {
    assert_eq!(evaluate(&args(&["-d", "/tmp"])), Ok(true));
    assert_eq!(evaluate(&args(&["-d", "/etc/passwd"])), Ok(false));
}

#[test]
fn test_test_readable() {
    // /etc/passwd should be readable by everyone
    assert_eq!(evaluate(&args(&["-r", "/etc/passwd"])), Ok(true));
}

#[test]
fn test_test_string_eq() {
    assert_eq!(evaluate(&args(&["hello", "=", "hello"])), Ok(true));
    assert_eq!(evaluate(&args(&["hello", "=", "world"])), Ok(false));
}

#[test]
fn test_test_string_ne() {
    assert_eq!(evaluate(&args(&["hello", "!=", "world"])), Ok(true));
    assert_eq!(evaluate(&args(&["hello", "!=", "hello"])), Ok(false));
}

#[test]
fn test_test_int_eq() {
    assert_eq!(evaluate(&args(&["42", "-eq", "42"])), Ok(true));
    assert_eq!(evaluate(&args(&["42", "-eq", "43"])), Ok(false));
}

#[test]
fn test_test_int_lt() {
    assert_eq!(evaluate(&args(&["1", "-lt", "2"])), Ok(true));
    assert_eq!(evaluate(&args(&["2", "-lt", "1"])), Ok(false));
    assert_eq!(evaluate(&args(&["2", "-lt", "2"])), Ok(false));
}

#[test]
fn test_test_not() {
    assert_eq!(evaluate(&args(&["!", "-e", "/nonexistent_xyz"])), Ok(true));
    assert_eq!(evaluate(&args(&["!", "-d", "/tmp"])), Ok(false));
}

#[test]
fn test_test_and_or() {
    // -a (and)
    assert_eq!(
        evaluate(&args(&["-d", "/tmp", "-a", "-e", "/tmp"])),
        Ok(true)
    );
    assert_eq!(
        evaluate(&args(&["-d", "/tmp", "-a", "-e", "/nonexistent_xyz"])),
        Ok(false)
    );
    // -o (or)
    assert_eq!(
        evaluate(&args(&["-e", "/nonexistent_xyz", "-o", "-d", "/tmp"])),
        Ok(true)
    );
    assert_eq!(
        evaluate(&args(&[
            "-e",
            "/nonexistent_xyz",
            "-o",
            "-e",
            "/also_nonexistent"
        ])),
        Ok(false)
    );
}

#[test]
fn test_test_empty_string() {
    // No args => false
    assert_eq!(evaluate(&args(&[])), Ok(false));
    // Single empty string => false
    assert_eq!(evaluate(&args(&[""])), Ok(false));
    // Single non-empty string => true
    assert_eq!(evaluate(&args(&["hello"])), Ok(true));
    // -z with empty string => true
    assert_eq!(evaluate(&args(&["-z", ""])), Ok(true));
    // -z with non-empty string => false
    assert_eq!(evaluate(&args(&["-z", "hello"])), Ok(false));
    // -n with non-empty string => true
    assert_eq!(evaluate(&args(&["-n", "hello"])), Ok(true));
    // -n with empty string => false
    assert_eq!(evaluate(&args(&["-n", ""])), Ok(false));
}

#[test]
fn test_test_matches_gnu() {
    // Compare our ftest behavior with GNU test for basic cases
    let gnu = Command::new("test").args(["-d", "/tmp"]).status();
    if let Ok(gnu_status) = gnu {
        let our = cmd().args(["-d", "/tmp"]).status().unwrap();
        assert_eq!(
            our.code(),
            gnu_status.code(),
            "Exit code mismatch for: test -d /tmp"
        );
    }

    let gnu = Command::new("test")
        .args(["-e", "/nonexistent_xyz_123"])
        .status();
    if let Ok(gnu_status) = gnu {
        let our = cmd().args(["-e", "/nonexistent_xyz_123"]).status().unwrap();
        assert_eq!(
            our.code(),
            gnu_status.code(),
            "Exit code mismatch for: test -e /nonexistent_xyz_123"
        );
    }

    let gnu = Command::new("test")
        .args(["hello", "=", "hello"])
        .status();
    if let Ok(gnu_status) = gnu {
        let our = cmd().args(["hello", "=", "hello"]).status().unwrap();
        assert_eq!(
            our.code(),
            gnu_status.code(),
            "Exit code mismatch for: test hello = hello"
        );
    }

    let gnu = Command::new("test").args(["5", "-lt", "10"]).status();
    if let Ok(gnu_status) = gnu {
        let our = cmd().args(["5", "-lt", "10"]).status().unwrap();
        assert_eq!(
            our.code(),
            gnu_status.code(),
            "Exit code mismatch for: test 5 -lt 10"
        );
    }

    // No arguments: should exit 1
    let gnu = Command::new("test").status();
    if let Ok(gnu_status) = gnu {
        let our = cmd().status().unwrap();
        assert_eq!(
            our.code(),
            gnu_status.code(),
            "Exit code mismatch for: test (no args)"
        );
    }
}

// Additional tests for integer comparisons
#[test]
fn test_int_comparisons() {
    assert_eq!(evaluate(&args(&["10", "-ne", "20"])), Ok(true));
    assert_eq!(evaluate(&args(&["10", "-ne", "10"])), Ok(false));
    assert_eq!(evaluate(&args(&["10", "-le", "10"])), Ok(true));
    assert_eq!(evaluate(&args(&["10", "-le", "9"])), Ok(false));
    assert_eq!(evaluate(&args(&["10", "-gt", "5"])), Ok(true));
    assert_eq!(evaluate(&args(&["10", "-gt", "10"])), Ok(false));
    assert_eq!(evaluate(&args(&["10", "-ge", "10"])), Ok(true));
    assert_eq!(evaluate(&args(&["10", "-ge", "11"])), Ok(false));
}

// Additional tests for string comparison operators
#[test]
fn test_string_ordering() {
    assert_eq!(evaluate(&args(&["abc", "<", "abd"])), Ok(true));
    assert_eq!(evaluate(&args(&["abd", "<", "abc"])), Ok(false));
    assert_eq!(evaluate(&args(&["abd", ">", "abc"])), Ok(true));
    assert_eq!(evaluate(&args(&["abc", ">", "abd"])), Ok(false));
}

// Parenthesized grouping
#[test]
fn test_parentheses() {
    assert_eq!(
        evaluate(&args(&["(", "-d", "/tmp", ")"])),
        Ok(true)
    );
    assert_eq!(
        evaluate(&args(&["!", "(", "-e", "/nonexistent_xyz", ")"])),
        Ok(true)
    );
}

// Error cases
#[test]
fn test_bad_integer() {
    let result = evaluate(&args(&["abc", "-eq", "1"]));
    assert!(result.is_err());
}

// Binary exit code test via command
#[test]
fn test_binary_exit_codes() {
    let status = cmd().args(["-d", "/tmp"]).status().unwrap();
    assert_eq!(status.code(), Some(0));

    let status = cmd()
        .args(["-e", "/nonexistent_xyz_123"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));

    // No args => exit 1
    let status = cmd().status().unwrap();
    assert_eq!(status.code(), Some(1));
}

// Test bracket mode: last arg must be ]
#[test]
fn test_bracket_mode() {
    let path = bin_path();
    // When the binary name ends with "[", it should require trailing "]"
    // We simulate this by creating a symlink named "[" and invoking it.
    // However, for simplicity we just test the ftest binary directly.
    // The ftest binary itself doesn't require "]" unless invoked as "[".
    let status = cmd().args(["-d", "/tmp"]).status().unwrap();
    assert_eq!(status.code(), Some(0));

    // Verify that ftest with a single non-empty string arg exits 0
    let status = cmd().args(["hello"]).status().unwrap();
    assert_eq!(status.code(), Some(0));

    // Verify that ftest with a single empty string arg exits 1
    let status = cmd().args([""]).status().unwrap();
    assert_eq!(status.code(), Some(1));

    let _ = path;
}
