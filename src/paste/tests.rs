use super::*;

fn paste_helper(files: &[&[u8]], config: &PasteConfig) -> Vec<u8> {
    let data_refs: Vec<&[u8]> = files.to_vec();
    let mut out = Vec::new();
    paste(&data_refs, config, &mut out).unwrap();
    out
}

fn default_config() -> PasteConfig {
    PasteConfig::default()
}

// --- Basic tests ---

#[test]
fn test_empty_input() {
    let result = paste_helper(&[b""], &default_config());
    assert_eq!(result, b"");
}

#[test]
fn test_single_file_single_line() {
    let result = paste_helper(&[b"hello\n"], &default_config());
    assert_eq!(result, b"hello\n");
}

#[test]
fn test_single_file_multiple_lines() {
    let result = paste_helper(&[b"line1\nline2\nline3\n"], &default_config());
    assert_eq!(result, b"line1\nline2\nline3\n");
}

#[test]
fn test_two_files_same_length() {
    let result = paste_helper(&[b"a\nb\nc\n", b"1\n2\n3\n"], &default_config());
    assert_eq!(result, b"a\t1\nb\t2\nc\t3\n");
}

#[test]
fn test_two_files_different_length() {
    let result = paste_helper(&[b"a\nb\n", b"1\n2\n3\n"], &default_config());
    assert_eq!(result, b"a\t1\nb\t2\n\t3\n");
}

#[test]
fn test_three_files() {
    let result = paste_helper(&[b"a\nb\n", b"1\n2\n", b"x\ny\n"], &default_config());
    assert_eq!(result, b"a\t1\tx\nb\t2\ty\n");
}

#[test]
fn test_shorter_first_file() {
    let result = paste_helper(&[b"a\n", b"1\n2\n3\n"], &default_config());
    assert_eq!(result, b"a\t1\n\t2\n\t3\n");
}

// --- Delimiter tests ---

