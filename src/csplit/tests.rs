use super::*;

fn default_config_with_dir(dir: &std::path::Path) -> CsplitConfig {
    CsplitConfig {
        prefix: dir.join("xx").to_string_lossy().into_owned(),
        ..CsplitConfig::default()
    }
}

#[test]
fn test_csplit_by_line_number() {
    let dir = tempfile::tempdir().unwrap();
    let config = default_config_with_dir(dir.path());

    let input = "line1\nline2\nline3\nline4\nline5\n";
    let patterns = vec![Pattern::LineNumber(3)];

    let sizes = csplit_file(input, &patterns, &config).unwrap();

    assert_eq!(sizes.len(), 2);

    // First file: lines 1-2
    let f0 = std::fs::read_to_string(output_filename(&config, 0)).unwrap();
    assert_eq!(f0, "line1\nline2\n");

    // Second file: lines 3-5
    let f1 = std::fs::read_to_string(output_filename(&config, 1)).unwrap();
    assert_eq!(f1, "line3\nline4\nline5\n");

    assert_eq!(sizes[0], f0.len() as u64);
    assert_eq!(sizes[1], f1.len() as u64);
}

#[test]
fn test_csplit_by_regex() {
    let dir = tempfile::tempdir().unwrap();
    let config = default_config_with_dir(dir.path());

    let input = "header\ndata1\n---\ndata2\ndata3\n";
    let patterns = vec![Pattern::Regex {
        regex: "^---".to_string(),
        offset: 0,
    }];

    let sizes = csplit_file(input, &patterns, &config).unwrap();

    assert_eq!(sizes.len(), 2);

    let f0 = std::fs::read_to_string(output_filename(&config, 0)).unwrap();
    assert_eq!(f0, "header\ndata1\n");

    let f1 = std::fs::read_to_string(output_filename(&config, 1)).unwrap();
    assert_eq!(f1, "---\ndata2\ndata3\n");
}

#[test]
fn test_csplit_with_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let config = default_config_with_dir(dir.path());

    let input = "a\n---\nb\n---\nc\n---\nd\n";
    let patterns = vec![
        Pattern::Regex {
            regex: "^---".to_string(),
            offset: 0,
        },
        Pattern::Repeat(2),
    ];

    let sizes = csplit_file(input, &patterns, &config).unwrap();

    // Should create 4 files: before first ---, between first and second ---,
    // between second and third ---, and after third ---
    assert_eq!(sizes.len(), 4);

    let f0 = std::fs::read_to_string(output_filename(&config, 0)).unwrap();
    assert_eq!(f0, "a\n");

    let f1 = std::fs::read_to_string(output_filename(&config, 1)).unwrap();
    assert_eq!(f1, "---\nb\n");

    let f2 = std::fs::read_to_string(output_filename(&config, 2)).unwrap();
    assert_eq!(f2, "---\nc\n");

    let f3 = std::fs::read_to_string(output_filename(&config, 3)).unwrap();
    assert_eq!(f3, "---\nd\n");
}

#[test]
fn test_csplit_custom_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let config = CsplitConfig {
        prefix: dir.path().join("part_").to_string_lossy().into_owned(),
        ..CsplitConfig::default()
    };

    let input = "line1\nline2\nline3\n";
    let patterns = vec![Pattern::LineNumber(2)];

    let sizes = csplit_file(input, &patterns, &config).unwrap();
    assert_eq!(sizes.len(), 2);

    // Verify files have custom prefix
    let f0_path = output_filename(&config, 0);
    assert!(f0_path.contains("part_"));
    assert!(std::path::Path::new(&f0_path).exists());

    let f1_path = output_filename(&config, 1);
    assert!(f1_path.contains("part_"));
    assert!(std::path::Path::new(&f1_path).exists());
}

#[test]
fn test_csplit_elide_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = CsplitConfig {
        prefix: dir.path().join("xx").to_string_lossy().into_owned(),
        elide_empty: true,
        ..CsplitConfig::default()
    };

    // Split at line 1 would create an empty first file
    let input = "line1\nline2\n";
    let patterns = vec![Pattern::LineNumber(1)];

    let sizes = csplit_file(input, &patterns, &config).unwrap();

    // With elide_empty, the empty first chunk should be skipped
    // The result should only contain non-empty chunks
    for &size in &sizes {
        assert!(size > 0, "Empty files should be elided");
    }
}

