use super::compare::*;
use super::core::*;
use super::key::*;
use std::cmp::Ordering;

#[test]
fn test_basic_lexical_sort() {
    let config = SortConfig::default();
    let _inputs: Vec<&str> = vec![];
    let mut lines = [b"banana".to_vec(), b"apple".to_vec(), b"cherry".to_vec()];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"apple");
    assert_eq!(lines[1], b"banana");
    assert_eq!(lines[2], b"cherry");
}

#[test]
fn test_reverse_sort() {
    let mut config = SortConfig::default();
    config.global_opts.reverse = true;
    let mut lines = [b"banana".to_vec(), b"apple".to_vec(), b"cherry".to_vec()];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"cherry");
    assert_eq!(lines[1], b"banana");
    assert_eq!(lines[2], b"apple");
}

#[test]
fn test_numeric_sort() {
    let mut config = SortConfig::default();
    config.global_opts.numeric = true;
    let mut lines = [b"10".to_vec(), b"2".to_vec(), b"1".to_vec(), b"20".to_vec()];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"1");
    assert_eq!(lines[1], b"2");
    assert_eq!(lines[2], b"10");
    assert_eq!(lines[3], b"20");
}

#[test]
fn test_ignore_case_sort() {
    let mut config = SortConfig::default();
    config.global_opts.ignore_case = true;
    let mut lines = [b"Banana".to_vec(), b"apple".to_vec(), b"Cherry".to_vec()];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"apple");
    assert_eq!(lines[1], b"Banana");
    assert_eq!(lines[2], b"Cherry");
}

#[test]
fn test_month_sort() {
    assert_eq!(compare_month(b"JAN", b"FEB"), Ordering::Less);
    assert_eq!(compare_month(b"DEC", b"JAN"), Ordering::Greater);
    assert_eq!(compare_month(b"mar", b"MAR"), Ordering::Equal);
    assert_eq!(compare_month(b"XXX", b"JAN"), Ordering::Less);
}

#[test]
fn test_human_numeric_sort() {
    assert_eq!(compare_human_numeric(b"1K", b"1M"), Ordering::Less);
    assert_eq!(compare_human_numeric(b"2G", b"1G"), Ordering::Greater);
    assert_eq!(compare_human_numeric(b"100", b"1K"), Ordering::Less);
}

#[test]
fn test_version_sort() {
    assert_eq!(compare_version(b"1.2", b"1.10"), Ordering::Less);
    assert_eq!(compare_version(b"2.0", b"1.9"), Ordering::Greater);
    assert_eq!(compare_version(b"1.0", b"1.0"), Ordering::Equal);
}

#[test]
fn test_general_numeric_sort() {
    assert_eq!(compare_general_numeric(b"1.5e2", b"200"), Ordering::Less);
    assert_eq!(compare_general_numeric(b"inf", b"1"), Ordering::Greater);
}

#[test]
fn test_key_parsing() {
    let k = KeyDef::parse("2,2").unwrap();
    assert_eq!(k.start_field, 2);
    assert_eq!(k.end_field, 2);
    assert_eq!(k.start_char, 0);
    assert_eq!(k.end_char, 0);

    let k = KeyDef::parse("1.3,1.5").unwrap();
    assert_eq!(k.start_field, 1);
    assert_eq!(k.start_char, 3);
    assert_eq!(k.end_field, 1);
    assert_eq!(k.end_char, 5);

    let k = KeyDef::parse("2,2n").unwrap();
    assert!(k.opts.numeric);

    let k = KeyDef::parse("3,3rn").unwrap();
    assert!(k.opts.reverse);
    assert!(k.opts.numeric);
}

#[test]
fn test_key_extraction_with_separator() {
    let line = b"alice\tbob\tcharlie";
    let key = KeyDef::parse("2,2").unwrap();
    let extracted = extract_key(line, &key, Some(b'\t'));
    assert_eq!(extracted, b"bob");
}

#[test]
fn test_key_extraction_blank_separator() {
    let line = b"alice bob charlie";
    let key = KeyDef::parse("2,2").unwrap();
    let extracted = extract_key(line, &key, None);
    // Field 2 with blank separator: "bob"
    // With default blank separator, leading blanks are part of the field
    let extracted_str = std::str::from_utf8(extracted).unwrap();
    assert!(extracted_str.contains("bob"), "got: {:?}", extracted_str);
}

