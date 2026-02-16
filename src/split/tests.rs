use super::*;
use std::fs;
use std::path::{Path, PathBuf};

// ---- Helper functions ----

/// Get the path to a built binary. Works in both lib tests and integration tests.
fn bin_path(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove 'deps'
    path.push(name);
    path
}

/// Create a test file with the given content in the given directory and return its path.
fn create_test_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

/// Read all output chunk files matching the prefix in a directory, sorted by name.
fn read_chunks(dir: &Path, prefix: &str) -> Vec<Vec<u8>> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            name_str.starts_with(prefix)
                && !name_str.ends_with(".txt")
                && name_str.len() > prefix.len()
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries
        .iter()
        .map(|e| fs::read(e.path()).unwrap())
        .collect()
}

/// Count output chunk files matching the prefix in a directory.
fn count_chunks(dir: &Path, prefix: &str) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            name_str.starts_with(prefix)
                && !name_str.ends_with(".txt")
                && name_str.len() > prefix.len()
        })
        .count()
}

/// Generate test content: N lines of "line XXXX\n".
fn generate_lines(n: usize) -> Vec<u8> {
    let mut content = Vec::new();
    for i in 1..=n {
        content.extend_from_slice(format!("line {}\n", i).as_bytes());
    }
    content
}

// ---- Unit tests for parse_size ----

#[test]
fn test_parse_size_plain() {
    assert_eq!(parse_size("10").unwrap(), 10);
    assert_eq!(parse_size("0").unwrap(), 0);
    assert_eq!(parse_size("1").unwrap(), 1);
    assert_eq!(parse_size("999999").unwrap(), 999999);
}

#[test]
fn test_parse_size_suffixes() {
    assert_eq!(parse_size("1b").unwrap(), 512);
    assert_eq!(parse_size("1K").unwrap(), 1024);
    assert_eq!(parse_size("1kB").unwrap(), 1000);
    assert_eq!(parse_size("1M").unwrap(), 1_048_576);
    assert_eq!(parse_size("1MB").unwrap(), 1_000_000);
    assert_eq!(parse_size("1G").unwrap(), 1_073_741_824);
    assert_eq!(parse_size("1GB").unwrap(), 1_000_000_000);
    assert_eq!(parse_size("2K").unwrap(), 2048);
    assert_eq!(parse_size("10M").unwrap(), 10_485_760);
}

#[test]
fn test_parse_size_invalid() {
    assert!(parse_size("abc").is_err());
    assert!(parse_size("").is_err());
    assert!(parse_size("1X").is_err());
}

// ---- Unit tests for generate_suffix ----

#[test]
fn test_suffix_alphabetic() {
    assert_eq!(generate_suffix(0, &SuffixType::Alphabetic, 2), "aa");
    assert_eq!(generate_suffix(1, &SuffixType::Alphabetic, 2), "ab");
    assert_eq!(generate_suffix(25, &SuffixType::Alphabetic, 2), "az");
    assert_eq!(generate_suffix(26, &SuffixType::Alphabetic, 2), "ba");
    assert_eq!(generate_suffix(27, &SuffixType::Alphabetic, 2), "bb");
}

#[test]
fn test_suffix_numeric() {
    assert_eq!(generate_suffix(0, &SuffixType::Numeric(0), 2), "00");
    assert_eq!(generate_suffix(1, &SuffixType::Numeric(0), 2), "01");
    assert_eq!(generate_suffix(99, &SuffixType::Numeric(0), 2), "99");
    assert_eq!(generate_suffix(0, &SuffixType::Numeric(5), 2), "05");
    assert_eq!(generate_suffix(3, &SuffixType::Numeric(10), 3), "013");
}

#[test]
fn test_suffix_hex() {
    assert_eq!(generate_suffix(0, &SuffixType::Hex(0), 2), "00");
    assert_eq!(generate_suffix(1, &SuffixType::Hex(0), 2), "01");
    assert_eq!(generate_suffix(15, &SuffixType::Hex(0), 2), "0f");
    assert_eq!(generate_suffix(16, &SuffixType::Hex(0), 2), "10");
    assert_eq!(generate_suffix(255, &SuffixType::Hex(0), 2), "ff");
}

