use super::*;
use std::io::Cursor;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn pr_helper(input: &str, config: &PrConfig) -> String {
    let reader = Cursor::new(input.as_bytes());
    let mut output = Vec::new();
    // Use a fixed date for reproducible tests
    let fixed_date = UNIX_EPOCH + Duration::from_secs(1_700_000_000); // 2023-11-14
    pr_file(reader, &mut output, config, "test_file", Some(fixed_date)).unwrap();
    String::from_utf8(output).unwrap()
}

fn default_config() -> PrConfig {
    PrConfig::default()
}

#[test]
fn test_pr_default_pagination() {
    let input = "line1\nline2\nline3\n";
    let config = default_config();
    let result = pr_helper(input, &config);

    // Should contain header with date and filename
    assert!(result.contains("test_file"));
    assert!(result.contains("Page 1"));

    // Should contain the input lines
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
    assert!(result.contains("line3"));

    // Count total lines: header(5) + body lines + padding to fill page + footer(5)
    let line_count = result.lines().count();
    // The output ends with footer newlines; total should be page_length (66)
    // But we might have a trailing newline after footer. Just verify header + footer exist.
    assert!(
        line_count >= DEFAULT_PAGE_LENGTH - 1,
        "Expected at least {} lines, got {}",
        DEFAULT_PAGE_LENGTH - 1,
        line_count
    );
}

#[test]
fn test_pr_columns() {
    let input = "a\nb\nc\nd\ne\nf\n";
    let config = PrConfig {
        columns: 2,
        omit_header: true,
        omit_pagination: true,
        ..default_config()
    };
    let result = pr_helper(input, &config);

    // With 2 columns and no header, lines should be arranged in columns
    // Default (down): col0 = a,b,c; col1 = d,e,f
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines.len() >= 3, "Expected at least 3 rows for 2 columns with 6 lines");
    // First line should contain 'a' in column 1
    assert!(lines[0].contains('a'));
}

#[test]
fn test_pr_header() {
    let input = "hello\n";
    let config = PrConfig {
        header: Some("My Custom Header".to_string()),
        ..default_config()
    };
    let result = pr_helper(input, &config);

    assert!(result.contains("My Custom Header"));
    assert!(result.contains("Page 1"));
    assert!(result.contains("hello"));
}

#[test]
fn test_pr_page_length() {
    let input = "line1\nline2\n";
    let config = PrConfig {
        page_length: 20,
        ..default_config()
    };
    let result = pr_helper(input, &config);

    // With page_length=20, header=5, footer=5 => body=10 lines
    // So total output should be around 20 lines
    let line_count = result.lines().count();
    // Account for possible trailing newline differences
    assert!(
        line_count <= 21 && line_count >= 19,
        "Expected ~20 lines, got {}",
        line_count
    );
}

#[test]
fn test_pr_double_space() {
    let input = "line1\nline2\nline3\n";
    let config = PrConfig {
        double_space: true,
        omit_header: true,
        omit_pagination: true,
        ..default_config()
    };
    let result = pr_helper(input, &config);

    // With double spacing, there should be blank lines between content lines
    let lines: Vec<&str> = result.lines().collect();
    // line1, blank, line2, blank, line3
    assert!(lines.len() >= 5, "Expected at least 5 lines with double spacing, got {}", lines.len());
    assert_eq!(lines[0].trim(), "line1");
    assert_eq!(lines[1].trim(), "");
    assert_eq!(lines[2].trim(), "line2");
}

#[test]
fn test_pr_number_lines() {
    let input = "aaa\nbbb\nccc\n";
    let config = PrConfig {
        number_lines: Some(('\t', 5)),
        omit_header: true,
        omit_pagination: true,
        ..default_config()
    };
    let result = pr_helper(input, &config);

    let lines: Vec<&str> = result.lines().collect();
    // Lines should be numbered starting from 1
    assert!(lines[0].contains('1'), "First line should contain number 1: '{}'", lines[0]);
    assert!(lines[0].contains("aaa"));
    assert!(lines[1].contains('2'), "Second line should contain number 2: '{}'", lines[1]);
    assert!(lines[1].contains("bbb"));
}

#[test]
fn test_pr_omit_header() {
    let input = "line1\nline2\n";
    let config = PrConfig {
        omit_header: true,
        ..default_config()
    };
    let result = pr_helper(input, &config);

    // Should NOT contain header elements
    assert!(!result.contains("Page 1"));
    assert!(!result.contains("test_file"));

    // Should still contain content
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
}

#[test]
fn test_pr_matches_gnu() {
    // Basic test: verify structure matches GNU pr output format.
    // GNU pr: 2 blank + header line + 2 blank + body + footer(5 blank)
    let input = "test\n";
    let config = PrConfig {
        page_length: 20,
        ..default_config()
    };
    let result = pr_helper(input, &config);
    let lines: Vec<&str> = result.lines().collect();

    // First two lines should be blank (header padding)
    assert_eq!(lines[0], "", "Line 0 should be blank header padding");
    assert_eq!(lines[1], "", "Line 1 should be blank header padding");

    // Line 2 should be the header line with date, filename, page
    assert!(
        lines[2].contains("test_file") && lines[2].contains("Page 1"),
        "Line 2 should be the header: '{}'",
        lines[2]
    );

    // Lines 3,4 should be blank (header padding)
    assert_eq!(lines[3], "", "Line 3 should be blank header padding");
    assert_eq!(lines[4], "", "Line 4 should be blank header padding");

    // Line 5 should be the content
    assert_eq!(lines[5], "test", "Line 5 should be content 'test'");
}
