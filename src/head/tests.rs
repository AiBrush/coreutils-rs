use super::*;

// ---- Helper functions ----

fn run_head_lines(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    head_lines(input, n, b'\n', &mut out).unwrap();
    out
}

fn run_head_lines_from_end(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    head_lines_from_end(input, n, b'\n', &mut out).unwrap();
    out
}

fn run_head_bytes(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    head_bytes(input, n, &mut out).unwrap();
    out
}

fn run_head_bytes_from_end(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    head_bytes_from_end(input, n, &mut out).unwrap();
    out
}

/// Get the path to a built binary. Works in both lib tests and integration tests.
fn bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove 'deps'
    path.push(name);
    path
}

// ---- Empty/minimal input ----

#[test]
fn test_empty_input() {
    assert_eq!(run_head_lines(b"", 10), b"");
}

#[test]
fn test_single_byte() {
    assert_eq!(run_head_lines(b"x", 10), b"x");
}

#[test]
fn test_single_line_with_newline() {
    assert_eq!(run_head_lines(b"hello\n", 10), b"hello\n");
}

#[test]
fn test_single_line_no_newline() {
    assert_eq!(run_head_lines(b"hello", 10), b"hello");
}

// ---- head -n N (positive lines) ----

#[test]
fn test_lines_default_10() {
    let mut input = Vec::new();
    for i in 1..=20 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_head_lines(&input, 10);
    let mut expected = Vec::new();
    for i in 1..=10 {
        expected.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    assert_eq!(result, expected);
}

#[test]
fn test_lines_fewer_than_n() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_head_lines(input, 10), input.as_slice());
}

#[test]
fn test_lines_exact_n() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_head_lines(input, 3), input.as_slice());
}

#[test]
fn test_lines_zero() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_head_lines(input, 0), b"");
}

#[test]
fn test_lines_one() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_head_lines(input, 1), b"one\n");
}

// ---- head -n -N (all but last N lines) ----

#[test]
fn test_lines_from_end_basic() {
    let input = b"one\ntwo\nthree\nfour\n";
    assert_eq!(run_head_lines_from_end(input, 2), b"one\ntwo\n");
}

#[test]
fn test_lines_from_end_all() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_head_lines_from_end(input, 3), b"");
}

#[test]
fn test_lines_from_end_more_than_total() {
    let input = b"one\ntwo\n";
    assert_eq!(run_head_lines_from_end(input, 10), b"");
}

#[test]
fn test_lines_from_end_zero() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_head_lines_from_end(input, 0), input.as_slice());
}

// ---- head -c N (positive bytes) ----

#[test]
fn test_bytes_basic() {
    let input = b"hello world";
    assert_eq!(run_head_bytes(input, 5), b"hello");
}

#[test]
fn test_bytes_more_than_file() {
    let input = b"hello";
    assert_eq!(run_head_bytes(input, 100), b"hello");
}

#[test]
fn test_bytes_zero() {
    let input = b"hello";
    assert_eq!(run_head_bytes(input, 0), b"");
}

#[test]
fn test_bytes_one() {
    let input = b"hello";
    assert_eq!(run_head_bytes(input, 1), b"h");
}

// ---- head -c -N (all but last N bytes) ----

#[test]
fn test_bytes_from_end_basic() {
    let input = b"hello world";
    assert_eq!(run_head_bytes_from_end(input, 6), b"hello");
}

#[test]
fn test_bytes_from_end_all() {
    let input = b"hello";
    assert_eq!(run_head_bytes_from_end(input, 5), b"");
}

#[test]
fn test_bytes_from_end_more_than_file() {
    let input = b"hello";
    assert_eq!(run_head_bytes_from_end(input, 100), b"");
}

#[test]
fn test_bytes_from_end_zero() {
    let input = b"hello";
    assert_eq!(run_head_bytes_from_end(input, 0), b"hello");
}

// ---- Zero-terminated mode ----

#[test]
fn test_zero_terminated() {
    let input = b"one\0two\0three\0four\0";
    let mut out = Vec::new();
    head_lines(input, 2, b'\0', &mut out).unwrap();
    assert_eq!(out, b"one\0two\0");
}

// ---- parse_size ----

#[test]
fn test_parse_size_plain() {
    assert_eq!(parse_size("10").unwrap(), 10);
    assert_eq!(parse_size("0").unwrap(), 0);
    assert_eq!(parse_size("1").unwrap(), 1);
}

#[test]
fn test_parse_size_suffixes() {
    assert_eq!(parse_size("1b").unwrap(), 512);
    assert_eq!(parse_size("1K").unwrap(), 1024);
    assert_eq!(parse_size("1kB").unwrap(), 1000);
    assert_eq!(parse_size("1M").unwrap(), 1048576);
    assert_eq!(parse_size("1MB").unwrap(), 1000000);
    assert_eq!(parse_size("1G").unwrap(), 1073741824);
}

#[test]
fn test_parse_size_invalid() {
    assert!(parse_size("abc").is_err());
    assert!(parse_size("").is_err());
    assert!(parse_size("1X").is_err());
}

// ---- Binary data ----

#[test]
fn test_binary_data_bytes() {
    let input: Vec<u8> = (0..=255).collect();
    assert_eq!(run_head_bytes(&input, 10), &input[..10]);
}

