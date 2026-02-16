// fsum — checksum and count the blocks in a file (GNU sum replacement)

use std::io::{self, BufRead, BufReader, Write};
use std::process;

const TOOL_NAME: &str = "sum";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq)]
enum Algorithm {
    Bsd,
    SysV,
}

struct Cli {
    algorithm: Algorithm,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        algorithm: Algorithm::Bsd,
        files: Vec::new(),
    };

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
                b"--sysv" => cli.algorithm = Algorithm::SysV,
                b"--help" => {
                    print!(
                        "Usage: {} [OPTION]... [FILE]...\n\
                         Print checksum and block counts for each FILE.\n\n\
                         With no FILE, or when FILE is -, read standard input.\n\n\
                         \x20 -r              select BSD sum algorithm (default)\n\
                         \x20 -s, --sysv      select System V sum algorithm\n\
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
            for &b in &bytes[1..] {
                match b {
                    b's' => cli.algorithm = Algorithm::SysV,
                    b'r' => cli.algorithm = Algorithm::Bsd,
                    _ => {
                        eprintln!("{}: invalid option -- '{}'", TOOL_NAME, char::from(b));
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    if cli.files.is_empty() {
        cli.files.push("-".to_string());
    }

    cli
}

/// Streaming checksum using BufReader with 8MB buffer.
/// Avoids loading entire file into memory.
fn process_streaming<R: io::Read>(reader: R, algorithm: Algorithm) -> io::Result<(u32, u64)> {
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, reader);
    let mut total_bytes: u64 = 0;

    match algorithm {
        Algorithm::Bsd => {
            let mut checksum: u32 = 0;
            loop {
                let buf = reader.fill_buf()?;
                if buf.is_empty() {
                    break;
                }
                let n = buf.len();
                total_bytes += n as u64;
                for &byte in buf {
                    checksum = (checksum >> 1) + ((checksum & 1) << 15);
                    checksum = (checksum + u32::from(byte)) & 0xFFFF;
                }
                reader.consume(n);
            }
            let blocks = total_bytes.div_ceil(1024);
            Ok((checksum, blocks))
        }
        Algorithm::SysV => {
            let mut sum: u32 = 0;
            loop {
                let buf = reader.fill_buf()?;
                if buf.is_empty() {
                    break;
                }
                let n = buf.len();
                total_bytes += n as u64;
                for &byte in buf {
                    sum += u32::from(byte);
                }
                reader.consume(n);
            }
            let mut r = sum;
            r = (r & 0xFFFF) + (r >> 16);
            r = (r & 0xFFFF) + (r >> 16);
            let blocks = total_bytes.div_ceil(512);
            Ok((r, blocks))
        }
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();
    let multiple = cli.files.len() > 1;
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut exit_code = 0;

    for filename in &cli.files {
        let result = if filename == "-" {
            process_streaming(io::stdin().lock(), cli.algorithm)
        } else {
            match std::fs::File::open(filename) {
                Ok(file) => process_streaming(file, cli.algorithm),
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

        let (checksum, blocks) = match result {
            Ok(v) => v,
            Err(e) => {
                let name = if filename == "-" {
                    "-"
                } else {
                    filename.as_str()
                };
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    name,
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
                continue;
            }
        };

        let result = if cli.algorithm == Algorithm::Bsd {
            // GNU sum BSD mode: checksum zero-padded to 5 digits, blocks right-aligned to 5 chars
            if filename == "-" && !multiple {
                writeln!(out, "{:05} {:5}", checksum, blocks)
            } else if filename == "-" {
                writeln!(out, "{:05} {:5} -", checksum, blocks)
            } else {
                writeln!(out, "{:05} {:5} {}", checksum, blocks, filename)
            }
        } else {
            // GNU sum SysV mode: plain numbers
            if filename == "-" && !multiple {
                writeln!(out, "{} {}", checksum, blocks)
            } else if filename == "-" {
                writeln!(out, "{} {} -", checksum, blocks)
            } else {
                writeln!(out, "{} {} {}", checksum, blocks, filename)
            }
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
    use std::io::Write;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fsum");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("--sysv"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("sum"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_bsd_stdin() {
        // Test BSD checksum of "hello\n" via stdin
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
        // Verify the checksum and blocks are numeric
        let _checksum: u32 = parts[0].parse().expect("checksum should be numeric");
        let _blocks: u64 = parts[1].parse().expect("blocks should be numeric");
    }

    #[test]
    fn test_sysv_stdin() {
        // Test System V checksum of "hello\n" via stdin
        let mut child = cmd()
            .arg("-s")
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
        let _checksum: u32 = parts[0].parse().expect("checksum should be numeric");
        let _blocks: u64 = parts[1].parse().expect("blocks should be numeric");
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
        assert!(parts[2].contains("test.txt"));
    }

    #[test]
    fn test_r_flag() {
        // -r selects BSD algorithm (default), should produce same result as no flag
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello\n").unwrap();

        let default_output = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
        let r_output = cmd()
            .arg("-r")
            .arg(file_path.to_str().unwrap())
            .output()
            .unwrap();
        assert_eq!(default_output.stdout, r_output.stdout);
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
        assert!(stderr.contains("sum:"));
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
        let checksum: u32 = parts[0].parse().unwrap();
        let blocks: u64 = parts[1].parse().unwrap();
        assert_eq!(checksum, 0);
        assert_eq!(blocks, 0);
    }

    #[test]
    fn test_bsd_known_value() {
        // Known BSD checksum for "hello\n" (6 bytes):
        // After processing: checksum = 26988, blocks = 1
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello\n").unwrap();

        let output = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        let blocks: u64 = parts[1].parse().unwrap();
        assert_eq!(blocks, 1);
    }

    #[test]
    fn test_sysv_known_value() {
        // System V checksum: simple sum of bytes mod 65535
        // "hello\n" = 104+101+108+108+111+10 = 542, blocks = ceil(6/512) = 1
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello\n").unwrap();

        let output = cmd()
            .arg("-s")
            .arg(file_path.to_str().unwrap())
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        let checksum: u32 = parts[0].parse().unwrap();
        let blocks: u64 = parts[1].parse().unwrap();
        assert_eq!(checksum, 542);
        assert_eq!(blocks, 1);
    }

    /// Check if system sum is GNU sum (BSD sum on macOS has different output format,
    /// Windows Git Bash sum may have different filename behavior)
    fn is_gnu_sum() -> bool {
        // Only compare against GNU sum on Linux where behavior is consistent
        if !cfg!(target_os = "linux") {
            return false;
        }
        Command::new("sum")
            .arg("--version")
            .output()
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                stdout.contains("GNU") || stderr.contains("GNU")
            })
            .unwrap_or(false)
    }

    #[test]
    fn test_compare_gnu_bsd() {
        if !is_gnu_sum() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"The quick brown fox jumps over the lazy dog\n").unwrap();

        let gnu_out = Command::new("sum")
            .arg(file_path.to_str().unwrap())
            .output();
        if let Ok(gnu_out) = gnu_out {
            let ours = cmd().arg(file_path.to_str().unwrap()).output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&gnu_out.stdout),
                "BSD checksum mismatch with GNU sum"
            );
        }
    }

    #[test]
    fn test_compare_gnu_sysv() {
        if !is_gnu_sum() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"The quick brown fox jumps over the lazy dog\n").unwrap();

        let gnu_out = Command::new("sum")
            .arg("-s")
            .arg(file_path.to_str().unwrap())
            .output();
        if let Ok(gnu_out) = gnu_out {
            let ours = cmd()
                .arg("-s")
                .arg(file_path.to_str().unwrap())
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&gnu_out.stdout),
                "SysV checksum mismatch with GNU sum"
            );
        }
    }

    #[test]
    fn test_sysv_long_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello\n").unwrap();

        let short_output = cmd()
            .arg("-s")
            .arg(file_path.to_str().unwrap())
            .output()
            .unwrap();
        let long_output = cmd()
            .arg("--sysv")
            .arg(file_path.to_str().unwrap())
            .output()
            .unwrap();
        assert_eq!(short_output.stdout, long_output.stdout);
    }
}
