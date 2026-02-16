use super::*;
use std::io::BufReader;

/// Helper: run fmt with the given config on the input string and return the output.
fn run_fmt(input: &str, config: &FmtConfig) -> String {
    let reader = BufReader::new(input.as_bytes());
    let mut output = Vec::new();
    fmt_file(reader, &mut output, config).unwrap();
    String::from_utf8(output).unwrap()
}

/// Helper: run fmt with default config.
fn run_default(input: &str) -> String {
    run_fmt(input, &FmtConfig::default())
}

// ===== test_fmt_default_width =====

#[test]
fn test_fmt_default_width() {
    // Default width is 75. A line of 150 'a' chars should wrap.
    let long_line = "a ".repeat(50); // 50 words of "a", each 2 chars = 100 chars
    let result = run_default(&long_line);

    // Every output line should be at most 75 characters.
    for line in result.lines() {
        assert!(
            line.len() <= 75,
            "Line exceeds default width of 75: len={}, line={:?}",
            line.len(),
            line
        );
    }
    // Should have more than one line since input exceeds 75 chars.
    assert!(
        result.lines().count() > 1,
        "Expected multiple lines for long input"
    );
}

// ===== test_fmt_custom_width =====

#[test]
fn test_fmt_custom_width() {
    let config = FmtConfig {
        width: 40,
        goal: (40 * 93) / 100,
        ..FmtConfig::default()
    };

    let input = "The quick brown fox jumps over the lazy dog and then runs away quickly";
    let result = run_fmt(input, &config);

    for line in result.lines() {
        assert!(
            line.len() <= 40,
            "Line exceeds width of 40: len={}, line={:?}",
            line.len(),
            line
        );
    }
    assert!(
        result.lines().count() > 1,
        "Expected wrapping at width 40"
    );
}

// ===== test_fmt_split_only =====

#[test]
fn test_fmt_split_only() {
    // split-only mode: long lines are split but short lines are NOT joined.
    let config = FmtConfig {
        width: 40,
        goal: (40 * 93) / 100,
        split_only: true,
        ..FmtConfig::default()
    };

    let input = "short line\nanother short line\n";
    let result = run_fmt(input, &config);
    // Short lines should remain separate, not joined.
    assert_eq!(result, "short line\nanother short line\n");
}

#[test]
fn test_fmt_split_only_long_line() {
    let config = FmtConfig {
        width: 20,
        goal: (20 * 93) / 100,
        split_only: true,
        ..FmtConfig::default()
    };

    let input = "this is a rather long line that should be split at the width boundary\n";
    let result = run_fmt(input, &config);

    for line in result.lines() {
        assert!(
            line.len() <= 20,
            "Split-only: line exceeds width 20: len={}, line={:?}",
            line.len(),
            line
        );
    }
}

// ===== test_fmt_uniform_spacing =====

#[test]
fn test_fmt_uniform_spacing() {
    let config = FmtConfig {
        uniform_spacing: true,
        ..FmtConfig::default()
    };

    // Input has multiple spaces between words and one space after a sentence end.
    let input = "Hello   world.  This is   a   test.\n";
    let result = run_fmt(input, &config);

    // Uniform spacing: words separated by single space, but two spaces after sentence end.
    assert!(
        result.contains("world.  This"),
        "Expected two spaces after sentence-ending period, got: {:?}",
        result
    );
    // Multiple spaces between non-sentence words should be collapsed.
    assert!(
        !result.contains("   "),
        "Expected no triple spaces in output: {:?}",
        result
    );
}

// ===== test_fmt_prefix =====

