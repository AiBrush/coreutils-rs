use super::*;

fn rev_str(input: &str) -> String {
    let mut out = Vec::new();
    rev_bytes(input.as_bytes(), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn test_empty() {
    assert_eq!(rev_str(""), "");
}

#[test]
fn test_single_newline() {
    assert_eq!(rev_str("\n"), "\n");
}

#[test]
fn test_single_char() {
    assert_eq!(rev_str("a"), "a");
}

#[test]
fn test_single_char_with_newline() {
    assert_eq!(rev_str("a\n"), "a\n");
}

#[test]
fn test_simple_line() {
    assert_eq!(rev_str("hello\n"), "olleh\n");
}

#[test]
fn test_multiple_lines() {
    assert_eq!(rev_str("hello\nworld\n"), "olleh\ndlrow\n");
}

#[test]
fn test_no_trailing_newline() {
    assert_eq!(rev_str("hello"), "olleh");
}

#[test]
fn test_no_trailing_newline_multiline() {
    assert_eq!(rev_str("hello\nworld"), "olleh\ndlrow");
}

#[test]
fn test_empty_lines() {
    assert_eq!(rev_str("\n\n\n"), "\n\n\n");
}

#[test]
fn test_mixed_empty_lines() {
    assert_eq!(rev_str("abc\n\ndef\n"), "cba\n\nfed\n");
}

#[test]
fn test_utf8_multibyte() {
    // Japanese characters
    assert_eq!(rev_str("あいう\n"), "ういあ\n");
}

#[test]
fn test_utf8_emoji() {
    assert_eq!(rev_str("ab🎉cd\n"), "dc🎉ba\n");
}

#[test]
fn test_tabs_and_spaces() {
    assert_eq!(rev_str("\t abc \t\n"), "\t cba \t\n");
}

#[test]
fn test_long_line() {
    let long = "a".repeat(10000);
    let input = format!("{}\n", long);
    let expected = format!("{}\n", long); // all same char
    assert_eq!(rev_str(&input), expected);

    let mixed = "abcdefghij".repeat(1000);
    let input2 = format!("{}\n", mixed);
    let reversed: String = mixed.chars().rev().collect();
    let expected2 = format!("{}\n", reversed);
    assert_eq!(rev_str(&input2), expected2);
}

#[test]
fn test_binary_data() {
    let input: Vec<u8> = vec![0x00, 0x01, 0xFF, 0xFE, b'\n'];
    let mut out = Vec::new();
    rev_bytes(&input, &mut out).unwrap();
    assert_eq!(out, vec![0xFE, 0xFF, 0x01, 0x00, b'\n']);
}

#[test]
fn test_crlf() {
    // CR is NOT a line terminator for rev, only LF is
    assert_eq!(rev_str("abc\r\n"), "\rcba\n");
}

#[test]
fn test_gnu_compat_basic() {
    // Compare with expected GNU rev output
    let input = "Hello, World!\n";
    let expected = "!dlroW ,olleH\n";
    assert_eq!(rev_str(input), expected);
}

#[test]
fn test_gnu_compat_numbers() {
    assert_eq!(rev_str("12345\n"), "54321\n");
}

// Integration tests using the binary
#[cfg(test)]
mod integration {
    use std::process::Command;

    fn bin_path(name: &str) -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("target");
        // Detect if running in release or debug mode
        if cfg!(debug_assertions) {
            path.push("debug");
        } else {
            path.push("release");
        }
        path.push(name);
        path
    }

    fn run_frev(input: &[u8], args: &[&str]) -> (Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("frev"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn frev");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (output.stdout, output.status.code().unwrap_or(1))
    }

    #[test]
    fn test_stdin() {
        let (out, code) = run_frev(b"hello\nworld\n", &[]);
        assert_eq!(code, 0);
        assert_eq!(out, b"olleh\ndlrow\n");
    }

    #[test]
    fn test_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"hello\nworld\n").unwrap();
        let (out, code) = run_frev(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"olleh\ndlrow\n");
    }

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, b"abc\n").unwrap();
        std::fs::write(&p2, b"def\n").unwrap();
        let (out, code) = run_frev(b"", &[p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"cba\nfed\n");
    }

    #[test]
    fn test_nonexistent_file() {
        let (_, code) = run_frev(b"", &["/tmp/nonexistent_frev_test_file"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_dash_stdin() {
        let (out, code) = run_frev(b"hello\n", &["-"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"olleh\n");
    }

    #[test]
    fn test_help() {
        let (_, code) = run_frev(b"", &["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_version() {
        let (_, code) = run_frev(b"", &["--version"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let mut data = Vec::new();
        for i in 0..10000 {
            data.extend_from_slice(format!("line {:07}: abcdefghij\n", i).as_bytes());
        }
        std::fs::write(&path, &data).unwrap();
        let (out, code) = run_frev(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        // Verify first line
        let first_line = out.split(|&b| b == b'\n').next().unwrap();
        assert_eq!(first_line, b"jihgfedcba :0000000 enil");
    }

    #[test]
    fn test_gnu_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_test.txt");
        let data = "Hello, World!\n12345\nabc def ghi\n\ntest\n";
        std::fs::write(&path, data).unwrap();

        // Run GNU rev
        let gnu_out = Command::new("rev").arg(path.to_str().unwrap()).output();

        // Run our frev
        let (our_out, code) = run_frev(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU rev");
            }
        }
    }
}
