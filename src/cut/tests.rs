use super::*;

fn cut_field_str(input: &str, delim: u8, spec: &str, complement: bool, suppress: bool) -> String {
    let ranges = parse_ranges(spec).unwrap();
    let output_delim = &[delim];
    let mut out = Vec::new();
    cut_fields(
        input.as_bytes(),
        delim,
        &ranges,
        complement,
        output_delim,
        suppress,
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

fn cut_byte_str(input: &str, spec: &str, complement: bool) -> String {
    let ranges = parse_ranges(spec).unwrap();
    let mut out = Vec::new();
    cut_bytes(input.as_bytes(), &ranges, complement, b"", &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

// --- Range parsing ---

#[test]
fn test_parse_single() {
    let r = parse_ranges("3").unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].start, 3);
    assert_eq!(r[0].end, 3);
}

#[test]
fn test_parse_range() {
    let r = parse_ranges("2-4").unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].start, 2);
    assert_eq!(r[0].end, 4);
}

#[test]
fn test_parse_open_start() {
    let r = parse_ranges("-3").unwrap();
    assert_eq!(r[0].start, 1);
    assert_eq!(r[0].end, 3);
}

#[test]
fn test_parse_open_end() {
    let r = parse_ranges("3-").unwrap();
    assert_eq!(r[0].start, 3);
    assert_eq!(r[0].end, usize::MAX);
}

#[test]
fn test_parse_multiple() {
    let r = parse_ranges("1,3,5").unwrap();
    assert_eq!(r.len(), 3);
}

#[test]
fn test_parse_merge_overlapping() {
    let r = parse_ranges("1-3,2-5").unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].start, 1);
    assert_eq!(r[0].end, 5);
}

#[test]
fn test_parse_merge_adjacent() {
    let r = parse_ranges("1-2,3-4").unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].start, 1);
    assert_eq!(r[0].end, 4);
}

#[test]
fn test_parse_zero_rejected() {
    assert!(parse_ranges("0").is_err());
}

// --- Field cutting ---

#[test]
fn test_cut_field_single() {
    assert_eq!(cut_field_str("a:b:c:d", b':', "2", false, false), "b");
}

#[test]
fn test_cut_field_multiple() {
    assert_eq!(cut_field_str("a:b:c:d", b':', "1,3", false, false), "a:c");
}

#[test]
fn test_cut_field_range() {
    assert_eq!(cut_field_str("a:b:c:d", b':', "2-4", false, false), "b:c:d");
}

#[test]
fn test_cut_field_open_start() {
    assert_eq!(cut_field_str("a:b:c:d", b':', "-2", false, false), "a:b");
}

#[test]
fn test_cut_field_open_end() {
    assert_eq!(cut_field_str("a:b:c:d", b':', "3-", false, false), "c:d");
}

#[test]
fn test_cut_field_no_delim_print() {
    assert_eq!(
        cut_field_str("no delimiter", b':', "1", false, false),
        "no delimiter"
    );
}

#[test]
fn test_cut_field_no_delim_suppress() {
    assert_eq!(cut_field_str("no delimiter", b':', "1", false, true), "");
}

#[test]
fn test_cut_field_complement() {
    assert_eq!(cut_field_str("a:b:c:d", b':', "2", true, false), "a:c:d");
}

#[test]
fn test_cut_field_empty_fields() {
    assert_eq!(cut_field_str("a::c", b':', "2", false, false), "");
    assert_eq!(cut_field_str("::c", b':', "1", false, false), "");
}

#[test]
fn test_cut_field_tab_default() {
    assert_eq!(cut_field_str("a\tb\tc", b'\t', "2", false, false), "b");
}

// --- Byte cutting ---

#[test]
fn test_cut_bytes_single() {
    assert_eq!(cut_byte_str("hello", "1", false), "h");
}

#[test]
fn test_cut_bytes_range() {
    assert_eq!(cut_byte_str("hello world", "1-5", false), "hello");
}

#[test]
fn test_cut_bytes_multiple() {
    assert_eq!(cut_byte_str("hello", "1,3,5", false), "hlo");
}

#[test]
fn test_cut_bytes_complement() {
    assert_eq!(cut_byte_str("hello", "1,3,5", true), "el");
}

#[test]
fn test_cut_bytes_open_end() {
    assert_eq!(cut_byte_str("hello", "3-", false), "llo");
}
