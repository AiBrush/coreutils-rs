use super::*;

fn run_tac(input: &[u8], sep: u8, before: bool) -> Vec<u8> {
    let mut out = Vec::new();
    tac_bytes(input, sep, before, &mut out).unwrap();
    out
}

fn run_tac_str(input: &[u8], sep: &[u8], before: bool) -> Vec<u8> {
    let mut out = Vec::new();
    tac_string_separator(input, sep, before, &mut out).unwrap();
    out
}

fn run_tac_regex(input: &[u8], pattern: &str, before: bool) -> Vec<u8> {
    let mut out = Vec::new();
    tac_regex_separator(input, pattern, before, &mut out).unwrap();
    out
}

// ---- Basic newline separator tests ----

#[test]
fn test_empty_input() {
    assert_eq!(run_tac(b"", b'\n', false), b"");
}

#[test]
fn test_single_line_with_newline() {
    assert_eq!(run_tac(b"hello\n", b'\n', false), b"hello\n");
}

#[test]
fn test_single_line_no_newline() {
    // No separator found in input — output as-is (consistent with tac_string_separator)
    assert_eq!(run_tac(b"hello", b'\n', false), b"hello");
}

#[test]
fn test_two_lines() {
    assert_eq!(run_tac(b"aaa\nbbb\n", b'\n', false), b"bbb\naaa\n");
}

#[test]
fn test_three_lines() {
    assert_eq!(
        run_tac(b"one\ntwo\nthree\n", b'\n', false),
        b"three\ntwo\none\n"
    );
}

#[test]
fn test_no_trailing_newline() {
    // GNU tac: trailing "bbb" stays bare (no separator added), then "aaa\n" = "bbbaaa\n"
    assert_eq!(run_tac(b"aaa\nbbb", b'\n', false), b"bbbaaa\n");
}

#[test]
fn test_empty_lines() {
    assert_eq!(run_tac(b"\n\n\n", b'\n', false), b"\n\n\n");
}

#[test]
fn test_mixed_empty_lines() {
    assert_eq!(run_tac(b"a\n\nb\n", b'\n', false), b"b\n\na\n");
}

#[test]
fn test_only_newline() {
    assert_eq!(run_tac(b"\n", b'\n', false), b"\n");
}

// ---- Before mode tests ----

#[test]
fn test_before_basic() {
    // With --before, separator attaches before the record
    // "aaa\nbbb\n" -> records are "aaa", "\nbbb", "\n"
    // reversed: "\n", "\nbbb", "aaa"
    assert_eq!(run_tac(b"aaa\nbbb\n", b'\n', true), b"\n\nbbbaaa");
}

#[test]
fn test_before_no_leading_sep() {
    assert_eq!(run_tac(b"aaa\nbbb", b'\n', true), b"\nbbbaaa");
}

// ---- Custom separator tests ----

#[test]
fn test_custom_separator_comma() {
    assert_eq!(run_tac(b"a,b,c,", b',', false), b"c,b,a,");
}

#[test]
fn test_custom_separator_no_trailing() {
    // GNU tac: trailing "c" stays bare (no separator added) = "cb,a,"
    assert_eq!(run_tac(b"a,b,c", b',', false), b"cb,a,");
}

// ---- Multi-byte string separator tests ----

#[test]
fn test_string_separator() {
    assert_eq!(run_tac_str(b"aXYbXYcXY", b"XY", false), b"cXYbXYaXY");
}

#[test]
fn test_string_separator_no_trailing() {
    // GNU tac: trailing "c" stays bare (no separator added) = "cbXYaXY"
    assert_eq!(run_tac_str(b"aXYbXYc", b"XY", false), b"cbXYaXY");
}

#[test]
fn test_string_separator_before() {
    assert_eq!(run_tac_str(b"aXYbXYc", b"XY", true), b"XYcXYba");
}

// ---- Regex separator tests ----

#[test]
fn test_regex_separator_digit() {
    // Separator is any digit — use [0-9] (POSIX ERE compatible, same as GNU tac)
    assert_eq!(run_tac_regex(b"a1b2c3", r"[0-9]", false), b"c3b2a1");
}

#[test]
fn test_regex_separator_newline() {
    assert_eq!(run_tac_regex(b"aaa\nbbb\n", r"\n", false), b"bbb\naaa\n");
}

// ---- Edge cases ----

#[test]
fn test_no_separator_found() {
    assert_eq!(run_tac(b"hello world", b',', false), b"hello world");
}

#[test]
fn test_only_separators() {
    assert_eq!(run_tac(b",,", b',', false), b",,");
}

#[test]
fn test_binary_data() {
    let data = b"\x00\x01\n\x02\x03\n";
    let result = run_tac(data, b'\n', false);
    assert_eq!(result, b"\x02\x03\n\x00\x01\n");
}

#[test]
fn test_large_input() {
    let mut input = Vec::new();
    for i in 0..10000 {
        input.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    let result = run_tac(&input, b'\n', false);
    // Check first and last lines of reversed output
    assert!(result.starts_with(b"line 9999\n"));
    assert!(result.ends_with(b"line 0\n"));
}
