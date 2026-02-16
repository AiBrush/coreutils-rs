use super::*;

fn run_tac(input: &[u8], sep: u8, before: bool) -> Vec<u8> {
    let mut out = Vec::new();
    tac_bytes(input, sep, before, &mut out).unwrap();
    out
}

fn run_tac_str(input: &[u8], sep: &[u8], before: bool) -> Vec<u8> {
    let mut out = Vec::new();
    tac_string_separator(input, sep, before, &mut out).unwrap();
    out
}

fn run_tac_regex(input: &[u8], pattern: &str, before: bool) -> Vec<u8> {
    let mut out = Vec::new();
    tac_regex_separator(input, pattern, before, &mut out).unwrap();
    out
}

// ---- Basic newline separator tests ----

#[test]
fn test_empty_input() {
    assert_eq!(run_tac(b"", b'\n', false), b"");
}

#[test]
fn test_single_line_with_newline() {
    assert_eq!(run_tac(b"hello\n", b'\n', false), b"hello\n");
}

#[test]
fn test_single_line_no_newline() {
    // No separator found in input — output as-is (consistent with tac_string_separator)
    assert_eq!(run_tac(b"hello", b'\n', false), b"hello");
}

#[test]
fn test_two_lines() {
    assert_eq!(run_tac(b"aaa\nbbb\n", b'\n', false), b"bbb\naaa\n");
}

#[test]
fn test_three_lines() {
    assert_eq!(
        run_tac(b"one\ntwo\nthree\n", b'\n', false),
        b"three\ntwo\none\n"
    );
}

#[test]
fn test_no_trailing_newline() {
    // GNU tac: trailing "bbb" stays bare (no separator added), then "aaa\n" = "bbbaaa\n"
    assert_eq!(run_tac(b"aaa\nbbb", b'\n', false), b"bbbaaa\n");
}

#[test]
fn test_empty_lines() {
    assert_eq!(run_tac(b"\n\n\n", b'\n', false), b"\n\n\n");
}

#[test]
fn test_mixed_empty_lines() {
    assert_eq!(run_tac(b"a\n\nb\n", b'\n', false), b"b\n\na\n");
}

#[test]
fn test_only_newline() {
    assert_eq!(run_tac(b"\n", b'\n', false), b"\n");
}

// ---- Before mode tests ----

#[test]
fn test_before_basic() {
    // With --before, separator attaches before the record
    // "aaa\nbbb\n" -> records are "aaa", "\nbbb", "\n"
    // reversed: "\n", "\nbbb", "aaa"
    assert_eq!(run_tac(b"aaa\nbbb\n", b'\n', true), b"\n\nbbbaaa");
}

#[test]
fn test_before_no_leading_sep() {
    assert_eq!(run_tac(b"aaa\nbbb", b'\n', true), b"\nbbbaaa");
}

// ---- Custom separator tests ----

#[test]
fn test_custom_separator_comma() {
    assert_eq!(run_tac(b"a,b,c,", b',', false), b"c,b,a,");
}

#[test]
fn test_custom_separator_no_trailing() {
    // GNU tac: trailing "c" stays bare (no separator added) = "cb,a,"
    assert_eq!(run_tac(b"a,b,c", b',', false), b"cb,a,");
}

// ---- Multi-byte string separator tests ----

#[test]
fn test_string_separator() {
    assert_eq!(run_tac_str(b"aXYbXYcXY", b"XY", false), b"cXYbXYaXY");
}

#[test]
fn test_string_separator_no_trailing() {
    // GNU tac: trailing "c" stays bare (no separator added) = "cbXYaXY"
    assert_eq!(run_tac_str(b"aXYbXYc", b"XY", false), b"cbXYaXY");
}

#[test]
fn test_string_separator_before() {
    assert_eq!(run_tac_str(b"aXYbXYc", b"XY", true), b"XYcXYba");
}

// ---- Regex separator tests ----