#[test]
fn test_custom_delimiter() {
    let config = PasteConfig {
        delimiters: vec![b','],
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\n", b"1\n2\n"], &config);
    assert_eq!(result, b"a,1\nb,2\n");
}

#[test]
fn test_multi_delimiter_cycling() {
    let config = PasteConfig {
        delimiters: vec![b',', b':'],
        ..default_config()
    };
    let result = paste_helper(&[b"a\n", b"1\n", b"x\n"], &config);
    // delimiter between col0-col1 is ',', between col1-col2 is ':'
    assert_eq!(result, b"a,1:x\n");
}

#[test]
fn test_empty_delimiter() {
    let config = PasteConfig {
        delimiters: Vec::new(),
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\n", b"1\n2\n"], &config);
    assert_eq!(result, b"a1\nb2\n");
}

// --- Delimiter parsing ---

#[test]
fn test_parse_delimiters_tab() {
    assert_eq!(parse_delimiters("\\t"), vec![b'\t']);
}

#[test]
fn test_parse_delimiters_newline() {
    assert_eq!(parse_delimiters("\\n"), vec![b'\n']);
}

#[test]
fn test_parse_delimiters_backslash() {
    assert_eq!(parse_delimiters("\\\\"), vec![b'\\']);
}

#[test]
fn test_parse_delimiters_nul() {
    assert_eq!(parse_delimiters("\\0"), vec![0u8]);
}

#[test]
fn test_parse_delimiters_mixed() {
    assert_eq!(parse_delimiters(",:\\t"), vec![b',', b':', b'\t']);
}

#[test]
fn test_parse_delimiters_empty() {
    assert_eq!(parse_delimiters(""), Vec::<u8>::new());
}

#[test]
fn test_parse_delimiters_single_char() {
    assert_eq!(parse_delimiters(","), vec![b',']);
}

// --- Serial mode tests ---

#[test]
fn test_serial_single_file() {
    let config = PasteConfig {
        serial: true,
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\nc\n"], &config);
    assert_eq!(result, b"a\tb\tc\n");
}

#[test]
fn test_serial_two_files() {
    let config = PasteConfig {
        serial: true,
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\n", b"1\n2\n"], &config);
    assert_eq!(result, b"a\tb\n1\t2\n");
}

#[test]
fn test_serial_custom_delimiter() {
    let config = PasteConfig {
        serial: true,
        delimiters: vec![b','],
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\nc\n"], &config);
    assert_eq!(result, b"a,b,c\n");
}

#[test]
fn test_serial_empty_file() {
    let config = PasteConfig {
        serial: true,
        ..default_config()
    };
    let result = paste_helper(&[b""], &config);
    assert_eq!(result, b"\n");
}

#[test]
fn test_serial_delimiter_cycling() {
    let config = PasteConfig {
        serial: true,
        delimiters: vec![b',', b':'],
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\nc\nd\n"], &config);
    // delimiters cycle: , : , between 4 items
    assert_eq!(result, b"a,b:c,d\n");
}

// --- Zero-terminated tests ---

#[test]
fn test_zero_terminated() {
    let config = PasteConfig {
        zero_terminated: true,
        ..default_config()
    };
    let result = paste_helper(&[b"a\x00b\x00", b"1\x002\x00"], &config);
    assert_eq!(result, b"a\t1\x00b\t2\x00");
}

#[test]
fn test_zero_terminated_serial() {
    let config = PasteConfig {
        zero_terminated: true,
        serial: true,
        ..default_config()
    };
    let result = paste_helper(&[b"a\x00b\x00c\x00"], &config);
    assert_eq!(result, b"a\tb\tc\x00");
}

// --- No trailing terminator ---

#[test]
fn test_no_trailing_newline() {
    let result = paste_helper(&[b"a\nb", b"1\n2"], &default_config());
    assert_eq!(result, b"a\t1\nb\t2\n");
}

#[test]
fn test_single_line_no_newline() {
    let result = paste_helper(&[b"hello"], &default_config());
    assert_eq!(result, b"hello\n");
}

// --- Edge cases ---

#[test]
fn test_binary_data() {
    let data1: &[u8] = &[0xFF, 0xFE, b'\n', 0x00, 0x01, b'\n'];
    let data2: &[u8] = &[0xAA, 0xBB, b'\n', 0xCC, 0xDD, b'\n'];
    let result = paste_helper(&[data1, data2], &default_config());
    let expected: Vec<u8> = vec![
        0xFF, 0xFE, b'\t', 0xAA, 0xBB, b'\n', 0x00, 0x01, b'\t', 0xCC, 0xDD, b'\n',
    ];
    assert_eq!(result, expected);
}

#[test]
fn test_empty_lines() {
    let result = paste_helper(&[b"\n\n\n", b"a\nb\nc\n"], &default_config());
    assert_eq!(result, b"\ta\n\tb\n\tc\n");
}

#[test]
fn test_crlf_lines() {
    // paste treats \n as line terminator; \r is part of the line content
    let result = paste_helper(&[b"a\r\nb\r\n", b"1\n2\n"], &default_config());
    assert_eq!(result, b"a\r\t1\nb\r\t2\n");
}

#[test]
fn test_utf8_content() {
    let result = paste_helper(
        &["あ\nい\n".as_bytes(), "う\nえ\n".as_bytes()],
        &default_config(),
    );
    assert_eq!(std::str::from_utf8(&result).unwrap(), "あ\tう\nい\tえ\n");
}

#[test]
fn test_very_long_line() {
    let long_line = "x".repeat(100_000);
    let data = format!("{}\n", long_line);
    let result = paste_helper(&[data.as_bytes(), b"short\n"], &default_config());
    let expected = format!("{}\tshort\n", long_line);
    assert_eq!(result, expected.as_bytes());
}

#[test]
fn test_many_files() {
    let files: Vec<Vec<u8>> = (0..10).map(|i| format!("{}\n", i).into_bytes()).collect();
    let refs: Vec<&[u8]> = files.iter().map(|f| f.as_slice()).collect();
    let mut out = Vec::new();
    paste(&refs, &default_config(), &mut out).unwrap();
    assert_eq!(out, b"0\t1\t2\t3\t4\t5\t6\t7\t8\t9\n");
}

#[test]
fn test_all_empty_files() {
    let result = paste_helper(&[b"", b"", b""], &default_config());
    assert_eq!(result, b"");
}

#[test]
fn test_serial_delimiter_cycling_multifile() {
    let config = PasteConfig {
        serial: true,
        delimiters: vec![b',', b':'],
        ..default_config()
    };
    // First input: lines joined with cycling delimiters ,: -> a,b:c,d
    // Second input: lines joined with cycling delimiters ,: -> 1,2:3
    let result = paste_helper(&[b"a\nb\nc\nd\n", b"1\n2\n3\n"], &config);
    assert_eq!(result, b"a,b:c,d\n1,2:3\n");
}

#[test]
fn test_serial_empty_delimiter() {
    let config = PasteConfig {
        serial: true,
        delimiters: Vec::new(),
        ..default_config()
    };
    let result = paste_helper(&[b"a\nb\nc\n"], &config);
    assert_eq!(result, b"abc\n");
}

#[test]
fn test_single_file_no_newline() {
    let result = paste_helper(&[b"abc"], &default_config());
    assert_eq!(result, b"abc\n");
}

#[test]
fn test_delimiter_longer_than_files() {
    let config = PasteConfig {
        delimiters: vec![b',', b':', b';', b'!', b'+'],
        ..default_config()
    };
    // Only 2 files, so only the first delimiter ',' is used between columns
    let result = paste_helper(&[b"a\nb\n", b"1\n2\n"], &config);
    assert_eq!(result, b"a,1\nb,2\n");
}

// --- Integration tests ---

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

    fn run_fpaste(input: &[u8], args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("fpaste"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn fpaste");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (
            output.stdout,
            output.stderr,
            output.status.code().unwrap_or(1),
        )
    }

    #[test]
    fn test_stdin_single() {
        let (out, _, code) = run_fpaste(b"a\nb\nc\n", &[]);
        assert_eq!(code, 0);
        assert_eq!(out, b"a\nb\nc\n");
    }

    #[test]
    fn test_stdin_serial() {
        let (out, _, code) = run_fpaste(b"a\nb\nc\n", &["-s"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"a\tb\tc\n");
    }

    #[test]
    fn test_file_input() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, b"a\nb\n").unwrap();
        std::fs::write(&p2, b"1\n2\n").unwrap();
        let (out, _, code) = run_fpaste(b"", &[p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"a\t1\nb\t2\n");
    }

    #[test]
    fn test_delimiter_flag() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, b"a\nb\n").unwrap();
        std::fs::write(&p2, b"1\n2\n").unwrap();
        let (out, _, code) = run_fpaste(
            b"",
            &["-d", ",", p1.to_str().unwrap(), p2.to_str().unwrap()],
        );
        assert_eq!(code, 0);
        assert_eq!(out, b"a,1\nb,2\n");
    }

    #[test]
    fn test_serial_files() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, b"a\nb\nc\n").unwrap();
        std::fs::write(&p2, b"1\n2\n").unwrap();
        let (out, _, code) = run_fpaste(b"", &["-s", p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"a\tb\tc\n1\t2\n");
    }

    #[test]
    fn test_nonexistent_file() {
        let (_, _, code) = run_fpaste(b"", &["/tmp/nonexistent_fpaste_test_file"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_help() {
        let (out, _, code) = run_fpaste(b"", &["--help"]);
        assert_eq!(code, 0);
        assert!(!out.is_empty());
    }

    #[test]
    fn test_version() {
        let (out, _, code) = run_fpaste(b"", &["--version"]);
        assert_eq!(code, 0);
        assert!(!out.is_empty());
    }

    #[test]
    fn test_dash_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        std::fs::write(&p1, b"a\nb\n").unwrap();
        let (out, _, code) = run_fpaste(b"1\n2\n", &[p1.to_str().unwrap(), "-"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"a\t1\nb\t2\n");
    }

    #[test]
    fn test_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("large1.txt");
        let p2 = dir.path().join("large2.txt");
        let mut d1 = Vec::new();
        let mut d2 = Vec::new();
        for i in 0..10000 {
            d1.extend_from_slice(format!("file1_{}\n", i).as_bytes());
            d2.extend_from_slice(format!("file2_{}\n", i).as_bytes());
        }
        std::fs::write(&p1, &d1).unwrap();
        std::fs::write(&p2, &d2).unwrap();
        let (out, _, code) = run_fpaste(b"", &[p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);
        // Verify first line
        let first_line = out.split(|&b| b == b'\n').next().unwrap();
        assert_eq!(first_line, b"file1_0\tfile2_0");
        // Verify last data line
        let lines: Vec<&[u8]> = out.split(|&b| b == b'\n').collect();
        assert_eq!(lines[9999], b"file1_9999\tfile2_9999");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("gnu_a.txt");
        let p2 = dir.path().join("gnu_b.txt");
        std::fs::write(&p1, b"a\nb\nc\n").unwrap();
        std::fs::write(&p2, b"1\n2\n3\n").unwrap();

        let gnu_out = Command::new("paste")
            .arg(p1.to_str().unwrap())
            .arg(p2.to_str().unwrap())
            .output();

        let (our_out, _, code) = run_fpaste(b"", &[p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU paste");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_serial() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("gnu_a.txt");
        let p2 = dir.path().join("gnu_b.txt");
        std::fs::write(&p1, b"a\nb\nc\n").unwrap();
        std::fs::write(&p2, b"1\n2\n").unwrap();

        let gnu_out = Command::new("paste")
            .args(["-s", p1.to_str().unwrap(), p2.to_str().unwrap()])
            .output();

        let (our_out, _, code) =
            run_fpaste(b"", &["-s", p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Serial output differs from GNU paste");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_different_lengths() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("short.txt");
        let p2 = dir.path().join("long.txt");
        std::fs::write(&p1, b"a\n").unwrap();
        std::fs::write(&p2, b"1\n2\n3\n").unwrap();

        let gnu_out = Command::new("paste")
            .arg(p1.to_str().unwrap())
            .arg(p2.to_str().unwrap())
            .output();

        let (our_out, _, code) = run_fpaste(b"", &[p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Different-length output differs from GNU paste"
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_custom_delimiter() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        let p3 = dir.path().join("c.txt");
        std::fs::write(&p1, b"a\nb\n").unwrap();
        std::fs::write(&p2, b"1\n2\n").unwrap();
        std::fs::write(&p3, b"x\ny\n").unwrap();

        let gnu_out = Command::new("paste")
            .args([
                "-d",
                ",:",
                p1.to_str().unwrap(),
                p2.to_str().unwrap(),
                p3.to_str().unwrap(),
            ])
            .output();

        let (our_out, _, code) = run_fpaste(
            b"",
            &[
                "-d",
                ",:",
                p1.to_str().unwrap(),
                p2.to_str().unwrap(),
                p3.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Custom-delimiter output differs from GNU paste"
                );
            }
        }
    }

    #[test]
    fn test_zero_flag_integration() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("z1.txt");
        let p2 = dir.path().join("z2.txt");
        std::fs::write(&p1, b"a\x00b\x00").unwrap();
        std::fs::write(&p2, b"c\x00d\x00").unwrap();
        let (out, _, code) = run_fpaste(b"", &["-z", p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"a\tc\x00b\td\x00");
    }

    #[test]
    fn test_stdin_mixed_serial() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("mixed.txt");
        std::fs::write(&p1, b"x\ny\nz\n").unwrap();
        let (out, _, code) = run_fpaste(b"a\nb\nc\n", &["-s", "-", p1.to_str().unwrap()]);
        assert_eq!(code, 0);
        // Serial mode: stdin lines joined, then file lines joined
        assert_eq!(out, b"a\tb\tc\nx\ty\tz\n");
    }

    #[test]
    fn test_more_than_4_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in 0..6 {
            let p = dir.path().join(format!("f{}.txt", i));
            std::fs::write(&p, format!("{}\n", i)).unwrap();
            paths.push(p);
        }
        let args: Vec<&str> = paths.iter().map(|p| p.to_str().unwrap()).collect();
        let (out, _, code) = run_fpaste(b"", &args);
        assert_eq!(code, 0);
        assert_eq!(out, b"0\t1\t2\t3\t4\t5\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_zero_mode() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("gz1.txt");
        let p2 = dir.path().join("gz2.txt");
        std::fs::write(&p1, b"a\x00b\x00").unwrap();
        std::fs::write(&p2, b"c\x00d\x00").unwrap();

        let gnu_out = Command::new("paste")
            .args(["-z", p1.to_str().unwrap(), p2.to_str().unwrap()])
            .output();

        let (our_out, _, code) =
            run_fpaste(b"", &["-z", p1.to_str().unwrap(), p2.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Zero-mode output differs from GNU paste"
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_serial_cycling() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("cycle.txt");
        std::fs::write(&p1, b"a\nb\nc\nd\ne\n").unwrap();

        let gnu_out = Command::new("paste")
            .args(["-s", "-d", ",:", p1.to_str().unwrap()])
            .output();

        let (our_out, _, code) = run_fpaste(b"", &["-s", "-d", ",:", p1.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Serial cycling delimiter output differs from GNU paste"
                );
            }
        }
    }
}
