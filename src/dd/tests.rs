use std::io::Write;
use std::process::{Command, Stdio};

use super::*;

/// Helper: build the path to the fdd binary.
fn fdd_cmd() -> Command {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fdd");
    Command::new(path)
}

/// Helper: run dd with the given operands and optional stdin, return (stdout, stderr, exit_code).
fn run_dd(operands: &[&str], stdin_data: Option<&[u8]>) -> (Vec<u8>, String, i32) {
    let mut child = fdd_cmd()
        .args(operands)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn fdd");

    if let Some(data) = stdin_data {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(data)
            .expect("failed to write stdin");
    }

    let output = child.wait_with_output().expect("failed to wait for fdd");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(-1);
    (output.stdout, stderr, code)
}

#[test]
fn test_dd_simple_copy() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.txt");
    let outfile = dir.path().join("output.txt");

    std::fs::write(&infile, b"Hello, dd world!\n").unwrap();

    let (stdout, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
        ],
        None,
    );

    assert_eq!(code, 0, "dd should exit 0, stderr: {}", stderr);
    assert!(stdout.is_empty(), "stdout should be empty when of= is used");

    let output_data = std::fs::read(&outfile).unwrap();
    assert_eq!(output_data, b"Hello, dd world!\n");

    // Status should show records
    assert!(stderr.contains("records in"), "stderr: {}", stderr);
    assert!(stderr.contains("records out"), "stderr: {}", stderr);
}

#[test]
fn test_dd_block_size() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.bin");
    let outfile = dir.path().join("output.bin");

    // Create input data: 2048 bytes
    let data: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    std::fs::write(&infile, &data).unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "bs=1024",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read(&outfile).unwrap();
    assert_eq!(output_data, data);

    // Should show 2+0 records in/out with bs=1024 for 2048 bytes
    assert!(stderr.contains("2+0 records in"), "stderr: {}", stderr);
    assert!(stderr.contains("2+0 records out"), "stderr: {}", stderr);
}

#[test]
fn test_dd_count() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.bin");
    let outfile = dir.path().join("output.bin");

    // Create 4 blocks of 512 bytes
    let data = vec![0xABu8; 2048];
    std::fs::write(&infile, &data).unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "count=2",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read(&outfile).unwrap();
    // count=2 with default ibs=512 means 1024 bytes
    assert_eq!(output_data.len(), 1024);
    assert!(output_data.iter().all(|&b| b == 0xAB));

    assert!(stderr.contains("2+0 records in"), "stderr: {}", stderr);
}

#[test]
fn test_dd_skip() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.bin");
    let outfile = dir.path().join("output.bin");

    // 3 blocks of 512 bytes: block0=0xAA, block1=0xBB, block2=0xCC
    let mut data = Vec::new();
    data.extend(vec![0xAAu8; 512]);
    data.extend(vec![0xBBu8; 512]);
    data.extend(vec![0xCCu8; 512]);
    std::fs::write(&infile, &data).unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "skip=1",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read(&outfile).unwrap();
    assert_eq!(output_data.len(), 1024);
    // First 512 bytes should be 0xBB (skipped block0)
    assert!(output_data[..512].iter().all(|&b| b == 0xBB));
    // Next 512 bytes should be 0xCC
    assert!(output_data[512..].iter().all(|&b| b == 0xCC));
}

#[test]
fn test_dd_seek() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.bin");
    let outfile = dir.path().join("output.bin");

    let data = vec![0xFFu8; 512];
    std::fs::write(&infile, &data).unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "seek=1",
            "conv=notrunc",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read(&outfile).unwrap();
    // seek=1 with obs=512: first 512 bytes are zeros, next 512 are 0xFF
    assert_eq!(output_data.len(), 1024);
    assert!(
        output_data[..512].iter().all(|&b| b == 0),
        "First block should be zeros from seek"
    );
    assert!(
        output_data[512..].iter().all(|&b| b == 0xFF),
        "Second block should be the input data"
    );
}

#[test]
fn test_dd_conv_ucase() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.txt");
    let outfile = dir.path().join("output.txt");

    std::fs::write(&infile, b"hello world\n").unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "conv=ucase",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read_to_string(&outfile).unwrap();
    assert_eq!(output_data, "HELLO WORLD\n");
}

#[test]
fn test_dd_conv_lcase() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.txt");
    let outfile = dir.path().join("output.txt");

    std::fs::write(&infile, b"HELLO WORLD\n").unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "conv=lcase",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read_to_string(&outfile).unwrap();
    assert_eq!(output_data, "hello world\n");
}

#[test]
fn test_dd_conv_swab() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.bin");
    let outfile = dir.path().join("output.bin");

    // Even number of bytes for clean swab
    std::fs::write(&infile, b"abcdef").unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "conv=swab",
        ],
        None,
    );

    assert_eq!(code, 0, "stderr: {}", stderr);

    let output_data = std::fs::read(&outfile).unwrap();
    assert_eq!(&output_data, b"badcfe");
}

#[test]
fn test_dd_status_none() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("input.txt");
    let outfile = dir.path().join("output.txt");

    std::fs::write(&infile, b"silent\n").unwrap();

    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", outfile.display()),
            "status=none",
        ],
        None,
    );

    assert_eq!(code, 0);
    assert!(
        stderr.is_empty(),
        "status=none should produce no stderr, got: {}",
        stderr
    );

    let output_data = std::fs::read_to_string(&outfile).unwrap();
    assert_eq!(output_data, "silent\n");
}

