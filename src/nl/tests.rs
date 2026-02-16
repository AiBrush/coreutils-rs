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
    // Non-numbered non-blank: 7 spaces (width 6 + separator 1) then content
    let expected = b"     1\t# comment\n       not comment\n     2\t# another\n";
    assert_eq!(
        std::str::from_utf8(&result).unwrap(),
        std::str::from_utf8(expected).unwrap()
    );
}

// --- Edge cases ---

#[test]
fn test_no_trailing_newline() {
    // GNU nl always adds a trailing newline even when input doesn't have one
    let result = nl_helper(b"hello", &default_config());
    assert_eq!(result, b"     1\thello\n");
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

// --- Custom delimiter tests ---

#[test]
fn test_custom_delimiter_single_char() {
    // -d X: single char delimiter becomes "XX" for section matching
    // "XX" repeated twice = body delimiter "XXXX" won't match; "XX" alone = footer
    // For body delimiter: the delimiter is "X", and body = 2x delimiter = "XX"
    let config = NlConfig {
        section_delimiter: vec![b'X'],
        body_style: NumberingStyle::All,
        ..default_config()
    };
    // "XX" is the body delimiter (delimiter repeated 2x)
    let input = b"first\nXX\nsecond\n";
    let result = nl_helper(input, &config);
    let output = std::str::from_utf8(&result).unwrap();
    // "first" is numbered as line 1 in initial body section
    // "XX" is a body section delimiter -> outputs empty line, resets numbering
    // "second" is numbered as line 1 in new body section
    assert!(output.contains("first"));
    assert!(output.contains("second"));
    // Body delimiter line should produce a blank line
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], ""); // delimiter line becomes empty
    assert!(lines[2].contains("1")); // numbering resets
    assert!(lines[2].contains("second"));
}

#[test]
fn test_custom_delimiter_double_char() {
    // -d XY: delimiter is "XY", body delimiter = "XYXY"
    let config = NlConfig {
        section_delimiter: vec![b'X', b'Y'],
        body_style: NumberingStyle::All,
        ..default_config()
    };
    let input = b"first\nXYXY\nsecond\n";
    let result = nl_helper(input, &config);
    let output = std::str::from_utf8(&result).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3);
    // "first" numbered as 1
    assert!(lines[0].ends_with("first"));
    assert!(lines[0].contains("1"));
    // Delimiter line becomes empty
    assert_eq!(lines[1], "");
    // "second" numbered as 1 (reset)
    assert!(lines[2].ends_with("second"));
    assert!(lines[2].contains("1"));
}

#[test]
fn test_full_page_cycle() {
    // Full logical page: header (\:\:\:), body (\:\:), footer (\:)
    let input =
        b"\\:\\:\\:\nheader1\nheader2\n\\:\\:\nbody1\nbody2\nbody3\n\\:\nfooter1\nfooter2\n";
    let config = NlConfig {
        header_style: NumberingStyle::All,
        body_style: NumberingStyle::All,
        footer_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(input, &config);
    let output = std::str::from_utf8(&result).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    // Line 0: header delimiter -> empty line
    assert_eq!(lines[0], "");
    // Lines 1-2: header section numbered 1,2
    assert!(lines[1].contains("1") && lines[1].contains("header1"));
    assert!(lines[2].contains("2") && lines[2].contains("header2"));
    // Line 3: body delimiter -> empty line
    assert_eq!(lines[3], "");
    // Lines 4-6: body section numbered 1,2,3 (reset)
    assert!(lines[4].contains("1") && lines[4].contains("body1"));
    assert!(lines[5].contains("2") && lines[5].contains("body2"));
    assert!(lines[6].contains("3") && lines[6].contains("body3"));
    // Line 7: footer delimiter -> empty line
    assert_eq!(lines[7], "");
    // Lines 8-9: footer section numbered 1,2 (reset)
    assert!(lines[8].contains("1") && lines[8].contains("footer1"));
    assert!(lines[9].contains("2") && lines[9].contains("footer2"));
}

#[test]
fn test_negative_starting_number() {
    // -v -5: start numbering at -5
    let config = NlConfig {
        starting_line_number: -5,
        body_style: NumberingStyle::All,
        ..default_config()
    };
    let result = nl_helper(b"a\nb\nc\n", &config);
    let output = std::str::from_utf8(&result).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3);
    // Expect line numbers -5, -4, -3
    assert!(lines[0].contains("-5"));
    assert!(lines[0].ends_with("a"));
    assert!(lines[1].contains("-4"));
    assert!(lines[1].ends_with("b"));
    assert!(lines[2].contains("-3"));
    assert!(lines[2].ends_with("c"));
}

