use super::*;
use std::io::Cursor;

// ── Hash computation tests (reader path) ────────────────────────────

#[test]
fn test_sha256_empty() {
    let hash = hash_reader(HashAlgorithm::Sha256, Cursor::new(b"")).unwrap();
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn test_sha256_hello_newline() {
    // echo "hello" | sha256sum -> hash of "hello\n"
    let hash = hash_reader(HashAlgorithm::Sha256, Cursor::new(b"hello\n")).unwrap();
    assert_eq!(
        hash,
        "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );
}

#[test]
fn test_md5_empty() {
    let hash = hash_reader(HashAlgorithm::Md5, Cursor::new(b"")).unwrap();
    assert_eq!(hash, "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn test_md5_hello_newline() {
    let hash = hash_reader(HashAlgorithm::Md5, Cursor::new(b"hello\n")).unwrap();
    assert_eq!(hash, "b1946ac92492d2347c6235b4d2611184");
}

#[test]
fn test_blake2b_empty() {
    let hash = hash_reader(HashAlgorithm::Blake2b, Cursor::new(b"")).unwrap();
    assert_eq!(
        hash,
        "786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419\
         d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce"
    );
}

#[test]
fn test_blake2b_hello_newline() {
    let hash = hash_reader(HashAlgorithm::Blake2b, Cursor::new(b"hello\n")).unwrap();
    assert_eq!(
        hash,
        "f60ce482e5cc1229f39d71313171a8d9f4ca3a87d066bf4b205effb528192a75\
         f14f3271e2c1a90e1de53f275b4d4793eef2f5e31ea90d2ce29d2e481c36435f"
    );
}

// ── hash_bytes tests (zero-copy path) ───────────────────────────────

#[test]
fn test_hash_bytes_md5_empty() {
    let hash = hash_bytes(HashAlgorithm::Md5, b"");
    assert_eq!(hash, "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn test_hash_bytes_md5_hello() {
    let hash = hash_bytes(HashAlgorithm::Md5, b"hello\n");
    assert_eq!(hash, "b1946ac92492d2347c6235b4d2611184");
}

#[test]
fn test_hash_bytes_sha256_empty() {
    let hash = hash_bytes(HashAlgorithm::Sha256, b"");
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn test_hash_bytes_sha256_hello() {
    let hash = hash_bytes(HashAlgorithm::Sha256, b"hello\n");
    assert_eq!(
        hash,
        "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );
}

#[test]
fn test_hash_bytes_matches_reader() {
    // Verify hash_bytes and hash_reader produce identical output
    let data = b"The quick brown fox jumps over the lazy dog\n";
    for algo in [
        HashAlgorithm::Md5,
        HashAlgorithm::Sha256,
        HashAlgorithm::Blake2b,
    ] {
        let from_bytes = hash_bytes(algo, data);
        let from_reader = hash_reader(algo, Cursor::new(data)).unwrap();
        assert_eq!(from_bytes, from_reader, "Mismatch for {:?}", algo);
    }
}

// ── hex_encode tests ────────────────────────────────────────────────

#[test]
fn test_hex_encode() {
    assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
}

#[test]
fn test_hex_encode_empty() {
    assert_eq!(hex_encode(&[]), "");
}

#[test]
fn test_hex_encode_all_bytes() {
    assert_eq!(hex_encode(&[0x00]), "00");
    assert_eq!(hex_encode(&[0x0f]), "0f");
    assert_eq!(hex_encode(&[0xf0]), "f0");
    assert_eq!(hex_encode(&[0xff]), "ff");
    assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
}

// ── parse_check_line tests ──────────────────────────────────────────

#[test]
fn test_parse_check_line_standard() {
    let (hash, file) = parse_check_line("abc123  test.txt").unwrap();
    assert_eq!(hash, "abc123");
    assert_eq!(file, "test.txt");
}

#[test]
fn test_parse_check_line_binary() {
    let (hash, file) = parse_check_line("abc123 *test.txt").unwrap();
    assert_eq!(hash, "abc123");
    assert_eq!(file, "test.txt");
}

#[test]
fn test_parse_check_line_invalid() {
    assert!(parse_check_line("no valid format").is_none());
}

#[test]
fn test_parse_check_line_bsd_md5() {
    let (hash, file) =
        parse_check_line("MD5 (test.txt) = d41d8cd98f00b204e9800998ecf8427e").unwrap();
    assert_eq!(hash, "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(file, "test.txt");
}

#[test]
fn test_parse_check_line_bsd_sha256() {
    let (hash, file) = parse_check_line(
        "SHA256 (file.bin) = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    )
    .unwrap();
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(file, "file.bin");
}

#[test]
fn test_parse_check_line_bsd_blake2b() {
    let (hash, file) = parse_check_line("BLAKE2b (data) = abcdef0123456789").unwrap();
    assert_eq!(hash, "abcdef0123456789");
    assert_eq!(file, "data");
}

#[test]
fn test_parse_check_line_backslash_escaped() {
    let (hash, file) = parse_check_line("\\abc123  test.txt").unwrap();
    assert_eq!(hash, "abc123");
    assert_eq!(file, "test.txt");
}

#[test]
fn test_parse_check_line_with_spaces_in_filename() {
    let (hash, file) = parse_check_line("abc123  my file.txt").unwrap();
    assert_eq!(hash, "abc123");
    assert_eq!(file, "my file.txt");
}

#[test]
fn test_parse_check_line_bsd_with_spaces_in_filename() {
    let (hash, file) = parse_check_line("SHA256 (my file.txt) = abc123def456").unwrap();
    assert_eq!(hash, "abc123def456");
    assert_eq!(file, "my file.txt");
}

// ── Output format tests ─────────────────────────────────────────────

#[test]
fn test_print_hash_text_mode() {
    let mut buf = Vec::new();
    print_hash(&mut buf, "abcdef", "test.txt", false).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), "abcdef  test.txt\n");
}

#[test]
fn test_print_hash_binary_mode() {
    let mut buf = Vec::new();
    print_hash(&mut buf, "abcdef", "test.txt", true).unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), "abcdef *test.txt\n");
}

#[test]
fn test_print_hash_tag() {
    let mut buf = Vec::new();
    print_hash_tag(&mut buf, HashAlgorithm::Sha256, "abcdef", "test.txt").unwrap();
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "SHA256 (test.txt) = abcdef\n"
    );
}

#[test]
fn test_print_hash_zero() {
    let mut buf = Vec::new();
    print_hash_zero(&mut buf, "abcdef", "test.txt", false).unwrap();
    assert_eq!(buf, b"abcdef  test.txt\0");
}

#[test]
fn test_print_hash_zero_binary() {
    let mut buf = Vec::new();
    print_hash_zero(&mut buf, "abcdef", "test.txt", true).unwrap();
    assert_eq!(buf, b"abcdef *test.txt\0");
}

#[test]
fn test_print_hash_tag_zero() {
    let mut buf = Vec::new();
    print_hash_tag_zero(&mut buf, HashAlgorithm::Sha256, "abcdef", "test.txt").unwrap();
    assert_eq!(buf, b"SHA256 (test.txt) = abcdef\0");
}

#[test]
fn test_print_hash_tag_md5() {
    let mut buf = Vec::new();
    print_hash_tag(&mut buf, HashAlgorithm::Md5, "abcdef", "test.txt").unwrap();
    assert_eq!(String::from_utf8(buf).unwrap(), "MD5 (test.txt) = abcdef\n");
}

#[test]
fn test_print_hash_tag_blake2b() {
    let mut buf = Vec::new();
    print_hash_tag(&mut buf, HashAlgorithm::Blake2b, "abcdef", "test.txt").unwrap();
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "BLAKE2b (test.txt) = abcdef\n"
    );
}

// ── hash_file tests ─────────────────────────────────────────────────

#[test]
fn test_hash_file_md5() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.txt");
    std::fs::write(&path, b"hello\n").unwrap();
    let hash = hash_file(HashAlgorithm::Md5, &path).unwrap();
    assert_eq!(hash, "b1946ac92492d2347c6235b4d2611184");
}

#[test]
fn test_hash_file_sha256() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.txt");
    std::fs::write(&path, b"hello\n").unwrap();
    let hash = hash_file(HashAlgorithm::Sha256, &path).unwrap();
    assert_eq!(
        hash,
        "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );
}

#[test]
fn test_hash_file_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.txt");
    std::fs::write(&path, b"").unwrap();
    let hash = hash_file(HashAlgorithm::Md5, &path).unwrap();
    assert_eq!(hash, "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn test_hash_file_large() {
    // Test a file large enough to trigger mmap path (>64KB)
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.bin");
    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    std::fs::write(&path, &data).unwrap();

    let file_hash = hash_file(HashAlgorithm::Md5, &path).unwrap();
    let bytes_hash = hash_bytes(HashAlgorithm::Md5, &data);
    assert_eq!(file_hash, bytes_hash);
}

#[test]
fn test_hash_file_large_sha256() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.bin");
    let data: Vec<u8> = (0..128 * 1024).map(|i| (i % 256) as u8).collect();
    std::fs::write(&path, &data).unwrap();

    let file_hash = hash_file(HashAlgorithm::Sha256, &path).unwrap();
    let bytes_hash = hash_bytes(HashAlgorithm::Sha256, &data);
    assert_eq!(file_hash, bytes_hash);
}