#[test]
fn test_dd_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let infile = dir.path().join("original.bin");
    let midfile = dir.path().join("mid.bin");
    let outfile = dir.path().join("roundtrip.bin");

    // Create a non-trivial binary file
    let data: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    std::fs::write(&infile, &data).unwrap();

    // Copy original -> mid
    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", infile.display()),
            &format!("of={}", midfile.display()),
            "bs=256",
        ],
        None,
    );
    assert_eq!(code, 0, "first copy failed: {}", stderr);

    // Copy mid -> roundtrip
    let (_, stderr, code) = run_dd(
        &[
            &format!("if={}", midfile.display()),
            &format!("of={}", outfile.display()),
            "bs=1024",
        ],
        None,
    );
    assert_eq!(code, 0, "second copy failed: {}", stderr);

    let original = std::fs::read(&infile).unwrap();
    let roundtripped = std::fs::read(&outfile).unwrap();
    assert_eq!(original, roundtripped, "roundtrip data mismatch");
}

#[test]
fn test_dd_matches_gnu() {
    // Only run if GNU dd is available
    let gnu_result = Command::new("dd")
        .args(["if=/dev/zero", "bs=512", "count=4", "status=none"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let gnu_output = match gnu_result {
        Ok(o) if o.status.success() => o,
        _ => return, // GNU dd not available, skip
    };

    let our_output = fdd_cmd()
        .args(["if=/dev/zero", "bs=512", "count=4", "status=none"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run fdd");

    assert_eq!(
        our_output.stdout, gnu_output.stdout,
        "output data mismatch with GNU dd"
    );
    assert_eq!(
        our_output.status.code(),
        gnu_output.status.code(),
        "exit code mismatch with GNU dd"
    );
}

// ---- Unit tests for parse_size ----

#[test]
fn test_parse_size_plain() {
    assert_eq!(parse_size("512").unwrap(), 512);
    assert_eq!(parse_size("0").unwrap(), 0);
    assert_eq!(parse_size("1").unwrap(), 1);
}

#[test]
fn test_parse_size_suffixes() {
    assert_eq!(parse_size("1c").unwrap(), 1);
    assert_eq!(parse_size("1w").unwrap(), 2);
    assert_eq!(parse_size("1b").unwrap(), 512);
    assert_eq!(parse_size("1K").unwrap(), 1000);
    assert_eq!(parse_size("1k").unwrap(), 1024);
    assert_eq!(parse_size("1kB").unwrap(), 1000);
    assert_eq!(parse_size("1KiB").unwrap(), 1024);
    assert_eq!(parse_size("1M").unwrap(), 1_000_000);
    assert_eq!(parse_size("1MiB").unwrap(), 1_048_576);
    assert_eq!(parse_size("1G").unwrap(), 1_000_000_000);
    assert_eq!(parse_size("1GiB").unwrap(), 1_073_741_824);
}

#[test]
fn test_parse_size_errors() {
    assert!(parse_size("").is_err());
    assert!(parse_size("abc").is_err());
    assert!(parse_size("1X").is_err());
}

// ---- Unit tests for parse_dd_args ----

#[test]
fn test_parse_dd_args_basic() {
    let args: Vec<String> = vec![
        "if=input.txt".to_string(),
        "of=output.txt".to_string(),
        "bs=4096".to_string(),
        "count=10".to_string(),
    ];
    let config = parse_dd_args(&args).unwrap();
    assert_eq!(config.input.as_deref(), Some("input.txt"));
    assert_eq!(config.output.as_deref(), Some("output.txt"));
    assert_eq!(config.ibs, 4096);
    assert_eq!(config.obs, 4096);
    assert_eq!(config.count, Some(10));
}

#[test]
fn test_parse_dd_args_conv() {
    let args: Vec<String> = vec!["conv=ucase,sync,notrunc".to_string()];
    let config = parse_dd_args(&args).unwrap();
    assert!(config.conv.ucase);
    assert!(config.conv.sync);
    assert!(config.conv.notrunc);
    assert!(!config.conv.lcase);
}

#[test]
fn test_parse_dd_args_mutually_exclusive() {
    let args: Vec<String> = vec!["conv=lcase,ucase".to_string()];
    assert!(parse_dd_args(&args).is_err());

    let args: Vec<String> = vec!["conv=excl,nocreat".to_string()];
    assert!(parse_dd_args(&args).is_err());
}

// ---- Unit tests for apply_conversions ----

#[test]
fn test_apply_conversions_ucase() {
    let conv = DdConv {
        ucase: true,
        ..Default::default()
    };
    let result = apply_conversions(b"hello 123", &conv);
    assert_eq!(&result, b"HELLO 123");
}

#[test]
fn test_apply_conversions_lcase() {
    let conv = DdConv {
        lcase: true,
        ..Default::default()
    };
    let result = apply_conversions(b"HELLO 123", &conv);
    assert_eq!(&result, b"hello 123");
}

#[test]
fn test_apply_conversions_swab() {
    let conv = DdConv {
        swab: true,
        ..Default::default()
    };
    let result = apply_conversions(b"abcdef", &conv);
    assert_eq!(&result, b"badcfe");

    // Odd length: last byte stays
    let result = apply_conversions(b"abcde", &conv);
    assert_eq!(&result, b"badce");
}
