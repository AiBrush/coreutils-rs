use super::*;

fn nl_helper(input: &[u8], config: &NlConfig) -> Vec<u8> {
    let mut out = Vec::new();
    nl(input, config, &mut out).unwrap();
    out
}

fn default_config() -> NlConfig {
    NlConfig::default()
}

// --- Basic tests ---

#[test]
fn test_empty() {
    let result = nl_helper(b"", &default_config());
    assert_eq!(result, b"");
}

#[test]
fn test_single_line() {
    let result = nl_helper(b"hello\n", &default_config());
    // Default: body style 't' (non-empty), width 6, separator TAB, right-justified
    assert_eq!(result, b"     1\thello\n");
}

#[test]
fn test_multiple_lines() {
    let result = nl_helper(b"a\nb\nc\n", &default_config());
    assert_eq!(result, b"     1\ta\n     2\tb\n     3\tc\n");
}

#[test]
fn test_empty_lines_not_numbered() {
    // Default body style 't' skips empty lines
    // Non-numbered lines get width + separator_len spaces (7 = 6+1)
    let result = nl_helper(b"a\n\nb\n", &default_config());
    assert_eq!(result, b"     1\ta\n       \n     2\tb\n");
}

#[test]
fn test_body_style_all() {
    let config = NlConfig {
        body_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(b"a\n\nb\n", &config);
    assert_eq!(result, b"     1\ta\n     2\t\n     3\tb\n");
}

#[test]
fn test_body_style_none() {
    let config = NlConfig {
        body_style: NumberingStyle::None,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    // Non-numbered lines: 7 spaces (width 6 + 1 for tab separator)
    assert_eq!(result, b"       a\n       b\n");
}

// --- Number format tests ---

#[test]
fn test_format_ln() {
    let config = NlConfig {
        number_format: NumberFormat::Ln,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    assert_eq!(result, b"1     \ta\n2     \tb\n");
}

#[test]
fn test_format_rn() {
    // Default is rn
    let result = nl_helper(b"a\n", &default_config());
    assert_eq!(result, b"     1\ta\n");
}

#[test]
fn test_format_rz() {
    let config = NlConfig {
        number_format: NumberFormat::Rz,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    assert_eq!(result, b"000001\ta\n000002\tb\n");
}

// --- Width tests ---

#[test]
fn test_custom_width() {
    let config = NlConfig {
        number_width: 3,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    assert_eq!(result, b"  1\ta\n  2\tb\n");
}

#[test]
fn test_width_1() {
    let config = NlConfig {
        number_width: 1,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    assert_eq!(result, b"1\ta\n2\tb\n");
}

// --- Separator tests ---

#[test]
fn test_custom_separator() {
    let config = NlConfig {
        number_separator: b": ".to_vec(),
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    assert_eq!(result, b"     1: a\n     2: b\n");
}

// --- Increment tests ---

#[test]
fn test_increment() {
    let config = NlConfig {
        line_increment: 5,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\nc\n", &config);
    assert_eq!(result, b"     1\ta\n     6\tb\n    11\tc\n");
}

// --- Starting number tests ---

#[test]
fn test_starting_number() {
    let config = NlConfig {
        starting_line_number: 10,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\n", &config);
    assert_eq!(result, b"    10\ta\n    11\tb\n");
}

#[test]
fn test_starting_number_with_increment() {
    let config = NlConfig {
        starting_line_number: 100,
        line_increment: 10,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\nc\n", &config);
    assert_eq!(result, b"   100\ta\n   110\tb\n   120\tc\n");
}

// --- Section delimiter tests ---

#[test]
fn test_section_header() {
    // \:\:\: is header section delimiter
    let input = b"\\:\\:\\:\nheader line\n\\:\\:\nbody line\n";
    let config = NlConfig {
        header_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(input, &config);
    // Section delimiter lines produce empty lines in output
    // Header line numbered with header style, body line with body style
    let expected = "\n     1\theader line\n\n     1\tbody line\n";
    assert_eq!(std::str::from_utf8(&result).unwrap(), expected);
}

#[test]
fn test_section_resets_numbering() {
    let input = b"a\nb\n\\:\\:\\:\nc\nd\n";
    let config = NlConfig {
        header_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(input, &config);
    // After header section delimiter, line numbers reset to 1
    let expected = b"     1\ta\n     2\tb\n\n     1\tc\n     2\td\n";
    assert_eq!(
        std::str::from_utf8(&result).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
}

#[test]
fn test_no_renumber() {
    let input = b"a\nb\n\\:\\:\\:\nc\nd\n";
    let config = NlConfig {
        header_style: NumberingStyle::All,
        no_renumber: true,
        ..default_config()
    };
    let result = nl_helper(input, &config);
    // -p: don't reset numbering at sections
    let expected = b"     1\ta\n     2\tb\n\n     3\tc\n     4\td\n";
    assert_eq!(
        std::str::from_utf8(&result).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
}

// --- Join blank lines tests ---

#[test]
fn test_join_blank_lines() {
    let config = NlConfig {
        body_style: NumberingStyle::All,
        join_blank_lines: 3,
        ..default_config()
    };
    let input = b"a\n\n\n\nb\n";
    let result = nl_helper(input, &config);
    // With -l 3 and -b a: first 2 blanks get spaces (not numbered), 3rd blank is numbered
    let expected = b"     1\ta\n       \n       \n     2\t\n     3\tb\n";
    assert_eq!(
        std::str::from_utf8(&result).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
}

// --- Regex style tests ---

#[test]
fn test_regex_style() {
    let config = NlConfig {
        body_style: NumberingStyle::Regex(regex::Regex::new("^#").unwrap()),
        ..default_config()
    };
    let input = b"# comment\nnot comment\n# another\n";
    let result = nl_helper(input, &config);
    let expected = b"     1\t# comment\n      \tnot comment\n     2\t# another\n";
    assert_eq!(
        std::str::from_utf8(&result).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
}

// --- Edge cases ---

#[test]
fn test_no_trailing_newline() {
    let result = nl_helper(b"hello", &default_config());
    assert_eq!(result, b"     1\thello");
}

#[test]
fn test_only_newlines() {
    // Default body style 't' doesn't number empty lines
    // Non-numbered lines get width + separator_len spaces (7 total)
    let result = nl_helper(b"\n\n\n", &default_config());
    assert_eq!(result, b"       \n       \n       \n");
}

#[test]
fn test_all_empty_lines_numbered() {
    let config = NlConfig {
        body_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(b"\n\n", &config);
    assert_eq!(result, b"     1\t\n     2\t\n");
}

#[test]
fn test_single_newline() {
    // Non-numbered blank: 7 spaces (width 6 + 1 for separator)
    let result = nl_helper(b"\n", &default_config());
    assert_eq!(result, b"       \n");
}

#[test]
fn test_utf8_content() {
    let result = nl_helper("あいう\n".as_bytes(), &default_config());
    assert_eq!(std::str::from_utf8(&result).unwrap(), "     1\tあいう\n");
}

#[test]
fn test_long_lines() {
    let long = "x".repeat(10000);
    let input = format!("{}\n", long);
    let result = nl_helper(input.as_bytes(), &default_config());
    let expected = format!("     1\t{}\n", long);
    assert_eq!(result, expected.as_bytes());
}

#[test]
fn test_many_lines() {
    let mut input = Vec::new();
    for i in 0..100 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = nl_helper(&input, &default_config());
    // Verify first line
    assert!(result.starts_with(b"     1\tline 0\n"));
    // Verify line 100
    let lines: Vec<&[u8]> = result.split(|&b| b == b'\n').collect();
    assert_eq!(lines[99], b"   100\tline 99");
}

#[test]
fn test_footer_section() {
    let input = b"body\n\\:\nfooter\n";
    let config = NlConfig {
        footer_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(input, &config);
    let expected = b"     1\tbody\n\n     1\tfooter\n";
    assert_eq!(
        std::str::from_utf8(&result).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
}

// --- Numbering style parsing ---

#[test]
fn test_parse_style_a() {
    let style = parse_numbering_style("a").unwrap();
    assert!(matches!(style, NumberingStyle::All));
}

#[test]
fn test_parse_style_t() {
    let style = parse_numbering_style("t").unwrap();
    assert!(matches!(style, NumberingStyle::NonEmpty));
}

#[test]
fn test_parse_style_n() {
    let style = parse_numbering_style("n").unwrap();
    assert!(matches!(style, NumberingStyle::None));
}

#[test]
fn test_parse_style_regex() {
    let style = parse_numbering_style("p^#").unwrap();
    assert!(matches!(style, NumberingStyle::Regex(_)));
}

#[test]
fn test_parse_style_invalid() {
    assert!(parse_numbering_style("x").is_err());
}

#[test]
fn test_parse_format() {
    assert_eq!(parse_number_format("ln").unwrap(), NumberFormat::Ln);
    assert_eq!(parse_number_format("rn").unwrap(), NumberFormat::Rn);
    assert_eq!(parse_number_format("rz").unwrap(), NumberFormat::Rz);
    assert!(parse_number_format("xx").is_err());
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

    fn run_fnl(input: &[u8], args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("fnl"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn fnl");
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
    fn test_stdin_basic() {
        let (out, _, code) = run_fnl(b"a\nb\nc\n", &[]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1\ta\n     2\tb\n     3\tc\n");
    }

    #[test]
    fn test_stdin_with_blanks() {
        let (out, _, code) = run_fnl(b"a\n\nb\n", &[]);
        assert_eq!(code, 0);
        // Default body style 't' doesn't number blank lines but adds padding
        assert_eq!(out, b"     1\ta\n      \t\n     2\tb\n");
    }

    #[test]
    fn test_body_all() {
        let (out, _, code) = run_fnl(b"a\n\nb\n", &["-b", "a"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1\ta\n     2\t\n     3\tb\n");
    }

    #[test]
    fn test_custom_width() {
        let (out, _, code) = run_fnl(b"a\n", &["-w", "3"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"  1\ta\n");
    }

    #[test]
    fn test_custom_separator() {
        let (out, _, code) = run_fnl(b"a\n", &["-s", ": "]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1: a\n");
    }

    #[test]
    fn test_format_rz() {
        let (out, _, code) = run_fnl(b"a\n", &["-n", "rz"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"000001\ta\n");
    }

    #[test]
    fn test_format_ln() {
        let (out, _, code) = run_fnl(b"a\n", &["-n", "ln"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"1     \ta\n");
    }

    #[test]
    fn test_starting_number() {
        let (out, _, code) = run_fnl(b"a\nb\n", &["-v", "10"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"    10\ta\n    11\tb\n");
    }

    #[test]
    fn test_increment() {
        let (out, _, code) = run_fnl(b"a\nb\nc\n", &["-i", "5"]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1\ta\n     6\tb\n    11\tc\n");
    }

    #[test]
    fn test_file_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();
        let (out, _, code) = run_fnl(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1\ta\n     2\tb\n     3\tc\n");
    }

    #[test]
    fn test_nonexistent_file() {
        let (_, _, code) = run_fnl(b"", &["/tmp/nonexistent_fnl_test_file"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn test_help() {
        let (out, _, code) = run_fnl(b"", &["--help"]);
        assert_eq!(code, 0);
        assert!(!out.is_empty());
    }

    #[test]
    fn test_version() {
        let (out, _, code) = run_fnl(b"", &["--version"]);
        assert_eq!(code, 0);
        assert!(!out.is_empty());
    }

    #[test]
    fn test_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let mut data = Vec::new();
        for i in 0..10000 {
            data.extend_from_slice(format!("line {:07}\n", i).as_bytes());
        }
        std::fs::write(&path, &data).unwrap();
        let (out, _, code) = run_fnl(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        // Verify first line
        assert!(out.starts_with(b"     1\tline 0000000\n"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_test.txt");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();

        let gnu_out = Command::new("nl").arg(path.to_str().unwrap()).output();
        let (our_out, _, code) = run_fnl(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Output differs from GNU nl");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_with_blanks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_blanks.txt");
        std::fs::write(&path, b"a\n\nb\n\n\nc\n").unwrap();

        let gnu_out = Command::new("nl").arg(path.to_str().unwrap()).output();
        let (our_out, _, code) = run_fnl(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "Blank line output differs from GNU nl");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_all.txt");
        std::fs::write(&path, b"a\n\nb\n").unwrap();

        let gnu_out = Command::new("nl")
            .args(["-b", "a", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-b", "a", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "-b a output differs from GNU nl");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_format_rz() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_rz.txt");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();

        let gnu_out = Command::new("nl")
            .args(["-n", "rz", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-n", "rz", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "-n rz output differs from GNU nl");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_format_ln() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_ln.txt");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();

        let gnu_out = Command::new("nl")
            .args(["-n", "ln", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-n", "ln", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(our_out, gnu.stdout, "-n ln output differs from GNU nl");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_custom_width_separator() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_ws.txt");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();

        let gnu_out = Command::new("nl")
            .args(["-w", "3", "-s", ": ", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-w", "3", "-s", ": ", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Custom width/separator output differs from GNU nl"
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_comparison_starting_increment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_vi.txt");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();

        let gnu_out = Command::new("nl")
            .args(["-v", "10", "-i", "5", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-v", "10", "-i", "5", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Starting/increment output differs from GNU nl"
                );
            }
        }
    }
}