#[test]
fn test_wide_numbers_exceeding_width() {
    // -w 3: width is 3, but when numbers exceed 999 GNU nl expands rather than truncates
    let config = NlConfig {
        number_width: 3,
        body_style: NumberingStyle::All,
        ..default_config()
    };
    // Generate 1005 lines
    let mut input = Vec::new();
    for _ in 0..1005 {
        input.extend_from_slice(b"x\n");
    }
    let result = nl_helper(&input, &config);
    let output = std::str::from_utf8(&result).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 1005);
    // Line 1000 should have "1000" which is 4 digits, wider than width 3
    // GNU nl does NOT truncate; it expands
    assert!(lines[999].contains("1000"));
    assert!(lines[999].ends_with("x"));
    // Line 1 should be "  1\tx"
    assert_eq!(lines[0], "  1\tx");
    // Line 1005 should contain "1005"
    assert!(lines[1004].contains("1005"));
}

#[test]
fn test_tab_containing_lines() {
    // Lines containing tabs should not interfere with the number separator
    let config = default_config();
    let input = b"col1\tcol2\n\tanother\n";
    let result = nl_helper(input, &config);
    let output = std::str::from_utf8(&result).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    // First line: "     1\tcol1\tcol2"
    assert_eq!(lines[0], "     1\tcol1\tcol2");
    // Second line: "     2\t\tanother"
    assert_eq!(lines[1], "     2\t\tanother");
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
        // Default body style 't' doesn't number blank lines, but adds 7 spaces
        assert_eq!(out, b"     1\ta\n       \n     2\tb\n");
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
    fn test_no_trailing_newline_file() {
        // GNU nl always adds a trailing newline even when input doesn't have one
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no_newline.txt");
        std::fs::write(&path, b"hello").unwrap();
        let (out, _, code) = run_fnl(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1\thello\n");
    }

    #[test]
    fn test_no_trailing_newline_stdin() {
        // GNU nl always adds a trailing newline even when input doesn't have one
        let (out, _, code) = run_fnl(b"hello", &[]);
        assert_eq!(code, 0);
        assert_eq!(out, b"     1\thello\n");
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

    // --- Additional integration tests ---

    #[test]
    fn test_join_blank_lines_integration() {
        // -l 3 -ba: number all lines, but only number every 3rd consecutive blank
        let input = b"a\n\n\n\nb\n\n\n\nc\n";
        let (out, _, code) = run_fnl(input, &["-l", "3", "-b", "a"]);
        assert_eq!(code, 0);
        let output = std::str::from_utf8(&out).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        // "a" is numbered (1), then 3 blanks: first 2 not numbered, 3rd numbered (2)
        // "b" is numbered (3), then 3 blanks: first 2 not numbered, 3rd numbered (4)
        // "c" is numbered (5)
        assert!(lines[0].contains("a"));
        // The 3rd blank line should be numbered
        assert!(lines[3].trim().starts_with(|c: char| c.is_ascii_digit()));
        assert!(lines[4].contains("b"));
    }

    #[test]
    fn test_regex_body_integration() {
        // -b p^#: only number lines matching ^#
        let input = b"# comment\nnormal line\n# another\nplain\n";
        let (out, _, code) = run_fnl(input, &["-b", "p^#"]);
        assert_eq!(code, 0);
        let output = std::str::from_utf8(&out).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4);
        // Lines starting with # should be numbered
        assert!(lines[0].contains("1") && lines[0].contains("# comment"));
        // "normal line" should NOT be numbered (just spaces prefix)
        assert!(lines[1].trim_start().starts_with("normal line"));
        // "# another" should be numbered as 2
        assert!(lines[2].contains("2") && lines[2].contains("# another"));
        // "plain" should NOT be numbered
        assert!(lines[3].trim_start().starts_with("plain"));
    }

    #[test]
    fn test_no_renumber_integration() {
        // -p -h a: don't reset numbering at logical page boundaries, number header lines
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no_renumber.txt");
        std::fs::write(&path, b"a\nb\n\\:\\:\\:\nc\nd\n").unwrap();
        let (out, _, code) = run_fnl(b"", &["-p", "-h", "a", path.to_str().unwrap()]);
        assert_eq!(code, 0);
        let output = std::str::from_utf8(&out).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        // "a" -> 1, "b" -> 2, header delimiter -> empty, "c" -> 3, "d" -> 4
        // With -p, numbering should NOT reset at the header delimiter
        assert!(lines[0].contains("1"));
        assert!(lines[1].contains("2"));
        assert_eq!(lines[2], ""); // section delimiter
        assert!(lines[3].contains("3")); // continued numbering
        assert!(lines[4].contains("4"));
    }

    #[test]
    fn test_footer_section_integration() {
        // Test file containing \: footer delimiter
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("footer.txt");
        std::fs::write(&path, b"body1\nbody2\n\\:\nfooter1\nfooter2\n").unwrap();
        // Default footer style is 'n' (none), so footer lines should not be numbered
        let (out, _, code) = run_fnl(b"", &[path.to_str().unwrap()]);
        assert_eq!(code, 0);
        let output = std::str::from_utf8(&out).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 5);
        // body lines numbered
        assert!(lines[0].contains("1") && lines[0].contains("body1"));
        assert!(lines[1].contains("2") && lines[1].contains("body2"));
        // footer delimiter -> empty line
        assert_eq!(lines[2], "");
        // footer lines NOT numbered (default footer style 'n')
        assert!(lines[3].trim_start().starts_with("footer1"));
        assert!(lines[4].trim_start().starts_with("footer2"));
        // Verify footer lines don't have numbers
        assert!(
            !lines[3]
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit())
        );
        assert!(
            !lines[4]
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit())
        );
    }

    // --- GNU compatibility tests ---

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_section_delimiters() {
        // Write file with header, body, footer sections and compare with GNU nl
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_sections.txt");
        std::fs::write(
            &path,
            b"\\:\\:\\:\nheader line\n\\:\\:\nbody line\n\\:\nfooter line\n",
        )
        .unwrap();

        let gnu_out = Command::new("nl")
            .args(["-h", "a", "-f", "a", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-h", "a", "-f", "a", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Section delimiter output differs from GNU nl"
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gnu_compat_width_overflow() {
        // Write 1100 lines, run nl -w 3 -ba, compare with GNU nl
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gnu_width_overflow.txt");
        let mut data = Vec::new();
        for i in 0..1100 {
            data.extend_from_slice(format!("line{}\n", i).as_bytes());
        }
        std::fs::write(&path, &data).unwrap();

        let gnu_out = Command::new("nl")
            .args(["-w", "3", "-b", "a", path.to_str().unwrap()])
            .output();
        let (our_out, _, code) = run_fnl(b"", &["-w", "3", "-b", "a", path.to_str().unwrap()]);
        assert_eq!(code, 0);

        if let Ok(gnu) = gnu_out {
            if gnu.status.success() {
                assert_eq!(
                    our_out, gnu.stdout,
                    "Width overflow output differs from GNU nl"
                );
            }
        }
    }
}