#[test]
fn test_key_sort_numeric() {
    let config = SortConfig {
        separator: Some(b'\t'),
        keys: vec![KeyDef::parse("1,1n").unwrap()],
        ..SortConfig::default()
    };

    let mut lines = [
        b"10\tbanana".to_vec(),
        b"2\tapple".to_vec(),
        b"1\tcherry".to_vec(),
    ];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"1\tcherry");
    assert_eq!(lines[1], b"2\tapple");
    assert_eq!(lines[2], b"10\tbanana");
}

#[test]
fn test_unique_dedup() {
    let config = SortConfig {
        unique: true,
        ..SortConfig::default()
    };
    let mut lines = vec![
        b"apple".to_vec(),
        b"apple".to_vec(),
        b"banana".to_vec(),
        b"banana".to_vec(),
        b"cherry".to_vec(),
    ];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    // Dedup uses compare_lines_for_dedup (no last-resort)
    let mut output: Vec<&[u8]> = Vec::new();
    let mut prev: Option<&[u8]> = None;
    for line in &lines {
        let should = match prev {
            Some(p) => compare_lines_for_dedup(p, line, &config) != Ordering::Equal,
            None => true,
        };
        if should {
            output.push(line);
            prev = Some(line);
        }
    }
    assert_eq!(output.len(), 3);
}

#[test]
fn test_unique_dedup_case_insensitive() {
    // GNU sort -fu: "Apple" and "apple" are equal, only first should be emitted
    let config = SortConfig {
        unique: true,
        global_opts: KeyOpts {
            ignore_case: true,
            ..KeyOpts::default()
        },
        ..SortConfig::default()
    };
    let mut lines = vec![
        b"apple".to_vec(),
        b"Apple".to_vec(),
        b"BANANA".to_vec(),
        b"banana".to_vec(),
    ];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    let mut output: Vec<&[u8]> = Vec::new();
    let mut prev: Option<&[u8]> = None;
    for line in &lines {
        let should = match prev {
            Some(p) => compare_lines_for_dedup(p, line, &config) != Ordering::Equal,
            None => true,
        };
        if should {
            output.push(line);
            prev = Some(line);
        }
    }
    // Only 2 unique entries: one apple variant, one banana variant
    assert_eq!(output.len(), 2);
}

#[test]
fn test_dictionary_order() {
    assert_eq!(compare_dictionary(b"a-b", b"ab", false), Ordering::Equal);
}

#[test]
fn test_numeric_leading_blanks() {
    assert_eq!(compare_numeric(b"  10", b"2"), Ordering::Greater);
    assert_eq!(compare_numeric(b"-5", b"3"), Ordering::Less);
}

#[test]
fn test_buffer_size_parsing() {
    assert_eq!(parse_buffer_size("1024").unwrap(), 1024);
    assert_eq!(parse_buffer_size("1K").unwrap(), 1024);
    assert_eq!(parse_buffer_size("1M").unwrap(), 1024 * 1024);
    assert_eq!(parse_buffer_size("1G").unwrap(), 1024 * 1024 * 1024);
}

#[test]
fn test_empty_input() {
    let config = SortConfig::default();
    let mut lines: Vec<Vec<u8>> = vec![];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert!(lines.is_empty());
}

#[test]
fn test_single_line() {
    let config = SortConfig::default();
    let mut lines = [b"hello".to_vec()];
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"hello");
}

#[test]
fn test_stable_sort() {
    let config = SortConfig {
        stable: true,
        keys: vec![KeyDef::parse("1,1").unwrap()],
        separator: Some(b'\t'),
        ..SortConfig::default()
    };

    let mut lines = [b"a\t2".to_vec(), b"a\t1".to_vec(), b"b\t1".to_vec()];
    // With stable + key on field 1: a\t2 and a\t1 should keep original order
    lines.sort_by(|a, b| compare_lines(a, b, &config));
    assert_eq!(lines[0], b"a\t2");
    assert_eq!(lines[1], b"a\t1");
    assert_eq!(lines[2], b"b\t1");
}