#[test]
fn test_regex_separator_digit() {
    // Separator is any digit — use [0-9] (POSIX ERE compatible, same as GNU tac)
    assert_eq!(run_tac_regex(b"a1b2c3", r"[0-9]", false), b"c3b2a1");
}

#[test]
fn test_regex_separator_newline() {
    assert_eq!(run_tac_regex(b"aaa\nbbb\n", r"\n", false), b"bbb\naaa\n");
}

// ---- Edge cases ----

#[test]
fn test_no_separator_found() {
    assert_eq!(run_tac(b"hello world", b',', false), b"hello world");
}

#[test]
fn test_only_separators() {
    assert_eq!(run_tac(b",,", b',', false), b",,");
}

#[test]
fn test_binary_data() {
    let data = b"\x00\x01\n\x02\x03\n";
    let result = run_tac(data, b'\n', false);
    assert_eq!(result, b"\x02\x03\n\x00\x01\n");
}

#[test]
fn test_large_input() {
    let mut input = Vec::new();
    for i in 0..10000 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_tac(&input, b'\n', false);
    // Check first and last lines of reversed output
    assert!(result.starts_with(b"line 9999\n"));
    assert!(result.ends_with(b"line 0\n"));
}

// ==================== Integration & GNU compatibility tests ====================

