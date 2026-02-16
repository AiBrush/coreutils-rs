use super::*;

fn fold(input: &str, width: usize) -> String {
    let mut out = Vec::new();
    fold_bytes(input.as_bytes(), width, false, false, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn fold_b(input: &str, width: usize) -> String {
    let mut out = Vec::new();
    fold_bytes(input.as_bytes(), width, true, false, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn fold_s(input: &str, width: usize) -> String {
    let mut out = Vec::new();
    fold_bytes(input.as_bytes(), width, false, true, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn fold_bs(input: &str, width: usize) -> String {
    let mut out = Vec::new();
    fold_bytes(input.as_bytes(), width, true, true, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

// ===== Basic tests =====

#[test]
fn test_empty() {
    assert_eq!(fold("", 80), "");
}

#[test]
fn test_short_line() {
    assert_eq!(fold("hello\n", 80), "hello\n");
}

#[test]
fn test_exact_width() {
    assert_eq!(fold("12345\n", 5), "12345\n");
}

#[test]
fn test_one_over() {
    assert_eq!(fold("123456\n", 5), "12345\n6\n");
}

#[test]
fn test_double_width() {
    assert_eq!(fold("1234567890\n", 5), "12345\n67890\n");
}

#[test]
fn test_no_trailing_newline() {
    assert_eq!(fold("1234567890", 5), "12345\n67890");
}

#[test]
fn test_multiple_lines() {
    assert_eq!(fold("abc\ndef\n", 80), "abc\ndef\n");
}

#[test]
fn test_width_1() {
    assert_eq!(fold("abc\n", 1), "a\nb\nc\n");
}

#[test]
fn test_default_width_80() {
    let line = "a".repeat(100);
    let input = format!("{}\n", line);
    let result = fold(&input, 80);
    let lines: Vec<&str> = result.split('\n').collect();
    assert_eq!(lines[0].len(), 80);
    assert_eq!(lines[1].len(), 20);
}

// ===== -b (bytes) mode =====

#[test]
fn test_bytes_mode() {
    assert_eq!(fold_b("1234567890\n", 5), "12345\n67890\n");
}

#[test]
fn test_bytes_tab() {
    // In byte mode, tab is 1 byte
    assert_eq!(fold_b("\t12345678\n", 5), "\t1234\n5678\n");
}

// ===== -s (spaces) mode =====

#[test]
fn test_spaces_break() {
    // "world test" is exactly 10 chars, fits in width=10
    assert_eq!(fold_s("hello world test\n", 10), "hello \nworld test\n");
}

#[test]
fn test_spaces_no_space_in_range() {
    // No space within width: break at width anyway
    assert_eq!(fold_s("abcdefghijklmno\n", 10), "abcdefghij\nklmno\n");
}

#[test]
fn test_spaces_exact_fit() {
    assert_eq!(fold_s("hello worl\n", 10), "hello worl\n");
}

// ===== -b -s combined =====

#[test]
fn test_bytes_spaces() {
    // "world test" is exactly 10 bytes, fits in width=10
    assert_eq!(fold_bs("hello world test\n", 10), "hello \nworld test\n");
}

// ===== Tab handling (column mode) =====

#[test]
fn test_tab_column_mode() {
    // Tab at column 0 advances to column 8
    let result = fold("\t12345\n", 10);
    // Tab takes 8 columns, then 1 and 2 fit (cols 8,9), but 3 would be col 10 which = width
    assert_eq!(result, "\t12\n345\n");
}

// ===== Edge cases =====

#[test]
fn test_empty_lines() {
    assert_eq!(fold("\n\n\n", 5), "\n\n\n");
}

#[test]
fn test_single_char_lines() {
    assert_eq!(fold("a\nb\nc\n", 80), "a\nb\nc\n");
}

// ===== Additional edge case tests =====

#[test]
fn test_width_zero() {
    // Width 0: each byte in the input becomes a newline in the output.
    // Input "abc\n" is 4 bytes, output is 4 newlines.
    let mut out = Vec::new();
    fold_bytes(b"abc\n", 0, false, false, &mut out).unwrap();
    assert_eq!(out, b"\n\n\n\n");
}

#[test]
fn test_backspace_column_mode() {
    // In column mode (not byte mode), backspace (\x08) decrements the column counter.
    // Input "abcd\x08e\n" with width 4:
    //   a=col1, b=col2, c=col3, d=col4 (at width, but no break yet since nothing exceeds)
    //   \x08 is backspace: col decrements to 3
    //   e: col 3+1=4, which is not > 4, so no break
    // Output is unchanged: "abcd\x08e\n"
    let mut out = Vec::new();
    fold_bytes(b"abcd\x08e\n", 4, false, false, &mut out).unwrap();
    assert_eq!(out, b"abcd\x08e\n");
}

#[test]
fn test_multibyte_utf8_column() {
    // In column mode, each byte >= 0x20 (and != 0x7f) counts as 1 column.
    // é is encoded as 0xC3 0xA9 (2 bytes), each counting as 1 column.
    // Input "aéb\n" = bytes [0x61, 0xC3, 0xA9, 0x62, 0x0A] with width 3:
    //   a -> col=1, 0xC3 -> col=2, 0xA9 -> col=3
    //   b -> col+1=4 > 3, so break inserted before b
    // This matches GNU fold behavior (byte-by-byte column counting).
    let result = fold("a\u{e9}b\n", 3);
    assert_eq!(result, "a\u{e9}\nb\n");
}

#[test]
fn test_tab_at_exact_boundary() {
    // Tab at col 0 with width 8: tab advances to next 8-col stop.
    // char_width = 8, col + char_width = 0 + 8 = 8, which is NOT > 8.
    // So the tab fits exactly at the boundary without a break.
    let mut out = Vec::new();
    fold_bytes(b"\t\n", 8, false, false, &mut out).unwrap();
    assert_eq!(out, b"\t\n");
}

#[test]
fn test_s_trailing_spaces_at_width() {
    // Input "hello     world\n" with -s -w 10:
    // "hello     " is 10 chars (5 letters + 5 spaces), fits exactly in width.
    // Break occurs after the last space within width.
    assert_eq!(fold_s("hello     world\n", 10), "hello     \nworld\n");
}

#[test]
fn test_s_no_spaces_long_word() {
    // Input "abcdefghijklmno\n" with -s -w 5:
    // No spaces to break at, so fold must hard-break at width.
    assert_eq!(fold_s("abcdefghijklmno\n", 5), "abcde\nfghij\nklmno\n");
}

#[test]
fn test_s_space_at_position_zero() {
    // Input " abcd\n" with -s -w 5:
    // Space at start followed by 4 chars = 5 total columns, fits exactly.
    assert_eq!(fold_s(" abcd\n", 5), " abcd\n");
}

#[test]
fn test_crlf_line_endings() {
    // CR (\r = 0x0d) is a control char (< 0x20), so it has 0 display width.
    // Only LF (\n) is treated as a line terminator and resets the column.
    // Input "abc\r\ndef\r\n" with width 10: everything fits, output unchanged.
    let mut out = Vec::new();
    fold_bytes(b"abc\r\ndef\r\n", 10, false, false, &mut out).unwrap();
    assert_eq!(out, b"abc\r\ndef\r\n");
}

#[test]
fn test_very_long_line() {
    // Single line of 100,000 'a' characters with width 80.
    // 100000 / 80 = 1250 chunks of 80 chars each.
    // Each chunk gets a newline (1249 inserted + 1 original trailing = 1250 total).
    let input = "a".repeat(100_000) + "\n";
    let result = fold(&input, 80);
    let line_count = result.matches('\n').count();
    assert_eq!(line_count, 1250);
    // Verify each folded line is exactly 80 chars
    let lines: Vec<&str> = result.split('\n').collect();
    for i in 0..1250 {
        assert_eq!(lines[i].len(), 80, "Line {} should be 80 chars", i);
    }
}

#[test]
fn test_empty_input() {
    // Empty input should produce empty output.
    let mut out = Vec::new();
    fold_bytes(b"", 80, false, false, &mut out).unwrap();
    assert!(out.is_empty());
}

// ===== Integration tests =====

#[cfg(test)]
mod integration {
    use std::process::Command;

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

    fn run_ffold(input: &[u8], args: &[&str]) -> (Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("ffold"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn ffold");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (output.stdout, output.status.code().unwrap_or(1))
    }

    #[test]
    fn test_fold_stdin() {
        let input = "a".repeat(100) + "\n";
        let (out, code) = run_ffold(input.as_bytes(), &[]);
        assert_eq!(code, 0);
        let lines: Vec<&[u8]> = out.split(|&b| b == b'\n').collect();
        assert_eq!(lines[0].len(), 80);
        assert_eq!(lines[1].len(), 20);
    }

    #[test]
    fn test_fold_width() {
        let (out, code) = run_ffold(b"1234567890\n", &["-w", "5"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"12345\n67890\n");
    }

    #[test]
    fn test_fold_bytes_flag() {
        let (out, code) = run_ffold(b"1234567890\n", &["-b", "-w", "5"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"12345\n67890\n");
    }

    #[test]
    fn test_fold_spaces_flag() {
        let (out, code) = run_ffold(b"hello world test\n", &["-s", "-w", "10"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"hello \nworld test\n");
    }

    #[test]
    fn test_fold_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let data = "a".repeat(100) + "\n";
        std::fs::write(&path, &data).unwrap();
        let (out, code) = run_ffold(b"", &["-w", "50", path.to_str().unwrap()]);
        assert_eq!(code, 0);
        let lines: Vec<&[u8]> = out.split(|&b| b == b'\n').collect();
        assert_eq!(lines[0].len(), 50);
        assert_eq!(lines[1].len(), 50);
    }

    #[test]
    fn test_fold_help() {
        let (_, code) = run_ffold(b"", &["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_fold_version() {
        let (_, code) = run_ffold(b"", &["--version"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_fold_gnu_comparison() {
        let input = b"The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog.\n";
        let (our_out, code) = run_ffold(input, &["-w", "30"]);
        assert_eq!(code, 0);

        let gnu_out = Command::new("fold")
            .args(&["-w", "30"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(input).unwrap();
                child.wait_with_output()
            });

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU fold");
            }
        }
    }

    #[test]
    fn test_fold_gnu_comparison_bs() {
        let input = b"hello world this is a test of folding text at spaces\n";
        let (our_out, code) = run_ffold(input, &["-b", "-s", "-w", "15"]);
        assert_eq!(code, 0);

        let gnu_out = Command::new("fold")
            .args(&["-b", "-s", "-w", "15"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(input).unwrap();
                child.wait_with_output()
            });

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU fold -bs");
            }
        }
    }

    #[test]
    fn test_fold_nonexistent_file() {
        let (_, code) = run_ffold(b"", &["/tmp/nonexistent_ffold_test_file"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_fold_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let mut data = Vec::new();
        for i in 0..10000 {
            data.extend_from_slice(format!("line {:07}: The quick brown fox jumps over the lazy dog and keeps on running\n", i).as_bytes());
        }
        std::fs::write(&path, &data).unwrap();
        let (out, code) = run_ffold(b"", &["-w", "40", path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert!(!out.is_empty());
    }

    // ---- Additional integration tests ----

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("f1.txt");
        let f2 = dir.path().join("f2.txt");
        std::fs::write(&f1, "1234567890\n").unwrap();
        std::fs::write(&f2, "abcdefghij\n").unwrap();
        let (out, code) = run_ffold(
            b"",
            &["-w", "5", f1.to_str().unwrap(), f2.to_str().unwrap()],
        );
        assert_eq!(code, 0);
        assert_eq!(out, b"12345\n67890\nabcde\nfghij\n");
    }

    // ---- GNU compatibility tests ----

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_tabs_column() {
        // Write "a\tb\tc\n" to file and compare ffold -w 20 with GNU fold -w 20.
        // Tabs advance to next 8-column stop in column mode.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabs.txt");
        std::fs::write(&path, "a\tb\tc\n").unwrap();

        let our_output = Command::new(bin_path("ffold"))
            .arg("-w")
            .arg("20")
            .arg(path.to_str().unwrap())
            .output()
            .unwrap();

        let gnu_output = Command::new("fold")
            .arg("-w")
            .arg("20")
            .arg(path.to_str().unwrap())
            .output();

        if let Ok(gnu) = gnu_output {
            if gnu.status.success() {
                assert_eq!(
                    our_output.stdout, gnu.stdout,
                    "Output differs from GNU fold with tabs"
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_s_only() {
        // Write "hello world foo bar baz\n" to file and compare ffold -s -w 10 with GNU fold.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spaces.txt");
        std::fs::write(&path, "hello world foo bar baz\n").unwrap();

        let our_output = Command::new(bin_path("ffold"))
            .arg("-s")
            .arg("-w")
            .arg("10")
            .arg(path.to_str().unwrap())
            .output()
            .unwrap();

        let gnu_output = Command::new("fold")
            .arg("-s")
            .arg("-w")
            .arg("10")
            .arg(path.to_str().unwrap())
            .output();

        if let Ok(gnu) = gnu_output {
            if gnu.status.success() {
                assert_eq!(
                    our_output.stdout, gnu.stdout,
                    "Output differs from GNU fold -s"
                );
            }
        }
    }
}
