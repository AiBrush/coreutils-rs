use super::*;

// ---- Helper functions ----

fn run_tail_lines(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    tail_lines(input, n, b'\n', &mut out).unwrap();
    out
}

fn run_tail_lines_from(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    tail_lines_from(input, n, b'\n', &mut out).unwrap();
    out
}

fn run_tail_bytes(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    tail_bytes(input, n, &mut out).unwrap();
    out
}

fn run_tail_bytes_from(input: &[u8], n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    tail_bytes_from(input, n, &mut out).unwrap();
    out
}

fn bin_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push(name);
    path
}

// ---- Empty/minimal input ----

#[test]
fn test_empty_input() {
    assert_eq!(run_tail_lines(b"", 10), b"");
}

#[test]
fn test_single_byte() {
    assert_eq!(run_tail_lines(b"x", 10), b"x");
}

#[test]
fn test_single_line_with_newline() {
    assert_eq!(run_tail_lines(b"hello\n", 10), b"hello\n");
}

#[test]
fn test_single_line_no_newline() {
    assert_eq!(run_tail_lines(b"hello", 10), b"hello");
}

// ---- tail -n N (last N lines) ----

#[test]
fn test_lines_basic() {
    let input = b"one\ntwo\nthree\nfour\nfive\n";
    assert_eq!(run_tail_lines(input, 2), b"four\nfive\n");
}

#[test]
fn test_lines_default_10() {
    let mut input = Vec::new();
    for i in 1..=20 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_tail_lines(&input, 10);
    let mut expected = Vec::new();
    for i in 11..=20 {
        expected.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    assert_eq!(result, expected);
}

#[test]
fn test_lines_fewer_than_n() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_tail_lines(input, 10), input.as_slice());
}

#[test]
fn test_lines_exact_n() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_tail_lines(input, 3), input.as_slice());
}

#[test]
fn test_lines_zero() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_tail_lines(input, 0), b"");
}

#[test]
fn test_lines_one() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_tail_lines(input, 1), b"three\n");
}

// ---- tail -n +N (from line N onward) ----

#[test]
fn test_lines_from_basic() {
    let input = b"one\ntwo\nthree\nfour\nfive\n";
    assert_eq!(run_tail_lines_from(input, 3), b"three\nfour\nfive\n");
}

#[test]
fn test_lines_from_1() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_tail_lines_from(input, 1), input.as_slice());
}

#[test]
fn test_lines_from_beyond() {
    let input = b"one\ntwo\n";
    assert_eq!(run_tail_lines_from(input, 10), b"");
}

#[test]
fn test_lines_from_2() {
    let input = b"one\ntwo\nthree\n";
    assert_eq!(run_tail_lines_from(input, 2), b"two\nthree\n");
}

// ---- tail -c N (last N bytes) ----

#[test]
fn test_bytes_basic() {
    let input = b"hello world";
    assert_eq!(run_tail_bytes(input, 5), b"world");
}

#[test]
fn test_bytes_more_than_file() {
    let input = b"hello";
    assert_eq!(run_tail_bytes(input, 100), b"hello");
}

#[test]
fn test_bytes_zero() {
    let input = b"hello";
    assert_eq!(run_tail_bytes(input, 0), b"");
}

#[test]
fn test_bytes_one() {
    let input = b"hello";
    assert_eq!(run_tail_bytes(input, 1), b"o");
}

// ---- tail -c +N (from byte N onward) ----

#[test]
fn test_bytes_from_basic() {
    let input = b"hello world";
    assert_eq!(run_tail_bytes_from(input, 7), b"world");
}

#[test]
fn test_bytes_from_1() {
    let input = b"hello";
    assert_eq!(run_tail_bytes_from(input, 1), b"hello");
}

#[test]
fn test_bytes_from_beyond() {
    let input = b"hello";
    assert_eq!(run_tail_bytes_from(input, 100), b"");
}

// ---- Zero-terminated mode ----

#[test]
fn test_zero_terminated() {
    let input = b"one\0two\0three\0four\0";
    let mut out = Vec::new();
    tail_lines(input, 2, b'\0', &mut out).unwrap();
    assert_eq!(out, b"three\0four\0");
}

// ---- Binary data ----

#[test]
fn test_binary_data_bytes() {
    let input: Vec<u8> = (0..=255).collect();
    assert_eq!(run_tail_bytes(&input, 10), &input[246..]);
}

// ---- Large input ----

#[test]
fn test_large_input_lines() {
    let mut input = Vec::new();
    for i in 0..100_000 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_tail_lines(&input, 10);
    let mut expected = Vec::new();
    for i in 99_990..100_000 {
        expected.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    assert_eq!(result, expected);
}

// ---- Integration tests with binary ----

#[test]
fn test_binary_basic() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("3")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line 3\nline 4\nline 5\n"
    );
}

#[test]
fn test_binary_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-c")
        .arg("5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "world");
}

#[test]
fn test_binary_plus_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "1\n2\n3\n4\n5\n").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("+3")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n4\n5\n");
}

