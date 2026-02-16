use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fusers");
    path
}

fn cmd() -> Command {
    Command::new(bin_path())
}

#[test]
fn test_users_runs() {
    let output = cmd().output().unwrap();
    assert!(
        output.status.success(),
        "fusers should exit with code 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_users_format() {
    let output = cmd().output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Output should be at most one line (may be empty if no users logged in)
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines.len() <= 1,
        "users output should be a single line, got {} lines",
        lines.len()
    );
    // If there is output, names should be space-separated (no tabs, no commas)
    if let Some(line) = lines.first() {
        assert!(
            !line.contains('\t'),
            "users output should not contain tabs"
        );
        assert!(
            !line.contains(','),
            "users output should not contain commas"
        );
    }
}

#[test]
fn test_users_matches_gnu() {
    let gnu = Command::new("users").output();
    if let Ok(gnu) = gnu {
        let ours = cmd().output().unwrap();
        assert_eq!(
            ours.status.code(),
            gnu.status.code(),
            "Exit code mismatch: ours={:?} gnu={:?}",
            ours.status.code(),
            gnu.status.code()
        );
        // Both should produce the same user list
        let gnu_stdout = String::from_utf8_lossy(&gnu.stdout);
        let our_stdout = String::from_utf8_lossy(&ours.stdout);
        assert_eq!(
            our_stdout.trim(),
            gnu_stdout.trim(),
            "Output mismatch: ours='{}' gnu='{}'",
            our_stdout.trim(),
            gnu_stdout.trim()
        );
    }
}

// ---- Unit tests for internal functions ----

use super::*;

#[test]
fn test_get_users_returns_vec() {
    // get_users should not panic and should return a valid vec
    let users = get_users();
    let _ = users.len();
}

#[test]
fn test_format_users_empty() {
    let users: Vec<String> = Vec::new();
    let result = format_users(&users);
    assert!(result.is_empty(), "format_users of empty vec should be empty");
}

#[test]
fn test_format_users_single() {
    let users = vec!["alice".to_string()];
    let result = format_users(&users);
    assert_eq!(result, "alice");
}

#[test]
fn test_format_users_multiple() {
    let users = vec!["alice".to_string(), "bob".to_string(), "charlie".to_string()];
    let result = format_users(&users);
    assert_eq!(result, "alice bob charlie");
}

#[test]
fn test_get_users_sorted() {
    let users = get_users();
    // Verify the list is sorted
    for i in 1..users.len() {
        assert!(
            users[i - 1] <= users[i],
            "Users should be sorted: '{}' should come before '{}'",
            users[i - 1],
            users[i]
        );
    }
}
