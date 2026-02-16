// fcksum — compute POSIX CRC-32 checksum and byte count (GNU cksum replacement)

use std::io::{self, Read, Write};
use std::process;

const TOOL_NAME: &str = "cksum";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// POSIX CRC-32 lookup table using polynomial 0x04C11DB7.
/// This is the standard table used by GNU cksum.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
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
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Compute the POSIX CRC-32 checksum.
/// Algorithm: for each byte, crc = (crc << 8) ^ table[((crc >> 24) ^ byte) & 0xFF]
/// Then feed the length bytes (big-endian), then complement.
fn posix_cksum(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;

    for &byte in data {
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) ^ u32::from(byte)) as usize];
    }

    // Feed length bytes (big-endian, only the significant bytes)
    let mut len = data.len() as u64;
    while len > 0 {
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) ^ (len & 0xFF) as u32) as usize];
        len >>= 8;
    }

    !crc
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
    let mut out = io::BufWriter::new(stdout.lock());
    let mut exit_code = 0;

    for filename in &cli.files {
        let data = if filename == "-" {
            let mut buf = Vec::new();
            if let Err(e) = io::stdin().lock().read_to_end(&mut buf) {
                eprintln!(
                    "{}: -: {}",
                    TOOL_NAME,
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
                continue;
            }
            buf
        } else {
            match std::fs::read(filename) {
                Ok(data) => data,
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

        let crc = posix_cksum(&data);
        let byte_count = data.len();

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
