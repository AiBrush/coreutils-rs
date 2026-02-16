// fcksum — compute POSIX CRC-32 checksum and byte count (GNU cksum replacement)

use std::io::{self, BufRead, Read, Write};
use std::process;

const TOOL_NAME: &str = "cksum";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// POSIX CRC-32 slicing-by-4 lookup tables using polynomial 0x04C11DB7.
/// Table 0 is the standard byte-at-a-time table; tables 1-3 enable processing
/// 4 bytes per iteration for ~4x throughput improvement.
const CRC_TABLES: [[u32; 256]; 4] = {
    let mut tables = [[0u32; 256]; 4];
    // Build the base table (table 0)
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i << 24;
        let mut j = 0;
        while j < 8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
            j += 1;
        }
        tables[0][i as usize] = crc;
        i += 1;
    }
    // Build extended tables for slicing-by-4
    let mut t = 1;
    while t < 4 {
        let mut i = 0;
        while i < 256 {
            let prev = tables[t - 1][i];
            tables[t][i] = (prev << 8) ^ tables[0][(prev >> 24) as usize];
            i += 1;
        }
        t += 1;
    }
    tables
};

/// Backward-compatible alias for tests that reference CRC_TABLE
#[cfg(test)]
const CRC_TABLE: [u32; 256] = CRC_TABLES[0];

/// Compute the POSIX CRC-32 checksum using slicing-by-4 for high throughput.
/// Processes 4 bytes per iteration in the main loop (~4x faster than byte-at-a-time).
#[cfg(test)]
fn posix_cksum(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;

    // Slicing-by-4: process 4 bytes per iteration
    let chunks = data.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let b0 = chunk[0];
        let b1 = chunk[1];
        let b2 = chunk[2];
        let b3 = chunk[3];
        crc = CRC_TABLES[3][((crc >> 24) ^ u32::from(b0)) as usize]
            ^ CRC_TABLES[2][((crc >> 16) as u8 ^ b1) as usize]
            ^ CRC_TABLES[1][((crc >> 8) as u8 ^ b2) as usize]
            ^ CRC_TABLES[0][(crc as u8 ^ b3) as usize];
    }

    // Process remaining bytes one at a time
    for &byte in remainder {
        crc = (crc << 8) ^ CRC_TABLES[0][((crc >> 24) ^ u32::from(byte)) as usize];
    }

    // Feed length bytes (big-endian, only the significant bytes)
    let mut len = data.len() as u64;
    while len > 0 {
        crc = (crc << 8) ^ CRC_TABLES[0][((crc >> 24) ^ (len & 0xFF) as u32) as usize];
        len >>= 8;
    }

    !crc
}

/// Streaming POSIX CRC-32: process data from a reader without loading everything into memory.
/// Uses 8MB buffer and slicing-by-4 for maximum throughput.
fn posix_cksum_streaming<R: Read>(reader: R) -> io::Result<(u32, u64)> {
    let mut reader = io::BufReader::with_capacity(8 * 1024 * 1024, reader);
    let mut crc: u32 = 0;
    let mut total_bytes: u64 = 0;

    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            break;
        }
        let n = buf.len();
        total_bytes += n as u64;

        // Slicing-by-4 on the buffer
        let chunks = buf.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            crc = CRC_TABLES[3][((crc >> 24) ^ u32::from(chunk[0])) as usize]
                ^ CRC_TABLES[2][((crc >> 16) as u8 ^ chunk[1]) as usize]
                ^ CRC_TABLES[1][((crc >> 8) as u8 ^ chunk[2]) as usize]
                ^ CRC_TABLES[0][(crc as u8 ^ chunk[3]) as usize];
        }
        for &byte in remainder {
            crc = (crc << 8) ^ CRC_TABLES[0][((crc >> 24) ^ u32::from(byte)) as usize];
        }

        reader.consume(n);
    }

    // Feed length bytes
    let mut len = total_bytes;
    while len > 0 {
        crc = (crc << 8) ^ CRC_TABLES[0][((crc >> 24) ^ (len & 0xFF) as u32) as usize];
        len >>= 8;
    }

    Ok((!crc, total_bytes))
}