#[test]
fn test_binary_data_lines() {
    // Byte 10 (0x0A) is a newline, so first "line" in a 0-255 byte range
    // ends at position 10 (inclusive). head -n 1 returns bytes 0..=10.
    let mut input = Vec::new();
    for i in 0..=255u8 {
        input.push(i);
    }
    input.push(b'\n');
    input.extend_from_slice(b"second\n");
    let result = run_head_lines(&input, 1);
    assert_eq!(result.len(), 11); // bytes 0..=10 (newline at position 10)
}

// ---- Large input ----

#[test]
fn test_large_input_lines() {
    let mut input = Vec::new();
    for i in 0..100_000 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_head_lines(&input, 10);
    let mut expected = Vec::new();
    for i in 0..10 {
        expected.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    assert_eq!(result, expected);
}

#[test]
fn test_large_input_bytes() {
    let input = vec![b'A'; 1_000_000];
    assert_eq!(run_head_bytes(&input, 1024).len(), 1024);
}

// ---- Integration tests with binary ----

#[test]
fn test_binary_basic() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-n")
        .arg("3")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line 1\nline 2\nline 3\n"
    );
}

#[test]
fn test_binary_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-c")
        .arg("5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
}

#[test]
fn test_binary_multiple_files() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa\n").unwrap();
    std::fs::write(&file2, "bbb\n").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-n")
        .arg("1")
        .arg(file1.to_str().unwrap())
        .arg(file2.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("==> "));
    assert!(stdout.contains("aaa\n"));
    assert!(stdout.contains("bbb\n"));
}

#[test]
fn test_binary_quiet_mode() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa\n").unwrap();
    std::fs::write(&file2, "bbb\n").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-q")
        .arg("-n")
        .arg("1")
        .arg(file1.to_str().unwrap())
        .arg(file2.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "aaa\nbbb\n");
}

#[test]
fn test_binary_nonexistent_file() {
    let output = std::process::Command::new(bin_path("fhead"))
        .arg("/nonexistent/file")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("head:"));
    #[cfg(unix)]
    assert!(stderr.contains("No such file or directory"));
}

#[test]
fn test_binary_version() {
    let output = std::process::Command::new(bin_path("fhead"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("head (fcoreutils)"));
}

#[test]
fn test_binary_negative_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "1\n2\n3\n4\n5\n").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-n")
        .arg("-2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n2\n3\n");
}

#[test]
fn test_binary_negative_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-c")
        .arg("-6")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
}

// ---- GNU compatibility (Linux only — macOS/Windows have BSD utilities) ----

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_default() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    let mut content = String::new();
    for i in 1..=20 {
        content.push_str(&format!("line {}\n", i));
    }
    std::fs::write(&file_path, &content).unwrap();

    let our_output = std::process::Command::new(bin_path("fhead"))
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("head")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_bytes_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "a".repeat(2048)).unwrap();

    let our_output = std::process::Command::new(bin_path("fhead"))
        .arg("-c")
        .arg("1K")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("head")
        .arg("-c")
        .arg("1K")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

// ---- Additional edge case and GNU compat tests ----

#[test]
fn test_stdin_integration() {
    use std::io::Write;
    let mut child = std::process::Command::new(bin_path("fhead"))
        .arg("-n")
        .arg("3")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1\n2\n3\n4\n5\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n2\n3\n");
}

#[test]
fn test_verbose_flag_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-v")
        .arg("-n")
        .arg("1")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("==> "));
}

#[test]
fn test_long_form_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("--lines=3")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line 1\nline 2\nline 3\n"
    );
}

#[test]
fn test_long_form_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("--bytes=5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
}

#[test]
fn test_empty_file_integration() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("empty.txt");
    std::fs::write(&file_path, "").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"");
}

#[test]
fn test_file_with_only_newlines() {
    let mut out = Vec::new();
    head_lines(b"\n\n\n", 2, b'\n', &mut out).unwrap();
    assert_eq!(out, b"\n\n");
}

#[test]
fn test_multiple_files_bytes_mode() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "abcdef").unwrap();
    std::fs::write(&file2, "ghijkl").unwrap();

    let output = std::process::Command::new(bin_path("fhead"))
        .arg("-c")
        .arg("3")
        .arg(file1.to_str().unwrap())
        .arg(file2.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("==> "));
    assert!(stdout.contains("abc"));
    assert!(stdout.contains("ghi"));
}

#[test]
fn test_zero_terminated_with_n() {
    let mut out = Vec::new();
    head_lines(b"a\0b\0c\0d\0", 2, b'\0', &mut out).unwrap();
    assert_eq!(out, b"a\0b\0");
}

#[test]
fn test_no_trailing_newline_from_end() {
    // Input "one\ntwo\nthree" has 2 newlines, so 2 complete lines.
    // With n=1, we drop the last 1 complete line ("two\n") and the trailing
    // fragment ("three"), leaving only "one\n".
    let mut out = Vec::new();
    head_lines_from_end(b"one\ntwo\nthree", 1, b'\n', &mut out).unwrap();
    assert_eq!(out, b"one\n");
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_negative_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "1\n2\n3\n4\n5\n").unwrap();

    let our_output = std::process::Command::new(bin_path("fhead"))
        .arg("-n")
        .arg("-2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("head")
        .arg("-n")
        .arg("-2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_negative_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let our_output = std::process::Command::new(bin_path("fhead"))
        .arg("-c")
        .arg("-5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("head")
        .arg("-c")
        .arg("-5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_zero_terminated() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.bin");
    std::fs::write(&file_path, b"a\0b\0c\0d\0").unwrap();

    let our_output = std::process::Command::new(bin_path("fhead"))
        .arg("-z")
        .arg("-n")
        .arg("2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("head")
        .arg("-z")
        .arg("-n")
        .arg("2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}