#[cfg(test)]
mod integration {
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Locate the compiled `ftac` binary under the target directory.
    fn bin_path(name: &str) -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("target");
        if cfg!(debug_assertions) {
            path.push("debug");
        } else {
            path.push("release");
        }
        path.push(name);
        path
    }

    /// Run the ftac binary with the given args, piping `input` to stdin.
    /// Returns (stdout, stderr, exit_code).
    fn run_ftac(input: &[u8], args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let ftac = bin_path("ftac");
        let mut child = Command::new(&ftac)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to spawn {:?}: {}", ftac, e));

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(input).expect("Failed to write to stdin");
        }
        // Drop stdin so the child sees EOF.
        drop(child.stdin.take());

        let output = child.wait_with_output().expect("Failed to wait on child");
        let code = output.status.code().unwrap_or(-1);
        (output.stdout, output.stderr, code)
    }

    // ---- Integration tests ----

    #[test]
    fn test_basic_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("input.txt");
        std::fs::write(&file, b"one\ntwo\nthree\n").unwrap();

        let (stdout, _stderr, code) = run_ftac(b"", &[file.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"three\ntwo\none\n");
    }

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = dir.path().join("f1.txt");
        let file2 = dir.path().join("f2.txt");
        std::fs::write(&file1, b"a\nb\n").unwrap();
        std::fs::write(&file2, b"c\nd\n").unwrap();

        let (stdout, _stderr, code) =
            run_ftac(b"", &[file1.to_str().unwrap(), file2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"b\na\nd\nc\n");
    }

    #[test]
    fn test_stdin() {
        let (stdout, _stderr, code) = run_ftac(b"one\ntwo\nthree\n", &[]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"three\ntwo\none\n");
    }

    #[test]
    fn test_before_flag() {
        let (stdout, _stderr, code) = run_ftac(b"aaa\nbbb\n", &["-b"]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"\n\nbbbaaa");
    }

    #[test]
    fn test_separator_flag() {
        let (stdout, _stderr, code) = run_ftac(b"aXYbXYcXY", &["-s", "XY"]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"cXYbXYaXY");
    }

    #[test]
    fn test_regex_flag() {
        let (stdout, _stderr, code) = run_ftac(b"a1b2c3", &["-r", "-s", "[0-9]"]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"c3b2a1");
    }

    #[test]
    fn test_combined_br() {
        let (stdout, _stderr, code) = run_ftac(b"aXYbXYc", &["-br", "-s", "XY"]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"XYcXYba");
    }

    #[test]
    fn test_help() {
        let (stdout, _stderr, code) = run_ftac(b"", &["--help"]);
        assert_eq!(code, 0);
        assert!(!stdout.is_empty(), "Expected non-empty help output");
    }

    #[test]
    fn test_version() {
        let (stdout, _stderr, code) = run_ftac(b"", &["--version"]);
        assert_eq!(code, 0);
        let out_str = String::from_utf8_lossy(&stdout);
        assert!(
            out_str.contains("tac"),
            "Expected version output to contain 'tac', got: {}",
            out_str
        );
    }

    #[test]
    fn test_nonexistent_file() {
        let (_stdout, stderr, code) = run_ftac(
            b"",
            &["/tmp/ftac_nonexistent_file_that_does_not_exist_12345"],
        );
        assert_eq!(code, 1);
        assert!(
            !stderr.is_empty(),
            "Expected error message on stderr for nonexistent file"
        );
    }

    #[test]
    fn test_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("empty.txt");
        std::fs::write(&file, b"").unwrap();

        let (stdout, _stderr, code) = run_ftac(b"", &[file.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"");
    }

    #[test]
    fn test_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("large.txt");
        let mut content = Vec::new();
        for i in 0..10_000 {
            content.extend_from_slice(format!("line {}\n", i).as_bytes());
        }
        std::fs::write(&file, &content).unwrap();

        let (stdout, _stderr, code) = run_ftac(b"", &[file.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert!(
            stdout.starts_with(b"line 9999\n"),
            "Expected output to start with 'line 9999\\n'"
        );
        assert!(
            stdout.ends_with(b"line 0\n"),
            "Expected output to end with 'line 0\\n'"
        );
    }

    #[test]
    fn test_binary_data() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        std::fs::write(&file, b"\x00\x01\n\x02\x03\n").unwrap();

        let (stdout, _stderr, code) = run_ftac(b"", &[file.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"\x02\x03\n\x00\x01\n");
    }

    #[test]
    fn test_no_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notrail.txt");
        std::fs::write(&file, b"aaa\nbbb").unwrap();

        let (stdout, _stderr, code) = run_ftac(b"", &[file.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(stdout, b"bbbaaa\n");
    }

    // ---- GNU compatibility tests (Linux only) ----

    #[cfg(target_os = "linux")]
    fn run_gnu_tac(input: &[u8], args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut child = Command::new("tac")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(input)?;
        }
        drop(child.stdin.take());
        child.wait_with_output()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_basic() {
        let input = b"one\ntwo\nthree\n";
        let (our_out, _stderr, code) = run_ftac(input, &[]);
        assert_eq!(code, 0);

        if let Ok(gnu) = run_gnu_tac(input, &[]) {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU tac");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_before() {
        let input = b"aaa\nbbb\n";
        let (our_out, _stderr, code) = run_ftac(input, &["-b"]);
        assert_eq!(code, 0);

        if let Ok(gnu) = run_gnu_tac(input, &["-b"]) {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU tac");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_separator() {
        let input = b"aXYbXYcXY";
        let (our_out, _stderr, code) = run_ftac(input, &["-s", "XY"]);
        assert_eq!(code, 0);

        if let Ok(gnu) = run_gnu_tac(input, &["-s", "XY"]) {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU tac");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_regex() {
        let input = b"a1b2c3";
        let (our_out, _stderr, code) = run_ftac(input, &["-r", "-s", "[0-9]"]);
        assert_eq!(code, 0);

        if let Ok(gnu) = run_gnu_tac(input, &["-r", "-s", "[0-9]"]) {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU tac");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_no_trailing_newline() {
        let input = b"aaa\nbbb";
        let (our_out, _stderr, code) = run_ftac(input, &[]);
        assert_eq!(code, 0);

        if let Ok(gnu) = run_gnu_tac(input, &[]) {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU tac");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_empty_lines() {
        let input = b"a\n\nb\n\nc\n";
        let (our_out, _stderr, code) = run_ftac(input, &[]);
        assert_eq!(code, 0);

        if let Ok(gnu) = run_gnu_tac(input, &[]) {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU tac");
            }
        }
    }
}