#[test]
fn test_hash_file_nonexistent() {
    let result = hash_file(
        HashAlgorithm::Md5,
        std::path::Path::new("/nonexistent/file"),
    );
    assert!(result.is_err());
}

// ── check_file tests ────────────────────────────────────────────────

#[test]
fn test_check_file_ok() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let hash = hash_file(HashAlgorithm::Sha256, &file_path).unwrap();
    let check_content = format!("{}  {}\n", hash, file_path.display());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 1);
    assert_eq!(r.mismatches, 0);
    assert_eq!(r.format_errors, 0);
    assert_eq!(r.read_errors, 0);
    assert!(String::from_utf8(out).unwrap().contains("OK"));
}

#[test]
fn test_check_file_fail() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let check_content = format!(
        "0000000000000000000000000000000000000000000000000000000000000000  {}\n",
        file_path.display()
    );

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 0);
    assert_eq!(r.mismatches, 1);
    assert!(String::from_utf8(out).unwrap().contains("FAILED"));
}

#[test]
fn test_check_file_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let hash = hash_file(HashAlgorithm::Sha256, &file_path).unwrap();
    let check_content = format!("{}  {}\n", hash, file_path.display());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: true,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 1);
    // Quiet mode suppresses "OK" output
    assert!(String::from_utf8(out).unwrap().is_empty());
}