struct Cli {
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli { files: Vec::new() };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            for f in args.by_ref() {
                cli.files.push(f.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            match bytes {
                b"--help" => {
                    print!(
                        "Usage: {} [FILE]...\n\
                         Print CRC checksum and byte counts of each FILE.\n\n\
                         With no FILE, or when FILE is -, read standard input.\n\n\
                         \x20     --help       display this help and exit\n\
                         \x20     --version    output version information and exit\n",
                        TOOL_NAME
                    );
                    process::exit(0);
                }
                b"--version" => {
                    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                    process::exit(0);
                }
                _ => {
                    eprintln!(
                        "{}: unrecognized option '{}'",
                        TOOL_NAME,
                        arg.to_string_lossy()
                    );
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // cksum doesn't have short options
            eprintln!(
                "{}: invalid option -- '{}'",
                TOOL_NAME,
                arg.to_string_lossy()
            );
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    if cli.files.is_empty() {
        cli.files.push("-".to_string());
    }

    cli
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();
    let stdout = io::stdout();
    let mut out = io::BufWriter::with_capacity(256 * 1024, stdout.lock());
    let mut exit_code = 0;

    for filename in &cli.files {
        let (crc, byte_count) = if filename == "-" {
            match posix_cksum_streaming(io::stdin().lock()) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "{}: -: {}",
                        TOOL_NAME,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    exit_code = 1;
                    continue;
                }
            }
        } else {
            match std::fs::File::open(filename) {
                Ok(file) => match posix_cksum_streaming(file) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "{}: {}: {}",
                            TOOL_NAME,
                            filename,
                            coreutils_rs::common::io_error_msg(&e)
                        );
                        exit_code = 1;
                        continue;
                    }
                },
                Err(e) => {
                    eprintln!(
                        "{}: {}: {}",
                        TOOL_NAME,
                        filename,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    exit_code = 1;
                    continue;
                }
            }
        };

        let result = if filename == "-" {
            writeln!(out, "{} {}", crc, byte_count)
        } else {
            writeln!(out, "{} {} {}", crc, byte_count, filename)
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("{}: write error: {}", TOOL_NAME, e);
            process::exit(1);
        }
    }

    if let Err(e) = out.flush()
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("{}: write error: {}", TOOL_NAME, e);
        process::exit(1);
    }

    process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fcksum");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("CRC"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("cksum"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_crc_table_correctness() {
        // Verify first and last entries of the CRC table
        assert_eq!(CRC_TABLE[0], 0);
        assert_ne!(CRC_TABLE[255], 0);
    }

    #[test]
    fn test_posix_cksum_empty() {
        // Empty input: CRC feeds only length (0), so only complement of 0
        let crc = posix_cksum(b"");
        assert_eq!(crc, 4294967295); // !0 = 0xFFFFFFFF
    }

    #[test]
    fn test_posix_cksum_hello() {
        // Known POSIX CRC for "hello\n"
        // GNU cksum gives: 3015617425 6
        let crc = posix_cksum(b"hello\n");
        assert_eq!(crc, 3015617425);
    }

    #[test]
    fn test_stdin() {
        let mut child = cmd()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"hello\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        assert_eq!(parts.len(), 2, "stdin should have no filename");
        assert_eq!(parts[0], "3015617425");
        assert_eq!(parts[1], "6");
    }

    #[test]
    fn test_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello\n").unwrap();

        let output = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        assert_eq!(parts.len(), 3, "file should include filename");
        assert_eq!(parts[0], "3015617425");
        assert_eq!(parts[1], "6");
    }

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = dir.path().join("a.txt");
        let file2 = dir.path().join("b.txt");
        std::fs::write(&file1, b"hello\n").unwrap();
        std::fs::write(&file2, b"world\n").unwrap();

        let output = cmd()
            .arg(file1.to_str().unwrap())
            .arg(file2.to_str().unwrap())
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 2, "should output one line per file");
    }

    #[test]
    fn test_nonexistent_file() {
        let output = cmd().arg("/nonexistent/file.txt").output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cksum:"));
    }

    #[test]
    fn test_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::write(&file_path, b"").unwrap();

        let output = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        assert_eq!(parts[0], "4294967295");
        assert_eq!(parts[1], "0");
    }

    #[test]
    fn test_large_data() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("large.bin");
        // Create a 10KB file of zeros
        let data = vec![0u8; 10240];
        std::fs::write(&file_path, &data).unwrap();

        let output = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        let _crc: u32 = parts[0].parse().expect("CRC should be numeric");
        let byte_count: u64 = parts[1].parse().expect("byte count should be numeric");
        assert_eq!(byte_count, 10240);
    }

    #[test]
    fn test_compare_gnu_cksum() {
        let gnu = Command::new("cksum").output();
        if let Ok(_gnu_output) = gnu {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("test.txt");
            std::fs::write(&file_path, b"The quick brown fox jumps over the lazy dog\n").unwrap();

            let gnu_out = Command::new("cksum")
                .arg(file_path.to_str().unwrap())
                .output();
            if let Ok(gnu_out) = gnu_out {
                let ours = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
                assert_eq!(
                    String::from_utf8_lossy(&ours.stdout),
                    String::from_utf8_lossy(&gnu_out.stdout),
                    "CRC mismatch with GNU cksum"
                );
            }
        }
    }

    #[test]
    fn test_compare_gnu_cksum_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::write(&file_path, b"").unwrap();

        let gnu_out = Command::new("cksum")
            .arg(file_path.to_str().unwrap())
            .output();
        if let Ok(gnu_out) = gnu_out {
            let ours = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&gnu_out.stdout),
                "Empty file CRC mismatch with GNU cksum"
            );
        }
    }
}
