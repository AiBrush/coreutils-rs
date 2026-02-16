use super::*;

fn expand(input: &str, tab_size: usize) -> String {
    let tabs = TabStops::Regular(tab_size);
    let mut out = Vec::new();
    expand_bytes(input.as_bytes(), &tabs, false, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn expand_initial(input: &str, tab_size: usize) -> String {
    let tabs = TabStops::Regular(tab_size);
    let mut out = Vec::new();
    expand_bytes(input.as_bytes(), &tabs, true, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn unexpand_str(input: &str, tab_size: usize, all: bool) -> String {
    let tabs = TabStops::Regular(tab_size);
    let mut out = Vec::new();
    unexpand_bytes(input.as_bytes(), &tabs, all, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

// ===== expand tests =====

#[test]
fn test_expand_empty() {
    assert_eq!(expand("", 8), "");
}

#[test]
fn test_expand_no_tabs() {
    assert_eq!(expand("hello world\n", 8), "hello world\n");
}

#[test]
fn test_expand_single_tab() {
    assert_eq!(expand("\thello\n", 8), "        hello\n");
}

#[test]
fn test_expand_tab_default() {
    // Tab at column 0 → 8 spaces
    assert_eq!(expand("\t", 8), "        ");
}

#[test]
fn test_expand_tab_after_chars() {
    // "abc" is 3 chars, tab fills to column 8 = 5 spaces
    assert_eq!(expand("abc\t", 8), "abc     ");
}

#[test]
fn test_expand_multiple_tabs() {
    assert_eq!(expand("\t\t", 8), "                ");
}

#[test]
fn test_expand_tab_size_4() {
    assert_eq!(expand("\thello\n", 4), "    hello\n");
    assert_eq!(expand("ab\tcd\n", 4), "ab  cd\n");
}

#[test]
fn test_expand_tab_size_1() {
    // Tab size 1: each tab = 1 space
    assert_eq!(expand("\thello\n", 1), " hello\n");
}

#[test]
fn test_expand_initial_only() {
    // -i flag: only expand leading tabs
    assert_eq!(
        expand_initial("\thello\tworld\n", 8),
        "        hello\tworld\n"
    );
}

#[test]
fn test_expand_initial_spaces_then_tab() {
    assert_eq!(
        expand_initial("  \thello\tworld\n", 8),
        "        hello\tworld\n"
    );
}

#[test]
fn test_expand_multiline() {
    let input = "\tline1\n\tline2\n";
    let expected = "        line1\n        line2\n";
    assert_eq!(expand(input, 8), expected);
}

#[test]
fn test_expand_backspace() {
    // Backspace decrements column
    let input = "ab\x08\t";
    let result = expand(input, 8);
    // "a" at col 0, "b" at col 1, backspace → col 1, tab fills to 8 → 7 spaces
    assert_eq!(result, "ab\x08       ");
}

#[test]
fn test_expand_tab_list() {
    let tabs = parse_tab_stops("4,8,12").unwrap();
    let mut out = Vec::new();
    expand_bytes(b"\ta\tb\tc\n", &tabs, false, &mut out).unwrap();
    let result = String::from_utf8(out).unwrap();
    // tab at 0 → 4 spaces, "a" at col 4, tab → col 8 (4 spaces), "b" at col 8, tab → col 12 (4 spaces)
    assert_eq!(result, "    a   b   c\n");
}

#[test]
fn test_expand_no_trailing_newline() {
    assert_eq!(expand("hello\tworld", 8), "hello   world");
}

// ===== unexpand tests =====

#[test]
fn test_unexpand_empty() {
    assert_eq!(unexpand_str("", 8, false), "");
}

#[test]
fn test_unexpand_no_spaces() {
    assert_eq!(unexpand_str("hello\n", 8, false), "hello\n");
}

#[test]
fn test_unexpand_leading_spaces() {
    // 8 spaces at start → 1 tab
    assert_eq!(unexpand_str("        hello\n", 8, false), "\thello\n");
}

#[test]
fn test_unexpand_partial_leading() {
    // 4 spaces (not enough for tab stop 8)
    assert_eq!(unexpand_str("    hello\n", 8, false), "    hello\n");
}

#[test]
fn test_unexpand_all() {
    // -a: convert all sequences of spaces
    assert_eq!(unexpand_str("hello   world\n", 4, true), "hello\tworld\n");
}

#[test]
fn test_unexpand_default_leading_only() {
    // Default: only convert leading spaces
    // "hello" + spaces → spaces stay as-is
    assert_eq!(
        unexpand_str("hello        world\n", 8, false),
        "hello        world\n"
    );
}

#[test]
fn test_unexpand_multiple_tabs() {
    assert_eq!(
        unexpand_str("                hello\n", 8, false),
        "\t\thello\n"
    );
}

#[test]
fn test_unexpand_tab_size_4() {
    assert_eq!(unexpand_str("    hello\n", 4, false), "\thello\n");
}

// ===== parse_tab_stops tests =====

#[test]
fn test_parse_single() {
    match parse_tab_stops("4").unwrap() {
        TabStops::Regular(4) => {}
        _ => panic!("expected Regular(4)"),
    }
}

#[test]
fn test_parse_list() {
    match parse_tab_stops("4,8,12").unwrap() {
        TabStops::List(v) => assert_eq!(v, vec![4, 8, 12]),
        _ => panic!("expected List"),
    }
}

#[test]
fn test_parse_zero() {
    assert!(parse_tab_stops("0").is_err());
}

#[test]
fn test_parse_descending() {
    assert!(parse_tab_stops("8,4").is_err());
}

#[test]
fn test_expand_tab_at_exact_boundary() {
    // Tab at position 8 with tab size 8.
    // "12345678" occupies columns 0-7, so the tab at column 8 is exactly on a stop.
    // It should advance to the next stop at column 16, producing 8 spaces.
    assert_eq!(expand("12345678\t", 8), "12345678        ");
}

#[test]
fn test_expand_single_tab_stop_list() {
    // A tab list with a single value "4" should behave as a regular interval of 4.
    let tabs = parse_tab_stops("4").unwrap();
    let mut out = Vec::new();
    expand_bytes(b"\thello\n", &tabs, false, &mut out).unwrap();
    let result = String::from_utf8(out).unwrap();
    assert_eq!(result, "    hello\n");
}

#[test]
fn test_unexpand_backspace_handling() {
    // Backspace in unexpand: column decrements, disrupting alignment.
    // Input: three spaces, backspace, three spaces at tabstop 4.
    // The backspace disrupts the column tracking so a clean tab replacement is not possible.
    let input = "   \x08   ";
    let tabs = TabStops::Regular(4);
    let mut out = Vec::new();
    unexpand_bytes(input.as_bytes(), &tabs, true, &mut out).unwrap();
    let result = String::from_utf8(out).unwrap();
    // The backspace disrupts alignment; verify the backspace is preserved in output.
    assert!(result.contains('\x08'));
}

#[test]
fn test_unexpand_all_custom_tab_list() {
    // unexpand with -a and custom tab list "4,8".
    // Input has spaces at positions matching both stops.
    // 4 spaces at start (cols 0-3 → tab stop at 4) + "ab" at cols 4-5 + 2 spaces (cols 6-7 → tab stop at 8) + "cd"
    let tabs = parse_tab_stops("4,8").unwrap();
    let input = "    ab  cd\n";
    let mut out = Vec::new();
    unexpand_bytes(input.as_bytes(), &tabs, true, &mut out).unwrap();
    let result = String::from_utf8(out).unwrap();
    assert_eq!(result, "\tab\tcd\n");
}

#[test]
fn test_expand_empty_file() {
    // Empty input returns empty output.
    assert_eq!(expand("", 8), "");
    assert_eq!(expand("", 4), "");
    assert_eq!(expand("", 1), "");
}

#[test]
fn test_expand_no_trailing_newline_simple() {
    // Input "a\tb" without trailing newline should still expand the tab correctly.
    // "a" at col 0, tab fills to col 8 → 7 spaces.
    assert_eq!(expand("a\tb", 8), "a       b");
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

    fn run_fexpand(input: &[u8], args: &[&str]) -> (Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("fexpand"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn fexpand");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (output.stdout, output.status.code().unwrap_or(1))
    }

    fn run_funexpand(input: &[u8], args: &[&str]) -> (Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("funexpand"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn funexpand");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (output.stdout, output.status.code().unwrap_or(1))
    }

    #[test]
    fn test_expand_stdin() {
        let (out, code) = run_fexpand(b"\thello\n", &[]);
        assert_eq!(code, 0);
        assert_eq!(out, b"        hello\n");
    }

    #[test]
    fn test_expand_tab_4() {
        let (out, code) = run_fexpand(b"\thello\n", &["-t", "4"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"    hello\n");
    }

    #[test]
    fn test_expand_initial() {
        let (out, code) = run_fexpand(b"\thello\tworld\n", &["-i"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"        hello\tworld\n");
    }

    #[test]
    fn test_expand_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"\thello\n").unwrap();
        let (out, code) = run_fexpand(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"        hello\n");
    }

    #[test]
    fn test_unexpand_stdin() {
        let (out, code) = run_funexpand(b"        hello\n", &[]);
        assert_eq!(code, 0);
        assert_eq!(out, b"\thello\n");
    }

    #[test]
    fn test_unexpand_all() {
        let (out, code) = run_funexpand(b"hello   world\n", &["-a", "-t", "4"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"hello\tworld\n");
    }

    #[test]
    fn test_expand_help() {
        let (_, code) = run_fexpand(b"", &["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_unexpand_help() {
        let (_, code) = run_funexpand(b"", &["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_expand_gnu_comparison() {
        let input = b"\t\thello\tworld\n  \ttab\n";
        let (our_out, code) = run_fexpand(input, &[]);
        assert_eq!(code, 0);

        let gnu_out = Command::new("expand")
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
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU expand");
            }
        }
    }

    #[test]
    fn test_unexpand_gnu_comparison() {
        let input = b"        hello\n                world\n";
        let (our_out, code) = run_funexpand(input, &[]);
        assert_eq!(code, 0);

        let gnu_out = Command::new("unexpand")
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
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU unexpand");
            }
        }
    }

    #[test]
    fn test_expand_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let mut data = Vec::new();
        for i in 0..10000 {
            data.extend_from_slice(format!("\t\tfield1\tfield2\t{}\n", i).as_bytes());
        }
        std::fs::write(&path, &data).unwrap();
        let (out, code) = run_fexpand(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert!(!out.is_empty());
        // Verify no tabs in output
        assert!(!out.contains(&b'\t'));
    }

    #[test]
    fn test_expand_nonexistent_file() {
        let (_, code) = run_fexpand(b"", &["/tmp/nonexistent_fexpand_test_file"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_expand_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");
        std::fs::write(&file1, b"\thello\n").unwrap();
        std::fs::write(&file2, b"\tworld\n").unwrap();
        let (out, code) = run_fexpand(b"", &[file1.to_str().unwrap(), file2.to_str().unwrap()]);
        assert_eq!(code, 0);
        let result = String::from_utf8(out).unwrap();
        assert!(result.contains("        hello"));
        assert!(result.contains("        world"));
        // No tabs should remain in the output
        assert!(!result.contains('\t'));
    }

    #[test]
    fn test_unexpand_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");
        std::fs::write(&file1, b"    hello\n").unwrap();
        std::fs::write(&file2, b"    world\n").unwrap();
        let (out, code) = run_funexpand(
            b"",
            &[
                "-a",
                "-t",
                "4",
                file1.to_str().unwrap(),
                file2.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0);
        let result = String::from_utf8(out).unwrap();
        assert!(result.contains("\thello"));
        assert!(result.contains("\tworld"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_expand_initial() {
        // Write file with leading tabs and mid-line tabs, compare expand -i with GNU expand
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("initial.txt");
        std::fs::write(&path, b"\t\thello\tworld\nfoo\tbar\n\tbaz\tqux\n").unwrap();
        let path_str = path.to_str().unwrap();

        // Run our fexpand -i
        let (our_out, code) = run_fexpand(b"", &["-i", path_str]);
        assert_eq!(code, 0);

        // Run GNU expand -i
        let gnu_result = Command::new("expand")
            .arg("-i")
            .arg(path_str)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();

        if let Ok(gnu) = gnu_result {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "fexpand -i output differs from GNU expand -i"
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_unexpand_a_t() {
        // Write file with spaces, compare unexpand -a -t 4 with GNU unexpand
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spaces.txt");
        std::fs::write(
            &path,
            b"    hello   world\n        indented\nno spaces here\n",
        )
        .unwrap();
        let path_str = path.to_str().unwrap();

        // Run our funexpand -a -t 4
        let (our_out, code) = run_funexpand(b"", &["-a", "-t", "4", path_str]);
        assert_eq!(code, 0);

        // Run GNU unexpand -a -t 4
        let gnu_result = Command::new("unexpand")
            .args(&["-a", "-t", "4"])
            .arg(path_str)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();

        if let Ok(gnu) = gnu_result {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "funexpand -a -t 4 output differs from GNU unexpand -a -t 4"
                );
            }
        }
    }
}