#[test]
fn test_binary_plus_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-c")
        .arg("+7")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "world");
}

#[test]
fn test_binary_multiple_files() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa\n").unwrap();
    std::fs::write(&file2, "bbb\n").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
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

    let output = std::process::Command::new(bin_path("ftail"))
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
    let output = std::process::Command::new(bin_path("ftail"))
        .arg("/nonexistent/file")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tail:"));
    #[cfg(unix)]
    assert!(stderr.contains("No such file or directory"));
}

#[test]
fn test_binary_version() {
    let output = std::process::Command::new(bin_path("ftail"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("tail (fcoreutils)"));
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

    let our_output = std::process::Command::new(bin_path("ftail"))
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("tail")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_plus_n() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    let mut content = String::new();
    for i in 1..=10 {
        content.push_str(&format!("line {}\n", i));
    }
    std::fs::write(&file_path, &content).unwrap();

    let our_output = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("+5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_output = std::process::Command::new("tail")
        .arg("-n")
        .arg("+5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert_eq!(our_output.stdout, gnu_output.stdout);
}

// ---- Additional edge case and GNU compat tests ----

#[test]
fn test_stdin_integration() {
    use std::io::Write;
    let mut child = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("3")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n4\n5\n");
}

#[test]
fn test_n_zero_integration() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("zero.txt");
    std::fs::write(&file_path, "1\n2\n3\n").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("0")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn test_verbose_flag_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("verbose.txt");
    std::fs::write(&file_path, "hello\nworld\n").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-v")
        .arg("-n")
        .arg("1")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("==> "),
        "expected verbose header in output: {}",
        stdout
    );
}

#[test]
fn test_empty_file_integration() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("empty.txt");
    std::fs::write(&file_path, "").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn test_no_trailing_newline_integration() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("notrail.txt");
    std::fs::write(&file_path, "one\ntwo\nthree").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-n")
        .arg("2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "two\nthree");
}

#[test]
fn test_only_newlines() {
    let result = run_tail_lines(b"\n\n\n", 2);
    assert_eq!(result, b"\n\n");
}

#[test]
fn test_multiple_files_bytes_mode() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("f1.txt");
    let file2 = dir.path().join("f2.txt");
    std::fs::write(&file1, "ABCDEF").unwrap();
    std::fs::write(&file2, "123456").unwrap();

    let output = std::process::Command::new(bin_path("ftail"))
        .arg("-c")
        .arg("3")
        .arg(file1.to_str().unwrap())
        .arg(file2.to_str().unwrap())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("==> "),
        "expected file headers for multiple files"
    );
    assert!(stdout.contains("DEF"), "expected last 3 bytes of file1");
    assert!(stdout.contains("456"), "expected last 3 bytes of file2");
}

#[test]
fn test_zero_terminated_plus_n() {
    let input = b"a\0b\0c\0d\0";
    let mut out = Vec::new();
    tail_lines_from(input, 2, b'\0', &mut out).unwrap();
    assert_eq!(out, b"b\0c\0d\0");
}

#[test]
fn test_lines_from_zero() {
    let input = b"a\nb\nc\n";
    let result = run_tail_lines_from(input, 0);
    assert_eq!(result, b"a\nb\nc\n");
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_c_n() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bytes.txt");
    std::fs::write(&file_path, "hello world!").unwrap();

    let our_output = std::process::Command::new(bin_path("ftail"))
        .arg("-c")
        .arg("5")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_out = std::process::Command::new("tail")
        .arg("-c")
        .arg("5")
        .arg(file_path.to_str().unwrap())
        .output();

    if let Ok(gnu) = gnu_out {
        if gnu.status.success() {
            assert_eq!(our_output.stdout, gnu.stdout);
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_c_plus_n() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bytes.txt");
    std::fs::write(&file_path, "hello world!").unwrap();

    let our_output = std::process::Command::new(bin_path("ftail"))
        .arg("-c")
        .arg("+7")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_out = std::process::Command::new("tail")
        .arg("-c")
        .arg("+7")
        .arg(file_path.to_str().unwrap())
        .output();

    if let Ok(gnu) = gnu_out {
        if gnu.status.success() {
            assert_eq!(our_output.stdout, gnu.stdout);
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_gnu_compat_zero_terminated() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("zero_term.bin");
    std::fs::write(&file_path, b"a\0b\0c\0d\0").unwrap();

    let our_output = std::process::Command::new(bin_path("ftail"))
        .arg("-z")
        .arg("-n")
        .arg("2")
        .arg(file_path.to_str().unwrap())
        .output()
        .unwrap();

    let gnu_out = std::process::Command::new("tail")
        .arg("-z")
        .arg("-n")
        .arg("2")
        .arg(file_path.to_str().unwrap())
        .output();

    if let Ok(gnu) = gnu_out {
        if gnu.status.success() {
            assert_eq!(our_output.stdout, gnu.stdout);
        }
    }
}