#[test]
fn test_fmt_prefix() {
    let config = FmtConfig {
        width: 40,
        goal: (40 * 93) / 100,
        prefix: Some("# ".to_string()),
        ..FmtConfig::default()
    };

    let input = "# This is a comment line that is quite long and should be wrapped at width forty\n\
                 This is not a comment line\n";
    let result = run_fmt(input, &config);

    // The prefix line should be reformatted and wrapped.
    let prefix_lines: Vec<&str> = result.lines().filter(|l| l.starts_with("# ")).collect();
    assert!(
        prefix_lines.len() > 1,
        "Expected prefix lines to be wrapped"
    );
    for line in &prefix_lines {
        assert!(
            line.len() <= 40,
            "Prefix line exceeds width: {:?}",
            line
        );
    }

    // The non-prefix line should be preserved verbatim.
    assert!(
        result.contains("This is not a comment line"),
        "Non-prefix line should be preserved"
    );
}

// ===== test_fmt_preserves_paragraphs =====

#[test]
fn test_fmt_preserves_paragraphs() {
    let input = "First paragraph words here.\n\nSecond paragraph words here.\n";
    let result = run_default(input);

    // The blank line between paragraphs must be preserved.
    assert!(
        result.contains("\n\n"),
        "Blank line between paragraphs should be preserved, got: {:?}",
        result
    );

    let paragraphs: Vec<&str> = result.split("\n\n").collect();
    assert!(
        paragraphs.len() >= 2,
        "Expected at least two paragraphs separated by blank line"
    );
}

// ===== test_fmt_tagged =====

#[test]
fn test_fmt_tagged() {
    let config = FmtConfig {
        width: 40,
        goal: (40 * 93) / 100,
        tagged: true,
        ..FmtConfig::default()
    };

    // First line has different indent (4 spaces) from continuation (8 spaces).
    let input = "    First line of the tagged paragraph that is long enough to wrap.\n\
                 \x20       Continuation with deeper indent here.\n";
    let result = run_fmt(input, &config);

    let lines: Vec<&str> = result.lines().collect();
    assert!(
        !lines.is_empty(),
        "Expected output for tagged paragraph"
    );

    // First output line should start with 4-space indent.
    assert!(
        lines[0].starts_with("    "),
        "First line should preserve first-line indent: {:?}",
        lines[0]
    );

    // Continuation lines should use the second line's indent (8 spaces).
    if lines.len() > 1 {
        assert!(
            lines[1].starts_with("        "),
            "Continuation lines should use second line indent: {:?}",
            lines[1]
        );
    }
}

// ===== test_fmt_empty_input =====

#[test]
fn test_fmt_empty_input() {
    let result = run_default("");
    assert_eq!(result, "", "Empty input should produce empty output");
}

// ===== test_fmt_already_formatted =====

#[test]
fn test_fmt_already_formatted() {
    // Input already fits within 75 chars and is a single paragraph.
    let input = "This is a short line.\n";
    let result = run_default(input);
    assert_eq!(
        result, input,
        "Already formatted input should not change"
    );
}

#[test]
fn test_fmt_already_formatted_multi() {
    let input = "Line one of the paragraph.\nLine two of the same paragraph.\n";
    let result = run_default(input);
    // Words from both lines should be reflowed into a single line (since they fit).
    assert!(
        !result.contains("\nLine two"),
        "Short lines in same paragraph should be joined"
    );
}

// ===== test_fmt_matches_gnu =====

#[test]
fn test_fmt_matches_gnu() {
    // Verify basic behavior matches GNU fmt defaults.
    let input = "The quick brown fox jumps over the lazy dog. \
                 The quick brown fox jumps over the lazy dog. \
                 The quick brown fox jumps over the lazy dog.\n";
    let result = run_default(input);

    // All lines should be at most 75 chars.
    for line in result.lines() {
        assert!(
            line.len() <= 75,
            "Line exceeds 75: len={}, {:?}",
            line.len(),
            line
        );
    }

    // Output should end with newline.
    assert!(result.ends_with('\n'), "Output should end with newline");

    // No leading or trailing whitespace on lines (default mode, no indent in input).
    for line in result.lines() {
        assert_eq!(
            line,
            line.trim(),
            "Lines should have no extra whitespace"
        );
    }
}

// ===== Additional edge-case tests =====