#[test]
fn test_csplit_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let config = default_config_with_dir(dir.path());

    let input = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
    let patterns = vec![Pattern::LineNumber(3), Pattern::LineNumber(5)];

    let sizes = csplit_file(input, &patterns, &config).unwrap();
    assert_eq!(sizes.len(), 3);

    // Concatenate all output files and verify they reconstruct the original
    let mut reconstructed = String::new();
    for i in 0..sizes.len() {
        let content = std::fs::read_to_string(output_filename(&config, i)).unwrap();
        reconstructed.push_str(&content);
    }

    // The original input has a trailing newline in each line; csplit preserves them
    assert_eq!(reconstructed, input);
}

#[test]
fn test_csplit_matches_gnu() {
    // Test that basic line-number splitting matches GNU csplit semantics:
    // csplit input 3 means split before line 3, creating files with
    // lines 1-2 and lines 3-end.
    let dir = tempfile::tempdir().unwrap();
    let config = default_config_with_dir(dir.path());

    let input = "1\n2\n3\n4\n5\n";
    let patterns = vec![Pattern::LineNumber(3)];

    let sizes = csplit_file(input, &patterns, &config).unwrap();
    assert_eq!(sizes.len(), 2);

    let f0 = std::fs::read_to_string(output_filename(&config, 0)).unwrap();
    let f1 = std::fs::read_to_string(output_filename(&config, 1)).unwrap();

    // GNU csplit: first file contains lines before line 3 (lines 1-2)
    assert_eq!(f0, "1\n2\n");
    // Second file contains line 3 onward
    assert_eq!(f1, "3\n4\n5\n");

    // Sizes should be byte counts including newlines
    assert_eq!(sizes[0], 4); // "1\n2\n" = 4 bytes
    assert_eq!(sizes[1], 6); // "3\n4\n5\n" = 6 bytes
}

#[test]
fn test_parse_pattern_line_number() {
    match parse_pattern("5").unwrap() {
        Pattern::LineNumber(5) => {}
        other => panic!("Expected LineNumber(5), got {:?}", other),
    }
}

#[test]
fn test_parse_pattern_regex() {
    match parse_pattern("/^Chapter/").unwrap() {
        Pattern::Regex { regex, offset } => {
            assert_eq!(regex, "^Chapter");
            assert_eq!(offset, 0);
        }
        other => panic!("Expected Regex, got {:?}", other),
    }
}

#[test]
fn test_parse_pattern_regex_with_offset() {
    match parse_pattern("/^---/+1").unwrap() {
        Pattern::Regex { regex, offset } => {
            assert_eq!(regex, "^---");
            assert_eq!(offset, 1);
        }
        other => panic!("Expected Regex with offset, got {:?}", other),
    }
}

#[test]
fn test_parse_pattern_skip_to() {
    match parse_pattern("%^END%").unwrap() {
        Pattern::SkipTo { regex, offset } => {
            assert_eq!(regex, "^END");
            assert_eq!(offset, 0);
        }
        other => panic!("Expected SkipTo, got {:?}", other),
    }
}

#[test]
fn test_parse_pattern_repeat() {
    match parse_pattern("{3}").unwrap() {
        Pattern::Repeat(3) => {}
        other => panic!("Expected Repeat(3), got {:?}", other),
    }
}

#[test]
fn test_parse_pattern_repeat_forever() {
    match parse_pattern("{*}").unwrap() {
        Pattern::RepeatForever => {}
        other => panic!("Expected RepeatForever, got {:?}", other),
    }
}

#[test]
fn test_parse_pattern_invalid() {
    assert!(parse_pattern("0").is_err());
    assert!(parse_pattern("abc").is_err());
    assert!(parse_pattern("/unclosed").is_err());
    assert!(parse_pattern("%unclosed").is_err());
}

#[test]
fn test_format_suffix() {
    assert_eq!(format_suffix("%02d", 0), "00");
    assert_eq!(format_suffix("%02d", 5), "05");
    assert_eq!(format_suffix("%02d", 42), "42");
    assert_eq!(format_suffix("%03d", 7), "007");
    assert_eq!(format_suffix("%d", 42), "42");
}

#[test]
fn test_csplit_skip_to_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let config = default_config_with_dir(dir.path());

    let input = "junk1\njunk2\nSTART\ndata1\ndata2\n";
    let patterns = vec![Pattern::SkipTo {
        regex: "^START".to_string(),
        offset: 0,
    }];

    let sizes = csplit_file(input, &patterns, &config).unwrap();

    // Only one output file: lines from START onward
    assert_eq!(sizes.len(), 1);

    let f0 = std::fs::read_to_string(output_filename(&config, 0)).unwrap();
    assert_eq!(f0, "START\ndata1\ndata2\n");
}