#[test]
fn test_suffix_length_3() {
    assert_eq!(generate_suffix(0, &SuffixType::Alphabetic, 3), "aaa");
    assert_eq!(generate_suffix(0, &SuffixType::Numeric(0), 3), "000");
    assert_eq!(generate_suffix(0, &SuffixType::Hex(0), 3), "000");
}

// ---- Unit tests for max_chunks ----

#[test]
fn test_max_chunks_alphabetic() {
    assert_eq!(max_chunks(&SuffixType::Alphabetic, 2), 676);
    assert_eq!(max_chunks(&SuffixType::Alphabetic, 3), 17576);
}

#[test]
fn test_max_chunks_numeric() {
    assert_eq!(max_chunks(&SuffixType::Numeric(0), 2), 100);
    assert_eq!(max_chunks(&SuffixType::Numeric(0), 3), 1000);
}

// ---- Integration: split by lines (default 1000 lines) ----

#[test]
fn test_split_by_lines() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(2500);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(1000),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 3);

    // First chunk: 1000 lines
    let first_lines: Vec<&[u8]> = chunks[0].split(|&b| b == b'\n').collect();
    // split produces an empty trailing element after the last newline
    assert_eq!(first_lines.len() - 1, 1000);

    // Second chunk: 1000 lines
    let second_lines: Vec<&[u8]> = chunks[1].split(|&b| b == b'\n').collect();
    assert_eq!(second_lines.len() - 1, 1000);

    // Third chunk: 500 lines
    let third_lines: Vec<&[u8]> = chunks[2].split(|&b| b == b'\n').collect();
    assert_eq!(third_lines.len() - 1, 500);
}

#[test]
fn test_split_by_lines_exact() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(2000);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(1000),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 2);
}

// ---- Integration: split by bytes (-b) ----

#[test]
fn test_split_by_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'A'; 3072]; // 3K = 3 * 1024 bytes
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let config = SplitConfig {
        mode: SplitMode::Bytes(1024),
        prefix: dir.path().join("chunk").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "chunk");
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].len(), 1024);
    assert_eq!(chunks[1].len(), 1024);
    assert_eq!(chunks[2].len(), 1024);
}

#[test]
fn test_split_by_bytes_not_exact() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'B'; 2500];
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let config = SplitConfig {
        mode: SplitMode::Bytes(1024),
        prefix: dir.path().join("chunk").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "chunk");
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].len(), 1024);
    assert_eq!(chunks[1].len(), 1024);
    assert_eq!(chunks[2].len(), 452);
}

// ---- Integration: split by number (-n) ----

#[test]
fn test_split_by_number() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'X'; 1000];
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let config = SplitConfig {
        mode: SplitMode::Number(5),
        prefix: dir.path().join("part").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "part");
    assert_eq!(chunks.len(), 5);

    // Total bytes must equal original
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    assert_eq!(total, 1000);

    // Each chunk should be 200 bytes (1000 / 5)
    assert_eq!(chunks[0].len(), 200);
    assert_eq!(chunks[4].len(), 200);
}

#[test]
fn test_split_by_number_uneven() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'Y'; 1003]; // 1003 / 5 = 200 remainder 3
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let config = SplitConfig {
        mode: SplitMode::Number(5),
        prefix: dir.path().join("part").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "part");
    assert_eq!(chunks.len(), 5);

    // First 3 chunks get 201 bytes, last 2 get 200
    assert_eq!(chunks[0].len(), 201);
    assert_eq!(chunks[1].len(), 201);
    assert_eq!(chunks[2].len(), 201);
    assert_eq!(chunks[3].len(), 200);
    assert_eq!(chunks[4].len(), 200);

    let total: usize = chunks.iter().map(|c| c.len()).sum();
    assert_eq!(total, 1003);
}

// ---- Integration: numeric suffixes (-d) ----

#[test]
fn test_split_numeric_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(30);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(10),
        suffix_type: SuffixType::Numeric(0),
        prefix: dir.path().join("out").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    // Check that numeric suffix files exist
    assert!(dir.path().join("out00").exists());
    assert!(dir.path().join("out01").exists());
    assert!(dir.path().join("out02").exists());
    assert!(!dir.path().join("out03").exists());
}

#[test]
fn test_split_numeric_suffixes_from() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(20);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(10),
        suffix_type: SuffixType::Numeric(5),
        prefix: dir.path().join("out").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    assert!(dir.path().join("out05").exists());
    assert!(dir.path().join("out06").exists());
    assert!(!dir.path().join("out07").exists());
}