#[test]
fn test_check_file_status_only() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let check_content = format!(
        "0000000000000000000000000000000000000000000000000000000000000000  {}\n",
        file_path.display()
    );

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: true,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.mismatches, 1);
    // Status-only mode: no output at all
    assert!(String::from_utf8(out).unwrap().is_empty());
}

#[test]
fn test_check_file_ignore_missing() {
    let input = "d41d8cd98f00b204e9800998ecf8427e  /nonexistent/missing/file\n";
    let reader = Cursor::new(input.as_bytes());
    let mut out = Vec::new();
    let mut err_out = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: true,
        warn_prefix: String::new(),
    };
    let r = check_file(HashAlgorithm::Md5, reader, &opts, &mut out, &mut err_out).unwrap();
    assert_eq!(r.ok, 0);
    assert_eq!(r.mismatches, 0);
    assert_eq!(r.read_errors, 0);
    assert_eq!(r.format_errors, 0);
    assert_eq!(r.ignored_missing, 1);
    // No output for missing files when ignore_missing is true
    assert!(String::from_utf8(out).unwrap().is_empty());
}

#[test]
fn test_check_file_missing_not_ignored() {
    let input = "d41d8cd98f00b204e9800998ecf8427e  /nonexistent/missing/file\n";
    let reader = Cursor::new(input.as_bytes());
    let mut out = Vec::new();
    let mut err_out = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };
    let r = check_file(HashAlgorithm::Md5, reader, &opts, &mut out, &mut err_out).unwrap();
    assert_eq!(r.ok, 0);
    assert_eq!(r.read_errors, 1);
    // "FAILED open or read" goes to stdout
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("FAILED open or read"));
    // Detailed error goes to stderr
    let err_str = String::from_utf8(err_out).unwrap();
    assert!(!err_str.is_empty());
}