#[test]
fn test_fmt_single_word() {
    let input = "superlongword\n";
    let result = run_default(input);
    assert_eq!(result, "superlongword\n");
}

#[test]
fn test_fmt_multiple_blank_lines() {
    let input = "para one\n\n\npara two\n";
    let result = run_default(input);
    // Two blank lines should be preserved.
    assert!(
        result.contains("\n\n\n"),
        "Multiple blank lines should be preserved: {:?}",
        result
    );
}

#[test]
fn test_fmt_only_whitespace_lines() {
    let input = "   \n   \n";
    let result = run_default(input);
    // Lines with only whitespace are blank and should be emitted as blank lines.
    assert_eq!(result, "\n\n");
}

#[test]
fn test_fmt_width_of_one() {
    let config = FmtConfig {
        width: 1,
        goal: 1,
        ..FmtConfig::default()
    };
    let input = "a b c\n";
    let result = run_fmt(input, &config);
    // Each word on its own line.
    assert_eq!(result, "a\nb\nc\n");
}

// ===== Integration tests via binary =====

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

    fn run_ffmt(input: &[u8], args: &[&str]) -> (Vec<u8>, i32) {
        let mut cmd = Command::new(bin_path("ffmt"));
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn ffmt");
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().expect("failed to wait");
        (output.stdout, output.status.code().unwrap_or(1))
    }

    #[test]
    fn test_ffmt_stdin_default() {
        let input = "The quick brown fox jumps over the lazy dog. \
                     The quick brown fox jumps over the lazy dog. \
                     The quick brown fox jumps over the lazy dog.\n";
        let (out, code) = run_ffmt(input.as_bytes(), &[]);
        assert_eq!(code, 0);
        let output = String::from_utf8(out).unwrap();
        for line in output.lines() {
            assert!(line.len() <= 75, "Line exceeds 75: {:?}", line);
        }
    }

    #[test]
    fn test_ffmt_custom_width() {
        let input = b"one two three four five six seven eight nine ten\n";
        let (out, code) = run_ffmt(input, &["-w", "20"]);
        assert_eq!(code, 0);
        let output = String::from_utf8(out).unwrap();
        for line in output.lines() {
            assert!(line.len() <= 20, "Line exceeds 20: {:?}", line);
        }
    }

    #[test]
    fn test_ffmt_split_only() {
        let input = b"short\nanother short\n";
        let (out, code) = run_ffmt(input, &["-s"]);
        assert_eq!(code, 0);
        let output = String::from_utf8(out).unwrap();
        assert_eq!(output, "short\nanother short\n");
    }

    #[test]
    fn test_ffmt_help() {
        let (_, code) = run_ffmt(b"", &["--help"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_ffmt_version() {
        let (_, code) = run_ffmt(b"", &["--version"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_ffmt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(
            &path,
            "one two three four five six seven eight nine ten eleven twelve\n",
        )
        .unwrap();
        let (out, code) = run_ffmt(b"", &["-w", "25", path.to_str().unwrap()]);
        assert_eq!(code, 0);
        let output = String::from_utf8(out).unwrap();
        for line in output.lines() {
            assert!(line.len() <= 25, "Line exceeds 25: {:?}", line);
        }
    }

    #[test]
    fn test_ffmt_nonexistent_file() {
        let (_, code) = run_ffmt(b"", &["/tmp/nonexistent_ffmt_test_file"]);
        assert_eq!(code, 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_ffmt_matches_gnu() {
        let input = b"The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog.\n";

        let (our_out, code) = run_ffmt(input, &["-w", "40"]);
        assert_eq!(code, 0);

        let gnu_out = Command::new("fmt")
            .args(["-w", "40"])
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
                assert_eq!(
                    our_out, gnu.stdout,
                    "Output differs from GNU fmt -w 40:\nours: {:?}\ngnu:  {:?}",
                    String::from_utf8_lossy(&our_out),
                    String::from_utf8_lossy(&gnu.stdout)
                );
            }
        }
    }
}