// ---- Integration: custom prefix ----

#[test]
fn test_split_custom_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(20);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(10),
        prefix: dir.path().join("myprefix_").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    assert!(dir.path().join("myprefix_aa").exists());
    assert!(dir.path().join("myprefix_ab").exists());
    assert!(!dir.path().join("myprefix_ac").exists());
}

// ---- Integration: roundtrip (concatenation of chunks == original) ----

#[test]
fn test_split_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(2500);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(1000),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}

#[test]
fn test_split_roundtrip_bytes() {
    let dir = tempfile::tempdir().unwrap();
    // Use random-ish binary data
    let content: Vec<u8> = (0..10000u32).map(|i| (i % 256) as u8).collect();
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let config = SplitConfig {
        mode: SplitMode::Bytes(3333),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}

#[test]
fn test_split_roundtrip_number() {
    let dir = tempfile::tempdir().unwrap();
    let content: Vec<u8> = (0..7777u32).map(|i| (i % 256) as u8).collect();
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let config = SplitConfig {
        mode: SplitMode::Number(13),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}

// ---- Integration: line-bytes (-C) ----

#[test]
fn test_split_line_bytes() {
    let dir = tempfile::tempdir().unwrap();
    // Create lines of varying sizes
    let mut content = Vec::new();
    for i in 0..20 {
        let line = format!("line {:04}\n", i);
        content.extend_from_slice(line.as_bytes());
    }
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    // Each line is 10 bytes ("line XXXX\n")
    // -C 50 should fit about 5 lines per chunk
    let config = SplitConfig {
        mode: SplitMode::LineBytes(50),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");

    // Each chunk should be at most 50 bytes
    for chunk in &chunks {
        assert!(
            chunk.len() <= 50,
            "chunk size {} exceeds 50",
            chunk.len()
        );
    }

    // Roundtrip check
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}

#[test]
fn test_split_line_bytes_long_line() {
    let dir = tempfile::tempdir().unwrap();
    // One very long line exceeding the byte limit
    let mut content = Vec::new();
    content.extend_from_slice(b"short\n");
    content.extend_from_slice(&[b'A'; 200]);
    content.push(b'\n');
    content.extend_from_slice(b"after\n");
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::LineBytes(50),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    // Roundtrip must still work
    let chunks = read_chunks(dir.path(), "x");
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}

// ---- Integration: verbose output ----

#[test]
fn test_split_verbose() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(20);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("--verbose")
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("creating file"),
        "verbose output should contain 'creating file', got: {}",
        stderr
    );
}

// ---- Integration: elide empty files ----

#[test]
fn test_split_elide_empty() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'A'; 10];
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    // Split 10 bytes into 20 chunks with elide-empty
    let config = SplitConfig {
        mode: SplitMode::Number(20),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        elide_empty: true,
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    // With elide-empty, we should have at most 10 files (some may have 0 bytes)
    let chunk_count = count_chunks(dir.path(), "x");
    assert!(
        chunk_count <= 10,
        "with elide-empty, should have at most 10 non-empty chunks, got {}",
        chunk_count
    );
}

// ---- Integration: additional suffix ----

#[test]
fn test_split_additional_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(20);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(10),
        additional_suffix: ".part".to_string(),
        prefix: dir.path().join("data").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    assert!(dir.path().join("dataaa.part").exists());
    assert!(dir.path().join("dataab.part").exists());
}

// ---- Integration: hex suffixes ----

#[test]
fn test_split_hex_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(30);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(10),
        suffix_type: SuffixType::Hex(0),
        prefix: dir.path().join("out").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    assert!(dir.path().join("out00").exists());
    assert!(dir.path().join("out01").exists());
    assert!(dir.path().join("out02").exists());
    assert!(!dir.path().join("out03").exists());
}

// ---- Integration: suffix length ----

#[test]
fn test_split_suffix_length() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(20);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(10),
        suffix_length: 4,
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    assert!(dir.path().join("xaaaa").exists());
    assert!(dir.path().join("xaaab").exists());
}

// ---- Integration: empty input ----

#[test]
fn test_split_empty_input() {
    let dir = tempfile::tempdir().unwrap();
    let input_path = create_test_file(dir.path(), "empty.txt", b"");

    let config = SplitConfig {
        mode: SplitMode::Lines(1000),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 0);
}

// ---- Integration: single line ----

