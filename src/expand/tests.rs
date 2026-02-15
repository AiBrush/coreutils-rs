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
}
