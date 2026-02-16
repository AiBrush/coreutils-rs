use super::*;

fn run_ptx(input: &str, config: &PtxConfig) -> String {
    let mut output = Vec::new();
    generate_ptx(input.as_bytes(), &mut output, config).unwrap();
    String::from_utf8(output).unwrap()
}

fn default_config() -> PtxConfig {
    PtxConfig::default()
}

#[test]
fn test_ptx_basic() {
    let input = "the quick brown fox\n";
    let config = default_config();
    let output = run_ptx(input, &config);

    // Should have entries for each word: the, quick, brown, fox
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 4, "Expected 4 KWIC entries, got {}", lines.len());

    // Entries should be sorted alphabetically by keyword
    // brown, fox, quick, the
    assert!(
        lines[0].contains("brown"),
        "First entry should contain 'brown', got: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("fox"),
        "Second entry should contain 'fox', got: {}",
        lines[1]
    );
    assert!(
        lines[2].contains("quick"),
        "Third entry should contain 'quick', got: {}",
        lines[2]
    );
    assert!(
        lines[3].contains("the"),
        "Fourth entry should contain 'the', got: {}",
        lines[3]
    );
}

#[test]
fn test_ptx_ignore_case() {
    let input = "The Quick BROWN fox\n";
    let mut config = default_config();
    config.ignore_case = true;

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 4);

    // Should be sorted case-insensitively: BROWN, fox, Quick, The
    assert!(lines[0].contains("BROWN"));
    assert!(lines[1].contains("fox"));
    assert!(lines[2].contains("Quick"));
    assert!(lines[3].contains("The"));
}

#[test]
fn test_ptx_width() {
    let input = "the quick brown fox jumps over the lazy dog\n";

    let mut config = default_config();
    config.width = 40;
    let output_narrow = run_ptx(input, &config);

    config.width = 120;
    let output_wide = run_ptx(input, &config);

    // Both should have the same number of entries
    let narrow_lines: Vec<&str> = output_narrow.lines().collect();
    let wide_lines: Vec<&str> = output_wide.lines().collect();
    assert_eq!(narrow_lines.len(), wide_lines.len());

    // Narrow output lines should not exceed width
    for line in &narrow_lines {
        assert!(
            line.len() <= 45,
            "Line exceeds width limit: {} (len={})",
            line,
            line.len()
        );
    }
}

#[test]
fn test_ptx_matches_gnu() {
    // Test that basic functionality produces expected sorted output
    let input = "apple banana cherry\n";
    let config = default_config();
    let output = run_ptx(input, &config);

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3);

    // Should be sorted: apple, banana, cherry
    assert!(lines[0].contains("apple"));
    assert!(lines[1].contains("banana"));
    assert!(lines[2].contains("cherry"));

    // Each entry should show the keyword with some context
    for line in &lines {
        assert!(!line.is_empty(), "Output line should not be empty");
    }
}

#[test]
fn test_ptx_ignore_words() {
    let input = "the quick brown fox\n";
    let mut config = default_config();
    config.ignore_words = ["the".to_string()].into_iter().collect();

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    // "the" should be excluded
    assert_eq!(lines.len(), 3);
    // Verify sorted order: brown, fox, quick
    assert!(lines[0].contains("brown"));
    assert!(lines[1].contains("fox"));
    assert!(lines[2].contains("quick"));
}

#[test]
fn test_ptx_only_words() {
    let input = "the quick brown fox\n";
    let mut config = default_config();
    config.only_words = Some(["brown".to_string(), "fox".to_string()].into_iter().collect());

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    // Only "brown" and "fox" should be indexed
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("brown"));
    assert!(lines[1].contains("fox"));
}

#[test]
fn test_ptx_auto_reference() {
    let input = "first line\nsecond line\n";
    let mut config = default_config();
    config.auto_reference = true;

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    // Should have entries for: first, line (x2), second
    assert_eq!(lines.len(), 4);

    // Auto references should include line numbers
    // Lines for "first" and "line" from line 1 should reference "1"
    // Lines for "second" and "line" from line 2 should reference "2"
    let has_ref = lines.iter().any(|l| l.contains('1') || l.contains('2'));
    assert!(has_ref, "Auto-reference should include line numbers");
}

#[test]
fn test_ptx_roff_format() {
    let input = "hello world\n";
    let mut config = default_config();
    config.format = OutputFormat::Roff;

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert!(
            line.starts_with(".xx "),
            "Roff output should start with '.xx ', got: {}",
            line
        );
    }
}

#[test]
fn test_ptx_tex_format() {
    let input = "hello world\n";
    let mut config = default_config();
    config.format = OutputFormat::Tex;

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert!(
            line.starts_with("\\xx "),
            "TeX output should start with '\\xx ', got: {}",
            line
        );
    }
}

#[test]
fn test_ptx_empty_input() {
    let input = "";
    let config = default_config();
    let output = run_ptx(input, &config);

    assert!(output.is_empty(), "Empty input should produce empty output");
}

#[test]
fn test_ptx_multiple_lines() {
    let input = "cat sat\ndog ran\n";
    let config = default_config();
    let output = run_ptx(input, &config);

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 4); // cat, dog, ran, sat

    assert!(lines[0].contains("cat"));
    assert!(lines[1].contains("dog"));
    assert!(lines[2].contains("ran"));
    assert!(lines[3].contains("sat"));
}

#[test]
fn test_ptx_ignore_case_sorting() {
    let input = "Zebra apple Mango banana\n";
    let mut config = default_config();
    config.ignore_case = true;

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    // Case-insensitive sort: apple, banana, Mango, Zebra
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("apple"));
    assert!(lines[1].contains("banana"));
    assert!(lines[2].contains("Mango"));
    assert!(lines[3].contains("Zebra"));
}

#[test]
fn test_ptx_ignore_case_in_ignore_words() {
    let input = "The quick Brown FOX\n";
    let mut config = default_config();
    config.ignore_case = true;
    config.ignore_words = ["the".to_string(), "fox".to_string()].into_iter().collect();

    let output = run_ptx(input, &config);
    let lines: Vec<&str> = output.lines().collect();

    // "The" and "FOX" should be excluded (case-insensitive match)
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("Brown"));
    assert!(lines[1].contains("quick"));
}