#[test]
fn test_split_single_line() {
    let dir = tempfile::tempdir().unwrap();
    let input_path = create_test_file(dir.path(), "input.txt", b"hello\n");

    let config = SplitConfig {
        mode: SplitMode::Lines(1000),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], b"hello\n");
}

// ---- Binary integration tests via fsplit ----

#[test]
fn test_binary_basic_lines() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(30);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 3);
}

#[test]
fn test_binary_bytes_mode() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'Z'; 5000];
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("-b")
        .arg("1K")
        .arg(input_path.to_str().unwrap())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 5); // 5000 / 1024 = 4.88, so 5 files
    assert_eq!(chunks[0].len(), 1024);
    assert_eq!(chunks[4].len(), 5000 - 4 * 1024);
}

#[test]
fn test_binary_number_mode() {
    let dir = tempfile::tempdir().unwrap();
    let content = vec![b'Q'; 1000];
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("-n")
        .arg("4")
        .arg(input_path.to_str().unwrap())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 4);
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    assert_eq!(total, 1000);
}

#[test]
fn test_binary_custom_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(20);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .arg("myprefix_")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(dir.path().join("myprefix_aa").exists());
    assert!(dir.path().join("myprefix_ab").exists());
}

#[test]
fn test_binary_numeric_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(30);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("-d")
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(dir.path().join("x00").exists());
    assert!(dir.path().join("x01").exists());
    assert!(dir.path().join("x02").exists());
}

#[test]
fn test_binary_hex_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(30);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("-x")
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(dir.path().join("x00").exists());
    assert!(dir.path().join("x01").exists());
    assert!(dir.path().join("x02").exists());
}

#[test]
fn test_binary_version() {
    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("split (fcoreutils)"));
}