#[test]
fn test_check_file_format_errors_with_warn() {
    let input = "not a valid line\n";
    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: true,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(input.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.format_errors, 1);
    let err_str = String::from_utf8(err).unwrap();
    assert!(err_str.contains("improperly formatted SHA256 checksum line"));
}

#[test]
fn test_check_file_strict_format_errors() {
    let input = "not a valid line\n";
    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: true,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(input.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.format_errors, 1);
    // Strict mode: format errors tracked separately, caller decides exit code
    assert_eq!(r.mismatches, 0);
}

#[test]
fn test_check_file_bsd_tag_format() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let hash = hash_file(HashAlgorithm::Sha256, &file_path).unwrap();
    let check_content = format!("SHA256 ({}) = {}\n", file_path.display(), hash);

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 1);
    assert_eq!(r.mismatches, 0);
    assert_eq!(r.format_errors, 0);
}

#[test]
fn test_check_file_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    // Use UPPERCASE hex
    let hash = hash_file(HashAlgorithm::Sha256, &file_path)
        .unwrap()
        .to_uppercase();
    let check_content = format!("{}  {}\n", hash, file_path.display());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 1);
    assert_eq!(r.mismatches, 0);
}

#[test]
fn test_check_file_multiple_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path1 = dir.path().join("a.txt");
    let path2 = dir.path().join("b.txt");
    std::fs::write(&path1, b"aaa\n").unwrap();
    std::fs::write(&path2, b"bbb\n").unwrap();

    let hash1 = hash_file(HashAlgorithm::Sha256, &path1).unwrap();
    let hash2 = hash_file(HashAlgorithm::Sha256, &path2).unwrap();
    let check_content = format!(
        "{}  {}\n{}  {}\n",
        hash1,
        path1.display(),
        hash2,
        path2.display()
    );

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 2);
    assert_eq!(r.mismatches, 0);
    assert_eq!(r.format_errors, 0);
}

#[test]
fn test_check_file_empty_lines_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, b"hello\n").unwrap();

    let hash = hash_file(HashAlgorithm::Sha256, &file_path).unwrap();
    let check_content = format!("\n\n{}  {}\n\n", hash, file_path.display());

    let mut out = Vec::new();
    let mut err = Vec::new();
    let opts = CheckOptions {
        quiet: false,
        status_only: false,
        strict: false,
        warn: false,
        ignore_missing: false,
        warn_prefix: String::new(),
    };

    let r = check_file(
        HashAlgorithm::Sha256,
        Cursor::new(check_content.as_bytes()),
        &opts,
        &mut out,
        &mut err,
    )
    .unwrap();

    assert_eq!(r.ok, 1);
    assert_eq!(r.mismatches, 0);
    assert_eq!(r.format_errors, 0);
}

// ── Algorithm name tests ────────────────────────────────────────────

#[test]
fn test_algorithm_names() {
    assert_eq!(HashAlgorithm::Md5.name(), "MD5");
    assert_eq!(HashAlgorithm::Sha256.name(), "SHA256");
    assert_eq!(HashAlgorithm::Blake2b.name(), "BLAKE2b");
}