#[test]
fn test_binary_help() {
    let output = std::process::Command::new(bin_path("fsplit"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("--bytes"));
    assert!(stdout.contains("--lines"));
}

// ---- GNU compatibility tests (Linux only) ----

#[test]
#[cfg(target_os = "linux")]
fn test_split_matches_gnu() {
    // Check if GNU split is available
    let gnu_check = std::process::Command::new("split")
        .arg("--version")
        .output();
    if gnu_check.is_err() {
        return; // GNU split not available, skip
    }
    let gnu_check = gnu_check.unwrap();
    if !String::from_utf8_lossy(&gnu_check.stdout).contains("GNU") {
        return; // Not GNU split
    }

    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(50);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    // Run GNU split
    let gnu_dir = dir.path().join("gnu");
    fs::create_dir(&gnu_dir).unwrap();
    let gnu_status = std::process::Command::new("split")
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .arg(gnu_dir.join("x").to_str().unwrap())
        .status()
        .unwrap();
    assert!(gnu_status.success());

    // Run our split
    let our_dir = dir.path().join("ours");
    fs::create_dir(&our_dir).unwrap();
    let our_status = std::process::Command::new(bin_path("fsplit"))
        .arg("-l")
        .arg("10")
        .arg(input_path.to_str().unwrap())
        .arg(our_dir.join("x").to_str().unwrap())
        .status()
        .unwrap();
    assert!(our_status.success());

    let gnu_chunks = read_chunks(&gnu_dir, "x");
    let our_chunks = read_chunks(&our_dir, "x");

    assert_eq!(
        gnu_chunks.len(),
        our_chunks.len(),
        "chunk count mismatch: GNU={}, ours={}",
        gnu_chunks.len(),
        our_chunks.len()
    );

    for (i, (gnu, ours)) in gnu_chunks.iter().zip(our_chunks.iter()).enumerate() {
        assert_eq!(
            gnu, ours,
            "chunk {} differs between GNU and ours",
            i
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_split_matches_gnu_bytes() {
    let gnu_check = std::process::Command::new("split")
        .arg("--version")
        .output();
    if gnu_check.is_err() {
        return;
    }
    let gnu_check = gnu_check.unwrap();
    if !String::from_utf8_lossy(&gnu_check.stdout).contains("GNU") {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let content: Vec<u8> = (0..5000u32).map(|i| (i % 256) as u8).collect();
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    let gnu_dir = dir.path().join("gnu");
    fs::create_dir(&gnu_dir).unwrap();
    let gnu_status = std::process::Command::new("split")
        .arg("-b")
        .arg("1024")
        .arg(input_path.to_str().unwrap())
        .arg(gnu_dir.join("x").to_str().unwrap())
        .status()
        .unwrap();
    assert!(gnu_status.success());

    let our_dir = dir.path().join("ours");
    fs::create_dir(&our_dir).unwrap();
    let our_status = std::process::Command::new(bin_path("fsplit"))
        .arg("-b")
        .arg("1024")
        .arg(input_path.to_str().unwrap())
        .arg(our_dir.join("x").to_str().unwrap())
        .status()
        .unwrap();
    assert!(our_status.success());

    let gnu_chunks = read_chunks(&gnu_dir, "x");
    let our_chunks = read_chunks(&our_dir, "x");

    assert_eq!(gnu_chunks.len(), our_chunks.len());
    for (i, (gnu, ours)) in gnu_chunks.iter().zip(our_chunks.iter()).enumerate() {
        assert_eq!(gnu, ours, "byte-split chunk {} differs", i);
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_split_matches_gnu_line_bytes() {
    let gnu_check = std::process::Command::new("split")
        .arg("--version")
        .output();
    if gnu_check.is_err() {
        return;
    }
    let gnu_check = gnu_check.unwrap();
    if !String::from_utf8_lossy(&gnu_check.stdout).contains("GNU") {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(50);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let gnu_dir = dir.path().join("gnu");
    fs::create_dir(&gnu_dir).unwrap();
    let gnu_status = std::process::Command::new("split")
        .arg("-C")
        .arg("100")
        .arg(input_path.to_str().unwrap())
        .arg(gnu_dir.join("x").to_str().unwrap())
        .status()
        .unwrap();
    assert!(gnu_status.success());

    let our_dir = dir.path().join("ours");
    fs::create_dir(&our_dir).unwrap();
    let our_status = std::process::Command::new(bin_path("fsplit"))
        .arg("-C")
        .arg("100")
        .arg(input_path.to_str().unwrap())
        .arg(our_dir.join("x").to_str().unwrap())
        .status()
        .unwrap();
    assert!(our_status.success());

    let gnu_chunks = read_chunks(&gnu_dir, "x");
    let our_chunks = read_chunks(&our_dir, "x");

    assert_eq!(
        gnu_chunks.len(),
        our_chunks.len(),
        "line-bytes chunk count: GNU={}, ours={}",
        gnu_chunks.len(),
        our_chunks.len()
    );
    for (i, (gnu, ours)) in gnu_chunks.iter().zip(our_chunks.iter()).enumerate() {
        assert_eq!(gnu, ours, "line-bytes chunk {} differs", i);
    }
}

// ---- Edge case: nonexistent input file ----

#[test]
fn test_split_nonexistent_file() {
    let config = SplitConfig::default();
    let result = split_file("/nonexistent/path/to/file.txt", &config);
    assert!(result.is_err());
}

// ---- Edge case: single byte input ----

#[test]
fn test_split_single_byte() {
    let dir = tempfile::tempdir().unwrap();
    let input_path = create_test_file(dir.path(), "input.bin", &[0x42]);

    let config = SplitConfig {
        mode: SplitMode::Bytes(1024),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], vec![0x42]);
}

// ---- Edge case: large number of small chunks ----

#[test]
fn test_split_many_small_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let content = generate_lines(100);
    let input_path = create_test_file(dir.path(), "input.txt", &content);

    let config = SplitConfig {
        mode: SplitMode::Lines(1),
        suffix_length: 3, // Support up to 17576 chunks
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 100);

    // Verify roundtrip
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}

// ---- Edge case: binary data with no newlines ----

#[test]
fn test_split_binary_no_newlines() {
    let dir = tempfile::tempdir().unwrap();
    let content: Vec<u8> = (0..256).map(|i| if i == 10 { 0 } else { i as u8 }).collect();
    let input_path = create_test_file(dir.path(), "input.bin", &content);

    // Split by bytes should work fine with no newlines
    let config = SplitConfig {
        mode: SplitMode::Bytes(64),
        prefix: dir.path().join("x").to_string_lossy().into_owned(),
        ..SplitConfig::default()
    };

    split_file(input_path.to_str().unwrap(), &config).unwrap();

    let chunks = read_chunks(dir.path(), "x");
    assert_eq!(chunks.len(), 4);

    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(chunk);
    }
    assert_eq!(reassembled, content);
}
